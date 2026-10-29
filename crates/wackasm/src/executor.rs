use std::sync::atomic::{AtomicIsize, Ordering};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::{
    HaltError, Value, WackasmModule,
    compiler::WackasmOperator,
    dispatch::{DispatchCtx, Exit, dispatch},
    module::{ExportIndex, FunctionFrame, FunctionIndex},
    pc::ProgramCounter,
    stack::{
        FrameOperandStack, GlobalStack as _, RuntimeStack as _, StackHandle, val_types_to_size,
    },
};

const MAX_INSTRUCTIONS_IN_LOOP: isize = 500;

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
            let target_fuel = remaining_fuel.min(MAX_INSTRUCTIONS_IN_LOOP);
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
        instructions: isize,
    ) -> Result<(FunctionScopedResult, ProgramCounter, StackHandle<'_>, isize), HaltError> {
        let function_frame = self
            .callstack
            .last_mut()
            .ok_or_else(|| anyhow!("callstack empty"))?;

        let base = function_frame.stack_base;
        let code_view = &self
            .module
            .functions
            .get_function_code(&function_frame.function_index)?
            .inner[..];

        let (stack, locals) = self.stack.handle(base);

        let mut ctx = DispatchCtx {
            memories: &mut self.module.memories,
            functions: &self.module.functions,
            globals: &mut self.module.globals,
            tables: &self.module.tables,
            code: code_view,
            fuel: instructions,
        };

        let base_ptr = code_view.as_ptr();
        let pc = unsafe { base_ptr.add(function_frame.pc) };

        let to_index = |p: *const WackasmOperator| -> ProgramCounter {
            debug_assert!(
                p >= base_ptr && p <= base_ptr.wrapping_add(code_view.len()),
                "program counter went out of bounds"
            );
            unsafe { p.offset_from_unsigned(base_ptr) }
        };

        let exit = dispatch(&mut ctx, pc, stack, locals);
        return match exit {
            Exit::OutOfFuel { pc, stack, fuel } => {
                Ok((FunctionScopedResult::OutOfFuel, to_index(pc), stack, fuel))
            }
            Exit::Call {
                pc,
                stack,
                fuel,
                function_index,
            } => Ok((
                FunctionScopedResult::FunctionCall { function_index },
                to_index(pc),
                stack,
                fuel,
            )),
            Exit::Return { pc, stack, fuel } => Ok((
                FunctionScopedResult::FunctionReturn,
                to_index(pc),
                stack,
                fuel,
            )),
            Exit::Halt(halt_error) => Err(halt_error),
        };

        /*
        for _ in 0..instructions {
            *instructions_executed += 1;
            let operator = unsafe { code_view.get_unchecked(pc) };
            match operator {
                WackasmOperator::Nop => {}
                // Gets
                WackasmOperator::LocalGet { slot } => {
                    let local = locals.get(*slot);
                    stack.push(local);
                }
                WackasmOperator::LocalGetWide { slot } => {
                    let local = locals.get_wide(*slot);
                    stack.push_v128(local);
                }

                WackasmOperator::LocalSet { slot } => {
                    let value = stack.pop();
                    locals.set(*slot, value);
                }
                WackasmOperator::LocalSetWide { slot } => {
                    let value = stack.pop_v128();
                    locals.set_wide(*slot, value);
                }

                WackasmOperator::Drop => {
                    stack.pop();
                }
                WackasmOperator::DropWide => {
                    stack.pop_v128();
                }

                WackasmOperator::LocalTee { slot } => {
                    let value = *stack.peek();
                    locals.set(*slot, value.clone());
                }
                WackasmOperator::LocalTeeWide { slot } => {
                    let value = stack.peek_wide();
                    locals.set_wide(*slot, value);
                }

                WackasmOperator::GlobalGet { global_index } => {
                    let global = self.module.globals.get(&GlobalIndex(*global_index))?;
                    stack.push(*global.inner.get());
                }
                WackasmOperator::GlobalGetWide { global_index } => {
                    let global = self.module.globals.get_wide(&GlobalIndex(*global_index))?;
                    stack.push_v128(global);
                }
                WackasmOperator::GlobalSet { global_index } => {
                    let value = stack.pop();
                    self.module
                        .globals
                        .set(&GlobalIndex(*global_index), value)?;
                }
                WackasmOperator::GlobalSetWide { global_index } => {
                    let value = stack.pop_v128();
                    self.module
                        .globals
                        .set_wide(&GlobalIndex(*global_index), value)?;
                }

                WackasmOperator::MemoryGrow { mem } => {
                    let amount = stack.pop_i32();
                    let result = self.module.memories.grow(*mem, amount)?;
                    stack.push(Value(result as u64));
                }
                WackasmOperator::MemoryFill { mem } => {
                    let amount = stack.pop_i32() as u32 as usize;
                    let value = stack.pop_i32() as u8;
                    let offset = stack.pop_i32() as u32 as usize;
                    self.module.memories.fill(*mem, offset, amount, value)?;
                }

                // memory load
                WackasmOperator::I32Load { memarg } => {
                    i32_load!(stack, self.module.memories, memarg, i32)
                }
                WackasmOperator::I32Load8U { memarg } => {
                    i32_load!(stack, self.module.memories, memarg, u8)
                }
                WackasmOperator::I32Load8S { memarg } => {
                    i32_load!(stack, self.module.memories, memarg, i8)
                }
                WackasmOperator::I32Load16U { memarg } => {
                    i32_load!(stack, self.module.memories, memarg, u16)
                }
                WackasmOperator::I32Load16S { memarg } => {
                    i32_load!(stack, self.module.memories, memarg, i16)
                }

                WackasmOperator::I64Load { memarg } => {
                    i64_load!(stack, self.module.memories, memarg, i64);
                }
                WackasmOperator::I64Load8U { memarg } => {
                    i64_load!(stack, self.module.memories, memarg, u8)
                }
                WackasmOperator::I64Load8S { memarg } => {
                    i64_load!(stack, self.module.memories, memarg, i8);
                }
                WackasmOperator::I64Load16U { memarg } => {
                    i64_load!(stack, self.module.memories, memarg, u16);
                }
                WackasmOperator::I64Load16S { memarg } => {
                    i64_load!(stack, self.module.memories, memarg, i16);
                }
                WackasmOperator::I64Load32U { memarg } => {
                    i64_load!(stack, self.module.memories, memarg, u32);
                }
                WackasmOperator::I64Load32S { memarg } => {
                    i64_load!(stack, self.module.memories, memarg, i32);
                }

                WackasmOperator::F32Load { memarg } => {
                    f32_load!(stack, self.module.memories, memarg)
                }
                WackasmOperator::F64Load { memarg } => {
                    f64_load!(stack, self.module.memories, memarg)
                }
                WackasmOperator::V128Load { memarg } => {
                    v128_load!(stack, self.module.memories, memarg)
                }

                // memory store
                WackasmOperator::I32Store { memarg } => {
                    i32_store!(stack, self.module.memories, memarg, i32)
                }
                WackasmOperator::I32Store8 { memarg } => {
                    i32_store!(stack, self.module.memories, memarg, u8)
                }
                WackasmOperator::I32Store16 { memarg } => {
                    i32_store!(stack, self.module.memories, memarg, u16)
                }

                WackasmOperator::I64Store { memarg } => {
                    i64_store!(stack, self.module.memories, memarg, i64)
                }
                WackasmOperator::I64Store8 { memarg } => {
                    i64_store!(stack, self.module.memories, memarg, u8)
                }
                WackasmOperator::I64Store16 { memarg } => {
                    i64_store!(stack, self.module.memories, memarg, u16)
                }
                WackasmOperator::I64Store32 { memarg } => {
                    i64_store!(stack, self.module.memories, memarg, u32)
                }

                WackasmOperator::F32Store { memarg } => {
                    f32_store!(stack, self.module.memories, memarg)
                }
                WackasmOperator::F64Store { memarg } => {
                    f64_store!(stack, self.module.memories, memarg)
                }
                WackasmOperator::V128Store { memarg } => {
                    v128_store!(stack, self.module.memories, memarg)
                }

                // Handle function call
                WackasmOperator::Call { function_index } => {
                    pc += 1;
                    return Ok((
                        FunctionScopedResult::FunctionCall {
                            function_index: *function_index,
                        },
                        pc,
                        stack,
                    ));
                }
                WackasmOperator::CallIndirect {
                    table_index,
                    type_index,
                } => {
                    let func_index_index = stack.pop_i32();
                    let table = self
                        .module
                        .tables
                        .get(&TableIndex(*table_index))
                        .ok_or_else(|| anyhow!("no table for index {}", table_index))?;

                    let table_value = table
                        .get(func_index_index as u32 as usize)
                        .ok_or_else(|| trap!("undefined element {}", func_index_index as u32))?;

                    let func_index = match table_value {
                        TableValue::FuncRef(items) => items,
                        TableValue::NullRef => return Err(trap!("uninitialized element")),
                    };

                    // Type check, per spec
                    let expected = self
                        .module
                        .functions
                        .function_type_idx_type(&FunctionTypeIndex(*type_index))?;
                    let actual = self.module.functions.function_type_idx(&func_index)?;
                    if expected != actual {
                        return Err(trap!("indirect call type mismatch"));
                    }

                    pc += 1;
                    return Ok((
                        FunctionScopedResult::FunctionCall {
                            function_index: func_index.0,
                        },
                        pc,
                        stack,
                    ));
                }

                WackasmOperator::Jump { target } => {
                    pc = *target as usize;
                    continue;
                }
                WackasmOperator::JumpIf { target } => {
                    if stack.pop_i32() != 0 {
                        pc = *target as usize;
                        continue;
                    }
                }
                WackasmOperator::JumpIfEqz { target } => {
                    if stack.pop_i32() == 0 {
                        pc = *target as usize;
                        continue;
                    }
                }
                WackasmOperator::Br { target, drop_keep } => {
                    stack.drop_keep(*drop_keep);
                    pc = *target as usize;
                    continue;
                }
                WackasmOperator::BrIf { target, drop_keep } => {
                    if stack.pop_i32() != 0 {
                        stack.drop_keep(*drop_keep);
                        pc = *target as usize;

                        continue;
                    }
                }

                WackasmOperator::BrTable { targets } => {
                    let i = stack.pop_i32() as u32 as usize;
                    match targets[i.min(targets.len() - 1)] {
                        WackasmTableTarget::Return => {
                            return Ok((FunctionScopedResult::FunctionReturn, pc, stack));
                        }
                        WackasmTableTarget::Break { target, dk } => {
                            stack.drop_keep(dk);
                            pc = target as usize;
                            continue;
                        }
                    }
                }

                WackasmOperator::Return => {
                    return Ok((FunctionScopedResult::FunctionReturn, pc, stack));
                }
                WackasmOperator::ReturnIf => {
                    if stack.pop_i32() != 0 {
                        return Ok((FunctionScopedResult::FunctionReturn, pc, stack));
                    }
                }

                // i32 maths
                WackasmOperator::I32Const { value } => {
                    stack.push(Value((*value) as u64));
                }
                WackasmOperator::I64Const { value } => {
                    stack.push(Value((*value) as u64));
                }
                WackasmOperator::F32Const { value } => {
                    stack.push(Value(value.to_bits() as u64));
                }
                WackasmOperator::F64Const { value } => {
                    stack.push(Value(value.to_bits()));
                }

                WackasmOperator::I32Add => {
                    i32_binop!(stack, i32, |lhs, rhs| lhs.wrapping_add(rhs))
                }
                WackasmOperator::I32Mul => {
                    i32_binop!(stack, i32, |lhs, rhs| lhs.wrapping_mul(rhs))
                }
                WackasmOperator::I32DivU => {
                    i32_binop!(stack, u32, |lhs, rhs| lhs
                        .checked_div(rhs)
                        .ok_or_else(|| trap!("integer divide by zero"))?)
                }
                WackasmOperator::I32Sub => {
                    i32_binop!(stack, i32, |lhs, rhs| lhs.wrapping_sub(rhs))
                }
                WackasmOperator::I32ShrU => {
                    i32_binop!(stack, u32, |lhs, rhs| lhs.wrapping_shr(rhs))
                }
                WackasmOperator::I32Shl => {
                    i32_binop!(stack, u32, |lhs, rhs| lhs.wrapping_shl(rhs))
                }
                WackasmOperator::I32And => {
                    i32_binop!(stack, i32, |lhs, rhs| lhs & rhs)
                }
                WackasmOperator::I32Xor => {
                    i32_binop!(stack, i32, |lhs, rhs| lhs ^ rhs)
                }

                // i32 comparators
                WackasmOperator::I32GtS => {
                    i32_binop!(stack, i32, |lhs, rhs| lhs > rhs)
                }
                WackasmOperator::I32LtS => {
                    i32_binop!(stack, i32, |lhs, rhs| lhs < rhs)
                }
                WackasmOperator::I32LtU => {
                    i32_binop!(stack, u32, |lhs, rhs| lhs < rhs)
                }
                WackasmOperator::I32GtU => {
                    i32_binop!(stack, u32, |lhs, rhs| lhs > rhs)
                }
                WackasmOperator::I32LeU => {
                    i32_binop!(stack, u32, |lhs, rhs| lhs <= rhs)
                }
                WackasmOperator::I32Ne => {
                    i32_binop!(stack, i32, |a, b| a != b)
                }
                WackasmOperator::I32GeU => {
                    i32_binop!(stack, u32, |lhs, rhs| lhs >= rhs)
                }
                WackasmOperator::I32Eqz => {
                    let a = stack.pop_i32();
                    let result = if a == 0 { 1 } else { 0 };
                    stack.push(Value(result as u64));
                }
                WackasmOperator::I32Eq => {
                    i32_binop!(stack, i32, |a, b| a == b)
                }
                WackasmOperator::I32Or => {
                    i32_binop!(stack, i32, |a, b| a | b)
                }

                // i64 maths
                WackasmOperator::I64Add => {
                    i64_binop!(stack, i64, |lhs, rhs| lhs.wrapping_add(rhs))
                }
                WackasmOperator::I64Mul => {
                    i64_binop!(stack, i64, |lhs, rhs| lhs.wrapping_mul(rhs))
                }
                WackasmOperator::I64Sub => {
                    i64_binop!(stack, i64, |lhs, rhs| lhs.wrapping_sub(rhs))
                }
                WackasmOperator::I64ShrU => {
                    i64_binop!(stack, u64, |lhs, rhs| lhs.wrapping_shr(rhs as u32))
                }
                WackasmOperator::I64Shl => {
                    i64_binop!(stack, u64, |lhs, rhs| lhs.wrapping_shl(rhs as u32))
                }
                WackasmOperator::I64And => {
                    i64_binop!(stack, i64, |lhs, rhs| lhs & rhs)
                }
                WackasmOperator::I64Or => {
                    i64_binop!(stack, i64, |lhs, rhs| lhs | rhs)
                }

                // i64 comparators
                WackasmOperator::I64GtS => {
                    i64_cmp!(stack, i64, |lhs, rhs| lhs > rhs)
                }
                WackasmOperator::I64LtU => {
                    i64_cmp!(stack, u64, |lhs, rhs| lhs < rhs)
                }
                WackasmOperator::I64GtU => {
                    i64_cmp!(stack, u64, |lhs, rhs| lhs > rhs)
                }
                WackasmOperator::I64LeU => {
                    i64_cmp!(stack, u64, |lhs, rhs| lhs <= rhs)
                }
                WackasmOperator::I64GeU => {
                    i64_cmp!(stack, u64, |lhs, rhs| lhs >= rhs)
                }
                WackasmOperator::I64Eq => {
                    i64_cmp!(stack, i64, |lhs, rhs| lhs == rhs)
                }
                WackasmOperator::I64Ne => {
                    i64_cmp!(stack, i64, |lhs, rhs| lhs != rhs)
                }
                WackasmOperator::I64Eqz => {
                    let a = stack.pop_i64();
                    stack.push(Value((a == 0) as u64));
                }
                WackasmOperator::Select { wide } => {
                    let cond = stack.pop_i32() == 0;

                    if *wide {
                        let top = stack.pop_v128();
                        let bottom = stack.pop_v128();
                        stack.push_v128(if cond { top } else { bottom });
                    } else {
                        let top = stack.pop();
                        let bottom = stack.pop();
                        stack.push(if cond { top } else { bottom });
                    }
                }
                WackasmOperator::I64ExtendI32U => {
                    let value = stack.pop_i32();
                    let new_value = value as u32 as i64;
                    stack.push(Value(new_value as u64));
                }
                WackasmOperator::I32WrapI64 => {
                    let value = stack.pop_i64();
                    let value = value as i32;
                    stack.push(Value(value as u64));
                }
                WackasmOperator::Unreachable => {
                    return Err(trap!("unreachable"));
                }
                WackasmOperator::Unsupported { description } => {
                    return Err(unsupported!("unsupported operator: {}", description));
                }
            }

            pc += 1;
        }

         */
    }
}
