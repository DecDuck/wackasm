use std::sync::atomic::{AtomicIsize, Ordering};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::{
    HaltError, Value, WackasmModule,
    dispatch::{DispatchCtx, Exit, dispatch},
    functions::WackasmFunction,
    imports::{WackasmHostBridge, WackasmImports, WackasmResource},
    macros::trap,
    memory::MemoryManager,
    module::{ExportIndex, FunctionFrame, FunctionIndex, PendingImportCall},
    stack::FrameOperandStack,
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
    initializing: bool,
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
        let mut executor = Self {
            stack: FrameOperandStack::new(10_000),
            fuel: AtomicIsize::new(isize::MAX),
            module,
            callstack: Vec::new(),
            pending_import: None,
            output: None,
            initializing: false,
        };
        if let Some(start_func) = executor.module.start_func.take() {
            executor.initializing = true;
            executor.queue_function(start_func, &[])?;
            match executor.execute()? {
                HostYield::Return => {}
                HostYield::ImportCall { .. } => {}
                HostYield::OutOfFuel => {
                    return Err(trap!("ran out of fuel while running _start"));
                }
            }
        }
        Ok(executor)
    }

    fn queue_function(&mut self, index: FunctionIndex, args: &[Value]) -> Result<(), HaltError> {
        let info = self
            .module
            .functions
            .info
            .get(index.0 as usize)
            .ok_or_else(|| anyhow!("internally inconsistent function index"))?;
        if args.len() != info.params_slots {
            return Err(trap!("invalid arguments len"));
        }
        if info.frame_size > self.stack.capacity() {
            return Err(trap!("potential stack overflow"));
        }
        self.stack.write(0, args)?;
        match self.module.functions.get_function(&index)? {
            WackasmFunction::Internal(body) => {
                self.stack.write(info.params_slots, &body.local_defaults)?;
                self.callstack.push(FunctionFrame {
                    function_index: index,
                    pc: 0,
                    stack_base: 0,
                });
            }
            WackasmFunction::Import {
                module,
                name,
                returns,
                ..
            } => {
                self.pending_import = Some(PendingImportCall {
                    module: module.clone(),
                    name: name.clone(),
                    args: args.to_vec(),
                    should_return: *returns,
                    stack_base: 0,
                });
            }
        }
        Ok(())
    }

    /// Resets the executor and queues the function for running.
    pub fn call_export(&mut self, name: &str, arguments: Vec<Value>) -> Result<(), HaltError> {
        if self.initializing {
            return Err(anyhow!(
                "start function has not completed; execute it before calling exports"
            )
            .into());
        }
        self.output = None;
        self.pending_import = None;
        self.callstack.clear();
        let function = match self.module.exports.get(name) {
            Some(ExportIndex::Func(index)) => *index,
            Some(_) => return Err(anyhow!("not a function").into()),
            None => return Err(anyhow!("function not found").into()),
        };
        self.queue_function(function, &arguments)
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
        loop {
            if let Some(pending) = &self.pending_import {
                return Ok(HostYield::ImportCall {
                    module: pending.module.clone(),
                    name: pending.name.clone(),
                    args: pending.args.clone(),
                    should_return: pending.should_return,
                });
            }
            let Some(frame) = self.callstack.last().copied() else {
                self.initializing = false;
                return Ok(HostYield::Return);
            };
            let target_fuel = self.get_fuel().min(MAX_FUEL_BEFORE_SYNC);
            if target_fuel <= 0 {
                return Ok(HostYield::OutOfFuel);
            }
            let info = &self.module.functions.info[frame.function_index.0 as usize];
            if frame.pc >= info.code_len {
                return Err(anyhow!("program counter out of bounds").into());
            }
            let buffer = self.stack.as_mut_ptr();
            let memory0 = self.module.memories.memory0();
            let mut ctx = DispatchCtx {
                memories: &mut self.module.memories,
                globals: &mut self.module.globals,
                tables: &self.module.tables,
                functions: &self.module.functions,
                frames: &mut self.callstack,
                buffer,
                capacity: self.stack.capacity(),
                fuel: target_fuel,
                exit: None,
            };
            dispatch(
                &mut ctx,
                unsafe { info.code.add(frame.pc) },
                unsafe { buffer.add(frame.stack_base) },
                memory0,
            );
            let remaining = ctx.fuel;
            let exit = ctx
                .exit
                .take()
                .ok_or_else(|| anyhow!("dispatch yielded without an exit"))?;
            self.fuel
                .fetch_sub(target_fuel - remaining, Ordering::Relaxed);
            match exit {
                Exit::OutOfFuel => {}
                Exit::Trap(error) => return Err(error),
                Exit::Return { base, len } => {
                    self.output = Some(self.stack.read(base, len)?.to_vec());
                }
                Exit::ImportCall {
                    function_index,
                    base,
                } => {
                    let WackasmFunction::Import {
                        module,
                        name,
                        args,
                        returns,
                    } = self
                        .module
                        .functions
                        .get_function(&FunctionIndex(function_index))?
                    else {
                        return Err(anyhow!("expected import function").into());
                    };
                    self.pending_import = Some(PendingImportCall {
                        module: module.clone(),
                        name: name.clone(),
                        args: self.stack.read(base, *args)?.to_vec(),
                        should_return: *returns,
                        stack_base: base,
                    });
                }
            }
        }
    }

    pub fn import_return(&mut self, results: &[Value]) -> Result<(), HaltError> {
        let pending = self
            .pending_import
            .as_ref()
            .ok_or_else(|| trap!("callstack empty or not expecting import return"))?;
        if results.len() != pending.should_return {
            return Err(trap!(
                "unexpected import result count: expected {} vs got {}",
                pending.should_return,
                results.len()
            ));
        }
        self.stack.write(pending.stack_base, results)?;
        if self.callstack.is_empty() {
            self.output = Some(results.to_vec());
        }
        self.pending_import = None;
        Ok(())
    }

    pub fn get_result(&self) -> Option<&Vec<Value>> {
        self.output.as_ref()
    }

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
                        return Err(anyhow!("host_bridge returned wrong result len").into());
                    }
                    self.import_return(&result)?;
                }
                value => return Ok(value),
            }
        }
    }
}
