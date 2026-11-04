use std::sync::atomic::{AtomicIsize, Ordering};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::{
    HaltError, Value, WackasmModule, compiler::{RuntimeWackasmOperator, WackasmOperator}, dispatch::{DispatchCtx, Exit, dispatch}, module::{ExportIndex, FunctionFrame, FunctionIndex}, pc::ProgramCounter, stack::{
        FrameOperandStack, GlobalStack as _, RuntimeStack as _, StackHandle, val_types_to_size,
    },
};

const MAX_FUEL_BEFORE_SYNC: isize = 1000;

#[derive(Debug, Serialize, Deserialize)]
pub struct WackasmExecutor {
    stack: FrameOperandStack,
    fuel: AtomicIsize,
    module: WackasmModule,
    callstack: Vec<FunctionFrame>,
    output: Option<Vec<Value>>,
}

pub enum FunctionScopedResult {
    OutOfFuel,
    FunctionReturn,
    FunctionCall { function_index: u32 },
}

impl WackasmExecutor {
    pub fn new(module: WackasmModule) -> Result<Self, HaltError> {
        let mut me = Self {
            stack: FrameOperandStack::new(10_000),
            fuel: AtomicIsize::new(isize::MAX),
            module,
            callstack: Vec::new(),
            output: None,
        };

        if let Some(start_func) = me.module.start_func.take() {
            me.module.functions.call_internal(
                &mut me.callstack,
                &mut me.stack,
                start_func,
                Some(Vec::new()),
            )?;
            me.execute()?;
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

        self.module.functions.call_internal(
            &mut self.callstack,
            &mut self.stack,
            *function,
            Some(arguments),
        )
    }

    // runs X instructions, decrementing fuel
    fn incremental_execute(&mut self, fuel: isize) -> Result<isize, HaltError> {
        let (function_result, pc, local_stack, fuel_remaining) =
            self.incremental_execute_function(fuel)?;
        let local_stack = local_stack.into_raw();
        self.stack.reconcile(local_stack);

        // Update PC
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

                self.module.functions.call_internal(
                    &mut self.callstack,
                    &mut self.stack,
                    function_index,
                    None,
                )?;
            }
        }

        Ok(fuel_remaining)
    }

    pub fn set_fuel(&self, fuel: isize) {
        self.fuel.store(fuel, Ordering::Relaxed);
    }

    pub fn get_fuel(&self) -> isize {
        self.fuel.load(Ordering::Relaxed)
    }

    pub fn execute(&mut self) -> Result<(), HaltError> {
        while self.callstack.len() > 0 {
            let remaining_fuel = self.fuel.load(Ordering::Relaxed);
            let target_fuel = remaining_fuel.min(MAX_FUEL_BEFORE_SYNC);
            if target_fuel == 0 {
                break;
            }
            let remaining_fuel = self.incremental_execute(target_fuel)?;
            let fuel_difference = target_fuel - remaining_fuel;
            self.fuel.fetch_sub(fuel_difference, Ordering::Relaxed);
        }
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
        instructions += 10; // Consume fuel for calling functions, just in case functions don't have any loops
        let function_frame = self
            .callstack
            .last_mut()
            .ok_or_else(|| anyhow!("callstack empty"))?;

        let base = function_frame.stack_base;
        let code_view = &self
            .module
            .functions
            .get_function_code(&function_frame.function_index)?
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
