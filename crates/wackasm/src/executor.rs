use std::sync::atomic::{AtomicIsize, Ordering};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::Operator;

use crate::{
    HaltError, Value, WackasmModule,
    blocks::{Block, BlockManager},
    functions::FunctionManager,
    macros::{
        f32_load, f32_store, f64_load, f64_store, i32_binop, i32_load, i32_store, i64_binop,
        i64_cmp, i64_load, i64_store, trap, unsupported, v128_load, v128_store,
    },
    module::{
        BlockIndex, ExportIndex, FunctionFrame, FunctionIndex, GlobalIndex, TableIndex, TableValue,
    },
    pc::ProgramCounter,
    stack::{FrameOperandStack, GlobalStack as _, StackSection},
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
            stack: FrameOperandStack::new(100),
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
        }

        Ok(me)
    }

    /// Resets the executor and queues the function for running
    pub fn call_export(&mut self, name: &str, arguments: Vec<Value>) -> Result<(), HaltError> {
        self.output.take();
        self.callstack.clear();

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

    fn block_break(
        stack: &mut FrameOperandStack,
        blocks: &BlockManager,
        functions: &FunctionManager,
        pc: &mut ProgramCounter,
        relative_depth: u32,
    ) -> Result<Option<FunctionScopedResult>, HaltError> {
        let target = stack.len_sections() - 1 - relative_depth as usize;
        let block = stack
            .peek_section(Some(target))
            .ok_or_else(|| anyhow!("no block to escape when br_if"))?;

        match block {
            StackSection::Block(block_index) => {
                let block = blocks.get(block_index)?;
                match block {
                    Block::Block { end, .. } | Block::If { end, .. } => {
                        let return_params = block.return_params(functions)?;
                        stack.return_section(return_params.unwrap_or(&[]), Some(target))?;
                        *pc = *end + 1;
                    }
                    // Keep the loop's section and skip re-executing the `loop` opcode
                    Block::Loop { start, .. } => {
                        let call_params = block.call_params(functions)?;
                        stack.reset_section(call_params.as_deref().unwrap_or(&[]), target)?;
                        *pc = *start + 1;
                    }
                }
            }
            StackSection::Function(_) => {
                return Ok(Some(FunctionScopedResult::FunctionReturn));
            }
        }

        Ok(None)
    }

    // runs X instructions, decrementing fuel
    #[inline]
    fn incremental_execute(&mut self, instructions: usize) -> Result<isize, HaltError> {
        let mut instructions_executed = 0;

        let (function_result, pc) =
            self.incremental_execute_function(instructions, &mut instructions_executed)?;

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

                self.stack
                    .return_function(result_types)?
                    .ok_or_else(|| anyhow!("no function section to return from"))?
                    .expect_function()?;

                if self.callstack.is_empty() {
                    let results = self.stack.pull_results(result_types)?;
                    self.output = Some(results);
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

        Ok(instructions_executed)
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
            let executed = self.incremental_execute(target_fuel as usize)?;
            self.fuel.fetch_sub(executed, Ordering::Relaxed);
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
        instructions: usize,
        instructions_executed: &mut isize,
    ) -> Result<(FunctionScopedResult, ProgramCounter), HaltError> {
        let function_frame = self
            .callstack
            .last_mut()
            .ok_or_else(|| anyhow!("callstack empty"))?;

        let mut pc = function_frame.pc;

        for _ in 0..instructions {
            *instructions_executed += 1;
            let operator = function_frame.code.get(pc).unwrap();
            #[cfg(feature = "op-profile")]
            crate::profiling::begin(operator);
            match operator {
                Operator::Nop => {}
                // Gets
                Operator::LocalGet { local_index } => {
                    let local = *function_frame.locals.get(&local_index)?;
                    self.stack.push(local)?;
                }
                Operator::LocalSet { local_index } => {
                    let value = self.stack.pop()?;
                    function_frame.locals.set_checked(*local_index, value)?;
                }
                Operator::Drop => {
                    self.stack.pop()?;
                }
                Operator::LocalTee { local_index } => {
                    let value = self.stack.peek()?;
                    function_frame
                        .locals
                        .set_checked(*local_index, value.clone())?;
                }
                Operator::GlobalGet { global_index } => {
                    let global = self.module.globals.get(&GlobalIndex(*global_index))?;
                    self.stack.push(global.inner.clone())?;
                }
                Operator::GlobalSet { global_index } => {
                    let value = self.stack.pop()?;
                    self.module
                        .globals
                        .set(&GlobalIndex(*global_index), value)?;
                }

                Operator::MemoryGrow { mem } => {
                    let amount = self.stack.pop_i32()?;
                    let result = self.module.memories.grow(*mem, amount)?;
                    self.stack.push(Value(result as u64))?;
                }
                Operator::MemoryFill { mem } => {
                    let amount = self.stack.pop_i32()? as u32 as usize;
                    let value = self.stack.pop_i32()? as u8;
                    let offset = self.stack.pop_i32()? as u32 as usize;
                    self.module.memories.fill(*mem, offset, amount, value)?;
                }

                // memory load
                Operator::I32Load { memarg } => {
                    i32_load!(self.stack, self.module.memories, memarg, i32)
                }
                Operator::I32Load8U { memarg } => {
                    i32_load!(self.stack, self.module.memories, memarg, u8)
                }
                Operator::I32Load8S { memarg } => {
                    i32_load!(self.stack, self.module.memories, memarg, i8)
                }
                Operator::I32Load16U { memarg } => {
                    i32_load!(self.stack, self.module.memories, memarg, u16)
                }
                Operator::I32Load16S { memarg } => {
                    i32_load!(self.stack, self.module.memories, memarg, i16)
                }

                Operator::I64Load { memarg } => {
                    i64_load!(self.stack, self.module.memories, memarg, i64);
                }
                Operator::I64Load8U { memarg } => {
                    i64_load!(self.stack, self.module.memories, memarg, u8)
                }
                Operator::I64Load8S { memarg } => {
                    i64_load!(self.stack, self.module.memories, memarg, i8);
                }
                Operator::I64Load16U { memarg } => {
                    i64_load!(self.stack, self.module.memories, memarg, u16);
                }
                Operator::I64Load16S { memarg } => {
                    i64_load!(self.stack, self.module.memories, memarg, i16);
                }
                Operator::I64Load32U { memarg } => {
                    i64_load!(self.stack, self.module.memories, memarg, u32);
                }
                Operator::I64Load32S { memarg } => {
                    i64_load!(self.stack, self.module.memories, memarg, i32);
                }

                Operator::F32Load { memarg } => {
                    f32_load!(self.stack, self.module.memories, memarg)
                }
                Operator::F64Load { memarg } => {
                    f64_load!(self.stack, self.module.memories, memarg)
                }
                Operator::V128Load { memarg } => {
                    v128_load!(self.stack, self.module.memories, memarg)
                }

                // memory store
                Operator::I32Store { memarg } => {
                    i32_store!(self.stack, self.module.memories, memarg, i32)
                }
                Operator::I32Store8 { memarg } => {
                    i32_store!(self.stack, self.module.memories, memarg, u8)
                }
                Operator::I32Store16 { memarg } => {
                    i32_store!(self.stack, self.module.memories, memarg, u16)
                }

                Operator::I64Store { memarg } => {
                    i64_store!(self.stack, self.module.memories, memarg, i64)
                }
                Operator::I64Store8 { memarg } => {
                    i64_store!(self.stack, self.module.memories, memarg, u8)
                }
                Operator::I64Store16 { memarg } => {
                    i64_store!(self.stack, self.module.memories, memarg, u16)
                }
                Operator::I64Store32 { memarg } => {
                    i64_store!(self.stack, self.module.memories, memarg, u32)
                }

                Operator::F32Store { memarg } => {
                    f32_store!(self.stack, self.module.memories, memarg)
                }
                Operator::F64Store { memarg } => {
                    f64_store!(self.stack, self.module.memories, memarg)
                }
                Operator::V128Store { memarg } => {
                    v128_store!(self.stack, self.module.memories, memarg)
                }

                // Handle function call
                Operator::Call { function_index } => {
                    pc += 1;
                    return Ok((
                        FunctionScopedResult::FunctionCall {
                            function_index: *function_index,
                        },
                        pc,
                    ));
                }
                Operator::CallIndirect {
                    type_index: _,
                    table_index,
                } => {
                    let func_index_index = self.stack.pop_i32()?;
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

                    pc += 1;
                    return Ok((
                        FunctionScopedResult::FunctionCall {
                            function_index: func_index.0,
                        },
                        pc,
                    ));
                }

                // Control flow
                Operator::If { blockty: _ } => {
                    let block_index = BlockIndex(pc as u32);
                    let block = function_frame.block_lookup.get(&block_index)?;

                    let condition = self.stack.pop_i32()?;
                    let condition = condition != 0;

                    let (start, else_block, end) = block.if_block()?;
                    let mut start_block = false;

                    if condition {
                        // Jump to next instruction
                        pc = start + 1;
                        start_block = true;
                    } else {
                        if let Some(else_block) = else_block {
                            start_block = true;
                            // Jump after else instruction
                            pc = else_block + 1;
                        } else {
                            // Jump after end
                            pc = end + 1;
                        }
                    }

                    if start_block {
                        self.module
                            .start_block(&mut self.stack, &block, &block_index)?;
                    }

                    // Skip PC increment
                    continue;
                }

                // Jump to end block if we hit an else
                Operator::Else => {
                    let block_index = self
                        .stack
                        .peek_section(None)
                        .ok_or_else(|| anyhow!("hit else op not in block"))?
                        .expect_block()?;
                    let block = function_frame.block_lookup.get(block_index)?;

                    let (_, _, end) = block.if_block()?;
                    // Jump to end block which will handle block frame cleanup
                    pc = end;

                    // Skip PC incremenet
                    continue;
                }

                Operator::Block { blockty: _ } | Operator::Loop { blockty: _ } => {
                    let block_index = BlockIndex(pc as u32);
                    let block = function_frame.block_lookup.get(&block_index)?;

                    self.module
                        .start_block(&mut self.stack, &block, &block_index)?;
                }

                Operator::BrIf { relative_depth } => {
                    let condition = self.stack.pop_i32()?;
                    let branching = condition != 0;

                    if branching {
                        if let Some(result) = WackasmExecutor::block_break(
                            &mut self.stack,
                            &function_frame.block_lookup,
                            &self.module.functions,
                            &mut pc,
                            *relative_depth,
                        )? {
                            return Ok((result, pc));
                        }

                        // Skip PC increment
                        continue;
                    }
                }

                Operator::Br { relative_depth } => {
                    if let Some(result) = WackasmExecutor::block_break(
                        &mut self.stack,
                        &function_frame.block_lookup,
                        &self.module.functions,
                        &mut pc,
                        *relative_depth,
                    )? {
                        return Ok((result, pc));
                    }

                    // Skip PC increment
                    continue;
                }

                Operator::BrTable { targets } => {
                    let decider = self.stack.pop_i32()? as u32;
                    let target = if decider >= targets.len() {
                        targets.default()
                    } else {
                        targets.targets().nth(decider as usize).ok_or_else(|| {
                            trap!("no function for index {} in br_table args", decider)
                        })??
                    };
                    if let Some(result) = WackasmExecutor::block_break(
                        &mut self.stack,
                        &function_frame.block_lookup,
                        &self.module.functions,
                        &mut pc,
                        target,
                    )? {
                        return Ok((result, pc));
                    }

                    // Skip PC increment
                    continue;
                }

                // Handle block/function return
                Operator::End => {
                    let section_idx = self
                        .stack
                        .peek_section(None)
                        .ok_or_else(|| trap!("end without any sections under it"))?;
                    match section_idx {
                        StackSection::Block(block_idx) => {
                            let block = function_frame
                                .block_lookup
                                .get(&block_idx)
                                .expect("we can't have exited from a block that doesn't exist");

                            let return_params = block.return_params(&self.module.functions)?;

                            self.stack
                                .return_section(&return_params.unwrap_or(&[]), None)?
                                .ok_or_else(|| anyhow!("no block to end from"))?;
                        }
                        StackSection::Function(_) => {
                            return Ok((FunctionScopedResult::FunctionReturn, pc));
                        }
                    }
                }
                Operator::Return => {
                    return Ok((FunctionScopedResult::FunctionReturn, pc));
                }

                // i32 maths
                Operator::I32Const { value } => {
                    self.stack.push(Value((*value) as u64))?;
                }
                Operator::I64Const { value } => {
                    self.stack.push(Value((*value) as u64))?;
                }
                Operator::F32Const { value } => {
                    self.stack.push(Value(value.bits() as u64))?;
                }
                Operator::F64Const { value } => {
                    self.stack.push(Value(value.bits()))?;
                }

                Operator::I32Add => {
                    i32_binop!(self.stack, i32, |lhs, rhs| lhs.wrapping_add(rhs))
                }
                Operator::I32Mul => {
                    i32_binop!(self.stack, i32, |lhs, rhs| lhs.wrapping_mul(rhs))
                }
                Operator::I32DivU => {
                    i32_binop!(self.stack, u32, |lhs, rhs| lhs
                        .checked_div(rhs)
                        .ok_or_else(|| trap!("integer divide by zero"))?)
                }
                Operator::I32Sub => {
                    i32_binop!(self.stack, i32, |lhs, rhs| lhs.wrapping_sub(rhs))
                }
                Operator::I32ShrU => {
                    i32_binop!(self.stack, u32, |lhs, rhs| lhs.wrapping_shr(rhs))
                }
                Operator::I32Shl => {
                    i32_binop!(self.stack, u32, |lhs, rhs| lhs.wrapping_shl(rhs))
                }
                Operator::I32And => {
                    i32_binop!(self.stack, i32, |lhs, rhs| lhs & rhs)
                }
                Operator::I32Xor => {
                    i32_binop!(self.stack, i32, |lhs, rhs| lhs ^ rhs)
                }

                // i32 comparators
                Operator::I32GtS => {
                    i32_binop!(self.stack, i32, |lhs, rhs| lhs > rhs)
                }
                Operator::I32LtU => {
                    i32_binop!(self.stack, u32, |lhs, rhs| lhs < rhs)
                }
                Operator::I32GtU => {
                    i32_binop!(self.stack, u32, |lhs, rhs| lhs > rhs)
                }
                Operator::I32LeU => {
                    i32_binop!(self.stack, u32, |lhs, rhs| lhs <= rhs)
                }
                Operator::I32Ne => {
                    i32_binop!(self.stack, i32, |a, b| a != b)
                }
                Operator::I32GeU => {
                    i32_binop!(self.stack, u32, |lhs, rhs| lhs >= rhs)
                }
                Operator::I32Eqz => {
                    let a = self.stack.pop_i32()?;
                    let result = if a == 0 { 1 } else { 0 };
                    self.stack.push(Value(result as u64))?;
                }
                Operator::I32Eq => {
                    i32_binop!(self.stack, i32, |a, b| a == b)
                }
                Operator::I32Or => {
                    i32_binop!(self.stack, i32, |a, b| a | b)
                }

                // i64 maths
                Operator::I64Add => {
                    i64_binop!(self.stack, i64, |lhs, rhs| lhs.wrapping_add(rhs))
                }
                Operator::I64Mul => {
                    i64_binop!(self.stack, i64, |lhs, rhs| lhs.wrapping_mul(rhs))
                }
                Operator::I64Sub => {
                    i64_binop!(self.stack, i64, |lhs, rhs| lhs.wrapping_sub(rhs))
                }
                Operator::I64ShrU => {
                    i64_binop!(self.stack, u64, |lhs, rhs| lhs.wrapping_shr(rhs as u32))
                }
                Operator::I64Shl => {
                    i64_binop!(self.stack, u64, |lhs, rhs| lhs.wrapping_shl(rhs as u32))
                }
                Operator::I64And => {
                    i64_binop!(self.stack, i64, |lhs, rhs| lhs & rhs)
                }
                Operator::I64Or => {
                    i64_binop!(self.stack, i64, |lhs, rhs| lhs | rhs)
                }

                // i64 comparators
                Operator::I64GtS => {
                    i64_cmp!(self.stack, i64, |lhs, rhs| lhs > rhs)
                }
                Operator::I64LtU => {
                    i64_cmp!(self.stack, u64, |lhs, rhs| lhs < rhs)
                }
                Operator::I64GtU => {
                    i64_cmp!(self.stack, u64, |lhs, rhs| lhs > rhs)
                }
                Operator::I64LeU => {
                    i64_cmp!(self.stack, u64, |lhs, rhs| lhs <= rhs)
                }
                Operator::I64GeU => {
                    i64_cmp!(self.stack, u64, |lhs, rhs| lhs >= rhs)
                }
                Operator::I64Eq => {
                    i64_cmp!(self.stack, i64, |lhs, rhs| lhs == rhs)
                }
                Operator::I64Ne => {
                    i64_cmp!(self.stack, i64, |lhs, rhs| lhs != rhs)
                }
                Operator::I64Eqz => {
                    let a = self.stack.pop_i64()?;
                    self.stack.push(Value((a == 0) as u64))?;
                }
                Operator::Select => {
                    let cond = self.stack.pop_i32()?;
                    let val2 = self.stack.pop()?;
                    let val1 = self.stack.pop()?;

                    if cond == 0 {
                        self.stack.push(val2)?;
                    } else {
                        self.stack.push(val1)?;
                    }
                }
                Operator::I64ExtendI32U => {
                    let value = self.stack.pop_i32()?;
                    let new_value = value as u32 as i64;
                    self.stack.push(Value(new_value as u64))?;
                }
                Operator::I32WrapI64 => {
                    let value = self.stack.pop_i64()?;
                    let value = value as i32;
                    self.stack.push(Value(value as u64))?;
                }
                _ => return Err(unsupported!("unsupported operator: {:?}", operator)),
            }

            pc += 1;
        }

        return Ok((FunctionScopedResult::OutOfFuel, pc));
    }
}
