use std::sync::atomic::{AtomicIsize, Ordering};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::{
    HaltError, Value, WackasmModule,
    compiler::RuntimeWackasmOperator,
    dispatch::{DispatchCtx, Exit, dispatch},
    imports::{WackasmHostBridge, WackasmImports, WackasmResource},
    macros::trap,
    memory::MemoryManager,
    module::{ExportIndex, FunctionFrame, FunctionIndex, PendingImportCall},
    pc::ProgramCounter,
    stack::{
        FrameOperandStack, GlobalStack as _, RuntimeStack as _, StackHandle, val_types_to_size,
    },
};

const MAX_FUEL_BEFORE_SYNC: isize = 100_000;

#[derive(Serialize, Deserialize)]
pub struct WackasmExecutor {
    stack: FrameOperandStack,
    fuel: AtomicIsize,
    module: WackasmModule,
    callstack: Vec<FunctionFrame>,
    pending_import: Option<PendingImportCall>,
    output: Option<Vec<Value>>,
}

pub enum FunctionScopedResult {
    OutOfFuel,
    FunctionReturn,
    FunctionCall { function_index: u32 },
}

#[must_use]
pub enum HostYield {
    OutOfFuel,
    Return,
    ImportCall {
        module: String,
        name: String,
        args: Vec<Value>,
        should_return: usize,
    },
}

impl WackasmExecutor {
    pub fn new(module: WackasmModule) -> Result<Self, HaltError> {
        let mut me = Self {
            stack: FrameOperandStack::new(10_000),
            fuel: AtomicIsize::new(isize::MAX),
            module,
            callstack: Vec::new(),
            pending_import: None,
            output: None,
        };

        if let Some(start_func) = me.module.start_func.take() {
            me.module.functions.call_internal(
                &mut me.pending_import,
                &mut me.callstack,
                &mut me.stack,
                start_func,
                Some(Vec::new()),
            )?;
            if !matches!(me.execute()?, HostYield::Return) {
                return Err(trap!("got a non-return host yield while running _start"));
            }
        }

        Ok(me)
    }

    /// Resets the executor and queues the function for running
    pub fn call_export(&mut self, name: &str, arguments: Vec<Value>) -> Result<(), HaltError> {
        self.output.take();
        self.callstack.clear();
        self.stack.clear();

        let function = self
            .module
            .exports
            .get(name)
            .ok_or_else(|| anyhow!("function not found"))?;
        let function = match function {
            ExportIndex::Func(function_type_index) => function_type_index,
            _ => return Err(HaltError::Error(anyhow!("not a function"))),
        };

        self.module
            .functions
            .call_internal(
                &mut self.pending_import,
                &mut self.callstack,
                &mut self.stack,
                *function,
                Some(arguments),
            )?
            .map_or(Ok(()), |_| {
                Err(anyhow!("got a host yield while calling export"))
            })?;

        Ok(())
    }

    // runs X instructions, decrementing fuel
    fn incremental_execute(
        &mut self,
        fuel: isize,
    ) -> Result<(isize, Option<HostYield>), HaltError> {
        let (function_result, pc, local_stack, fuel_remaining) =
            self.incremental_execute_function(fuel)?;
        let local_stack = local_stack.into_raw();
        self.stack.reconcile(local_stack);

        if let Some(pending_import_call) = &self.pending_import {
            return Ok((
                fuel,
                Some(HostYield::ImportCall {
                    module: pending_import_call.module.to_owned(),
                    name: pending_import_call.name.to_owned(),
                    args: pending_import_call.args.to_vec(),
                    should_return: pending_import_call.should_return,
                }),
            ));
        }

        // Update PC or return ImportCall
        self.callstack
            .last_mut()
            .ok_or_else(|| anyhow!("callstack empty"))?
            .pc = pc;

        match function_result {
            FunctionScopedResult::OutOfFuel => {} // nothing, just continue
            FunctionScopedResult::FunctionReturn => {
                // Now we get to pop the function frame and push the results to
                let function_frame = self
                    .callstack
                    .pop()
                    .expect("incremental_execute_function would have errored by now");

                let function_type = self
                    .module
                    .functions
                    .function_type_idx(&function_frame.function_index)?;

                let result_types = function_type.results();

                if self.callstack.is_empty() {
                    let results = self.stack.pull_results(result_types)?;
                    self.output = Some(results);
                } else {
                    self.stack
                        .unwind_to(function_frame.stack_base, val_types_to_size(result_types));
                }
            }
            FunctionScopedResult::FunctionCall { function_index } => {
                let function_index = FunctionIndex(function_index);

                if let Some(host_yield) = self.module.functions.call_internal(
                    &mut self.pending_import,
                    &mut self.callstack,
                    &mut self.stack,
                    function_index,
                    None,
                )? {
                    return Ok((fuel_remaining, Some(host_yield)));
                }
            }
        }

        Ok((fuel_remaining, None))
    }

    pub fn set_fuel(&self, fuel: isize) {
        self.fuel.store(fuel, Ordering::Relaxed);
    }

    pub fn get_fuel(&self) -> isize {
        self.fuel.load(Ordering::Relaxed)
    }

    pub fn memory(&self) -> &MemoryManager {
        &self.module.memories
    }

    pub fn execute(&mut self) -> Result<HostYield, HaltError> {
        while self.callstack.len() > 0 {
            let remaining_fuel = self.fuel.load(Ordering::Relaxed);
            let target_fuel = remaining_fuel.min(MAX_FUEL_BEFORE_SYNC);
            if target_fuel <= 0 {
                return Ok(HostYield::OutOfFuel);
            }
            let (remaining_fuel, host_yield) = self.incremental_execute(target_fuel)?;
            let fuel_difference = target_fuel - remaining_fuel;
            self.fuel.fetch_sub(fuel_difference, Ordering::Relaxed);

            if let Some(host_yield) = host_yield {
                return Ok(host_yield);
            }
        }

        Ok(HostYield::Return)
    }

    pub fn import_return(&mut self, results: &[Value]) -> Result<(), HaltError> {
        if self.pending_import.is_none() {
            return Err(trap!("callstack empty or not expecting import return"));
        }

        self.pending_import.take(); // Remove the pending call
        self.stack.push_all(results);

        Ok(())
    }

    pub fn get_result(&self) -> Option<&Vec<Value>> {
        self.output.as_ref()
    }

    /// Executes up to [`instructions`] instructions within a single
    /// function frame. Adds how many were executed to instructions_executed.
    fn incremental_execute_function(
        &mut self,
        mut instructions: isize,
    ) -> Result<(FunctionScopedResult, ProgramCounter, StackHandle<'_>, isize), HaltError> {
        instructions -= 1; // Consume fuel for calling functions, just in case functions don't have any loops
        let function_frame = self
            .callstack
            .last_mut()
            .ok_or_else(|| anyhow!("callstack empty"))?;

        let base = function_frame.stack_base;
        let code_view = &self
            .module
            .functions
            .get_function(&function_frame.function_index)?
            .as_internal()?
            .runtime[..];

        let (stack, locals) = self.stack.handle(base);

        let mut ctx = DispatchCtx {
            memories: &mut self.module.memories,
            functions: &self.module.functions,
            globals: &mut self.module.globals,
            tables: &self.module.tables,
            code: code_view,
            fuel: instructions,
            exit: None,
        };

        let base_ptr = code_view.as_ptr();
        let pc = unsafe { base_ptr.add(function_frame.pc) };

        let to_index = |p: *const RuntimeWackasmOperator| -> ProgramCounter {
            debug_assert!(
                p >= base_ptr && p <= base_ptr.wrapping_add(code_view.len()),
                "program counter went out of bounds"
            );
            unsafe { p.offset_from_unsigned(base_ptr) }
        };

        dispatch(&mut ctx, pc, stack, locals)?;
        return match ctx
            .exit
            .ok_or_else(|| anyhow!("dispatch yieled but no exit value"))?
        {
            Exit::OutOfFuel { pc, stack } => Ok((
                FunctionScopedResult::OutOfFuel,
                to_index(pc),
                stack,
                ctx.fuel,
            )),
            Exit::Call {
                pc,
                stack,
                function_index,
            } => Ok((
                FunctionScopedResult::FunctionCall { function_index },
                to_index(pc),
                stack,
                ctx.fuel,
            )),
            Exit::Return { pc, stack } => Ok((
                FunctionScopedResult::FunctionReturn,
                to_index(pc),
                stack,
                ctx.fuel,
            )),
        };
    }
}

impl WackasmExecutor {
    pub fn execute_with_imports<Ctx, I: WackasmImports<Ctx, R>, R: WackasmResource>(
        &mut self,
        host_bridge: &mut WackasmHostBridge<Ctx, I, R>,
    ) -> Result<HostYield, HaltError> {
        loop {
            match self.execute()? {
                HostYield::ImportCall {
                    module,
                    name,
                    args,
                    should_return,
                } => {
                    let result = host_bridge.call_untyped(self, &module, &name, args)?;
                    if result.len() != should_return {
                        Err(anyhow!("host_bridge returned wrong result len"))?
                    }
                    self.import_return(&result)?;
                }
                v => return Ok(v),
            }
        }
    }
}
