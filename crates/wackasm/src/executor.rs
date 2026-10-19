use std::{
    collections::HashMap,
    ops::Div,
    sync::atomic::{AtomicIsize, Ordering},
};

use anyhow::{Ok, anyhow};
use serde::{Deserialize, Serialize};
use wasmparser::Operator;

use crate::{
    Value, WackasmModule,
    blocks::{Block, BlockManager},
    macros::{
        f32_load, f32_store, f64_load, f64_store, i32_binop, i32_load, i32_store, i64_binop,
        i64_cmp, i64_load, v128_load, v128_store,
    },
    module::{
        BlockIndex, ExportIndex, FunctionFrame, FunctionIndex, FunctionTypeIndex, GlobalIndex,
        TableIndex, TableValue,
    },
    pc::ProgramCounterHandle,
    stack::{BlockAwareOperandStack, OperandStackFns as _},
};

const MAX_INSTRUCTIONS_IN_LOOP: isize = 500;

#[derive(Debug, Serialize, Deserialize)]
pub struct WackasmExecutor {
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
    pub fn new(module: WackasmModule) -> Self {
        Self {
            fuel: AtomicIsize::new(isize::MAX),
            module,
            callstack: Vec::new(),
            output: None,
        }
    }

    pub fn call_export(&mut self, name: &str, arguments: Vec<Value>) -> Result<(), anyhow::Error> {
        let function = self
            .module
            .exports
            .get(name)
            .ok_or_else(|| anyhow!("function not found"))?;
        let function = match function {
            ExportIndex::Func(function_type_index) => function_type_index,
            _ => return Err(anyhow!("not a function")),
        };

        self.call_internal(*function, arguments)
    }

    fn call_internal(
        &mut self,
        function_index: FunctionIndex,
        arguments: Vec<Value>,
    ) -> Result<(), anyhow::Error> {
        let function = self
            .module
            .init_function_frames
            .get(&function_index)
            .ok_or_else(|| {
                anyhow!("internal module corruption: cannot lookup initial function state")
            })?;
        let mut function = function.clone();

        // Get function signature
        let function_type_index =
            self.module
                .function_types
                .get(&function_index)
                .ok_or_else(|| {
                    anyhow!(
                        "no function type index for function index {:?}",
                        function_index
                    )
                })?;
        let function_type = self
            .module
            .function_type_indexes
            .get(function_type_index)
            .ok_or_else(|| {
                anyhow!(
                    "no function type for function type index {:?}",
                    function_type_index
                )
            })?;

        let params = function_type.params();
        if arguments.len() != params.len() {
            return Err(anyhow!(
                "invalid amount of arguments, expected {}, got {}",
                params.len(),
                arguments.len()
            ));
        }

        // Insert arguments
        for (index, value) in arguments.into_iter().enumerate() {
            function.locals.set_checked(index as u32, value)?;
        }

        self.callstack.push(function);

        Ok(())
    }

    fn block_break(
        operand_stack: &mut BlockAwareOperandStack,
        block_lookup: &BlockManager,
        pc: &mut ProgramCounterHandle<'_>,
        relative_depth: u32,
    ) -> Result<(), anyhow::Error> {
        for _ in 0..relative_depth {
            operand_stack
                .pop_block_no_stack()
                .ok_or_else(|| anyhow!("no block to escape when br_if"))?;
        }

        // Based on top block, either jump back to start of loop or jump to end of block
        let last_block_idx = operand_stack
            .peek_block()
            .ok_or_else(|| anyhow!("no block for br_if"))?;
        let last_block = block_lookup.get(last_block_idx)?;

        match last_block {
            Block::Loop { start, .. } => {
                pc.set_pc(*start + 1);
            }
            Block::If { end, .. } | Block::Block { end, .. } => {
                pc.set_pc(*end);
            }
        }

        Ok(())
    }

    /// Executes up to [`instructions`] instructions within a single
    /// function frame. Adds how many were executed to instructions_executed.
    fn incremental_execute_function(
        &mut self,
        instructions: usize,
        instructions_executed: &mut isize,
    ) -> Result<FunctionScopedResult, anyhow::Error> {
        let function_frame = self
            .callstack
            .last_mut()
            .ok_or_else(|| anyhow!("callstack empty"))?;

        for _ in 0..instructions {
            *instructions_executed += 1;
            let mut pc = function_frame.pc.handle();
            let operator = function_frame.code.get(pc.inner()).unwrap();
            match operator {
                // Gets
                Operator::LocalGet { local_index } => {
                    let local = *function_frame.locals.get(&local_index)?;
                    function_frame.stack.push(local)?;
                }
                Operator::LocalSet { local_index } => {
                    let value = function_frame.stack.pop()?;
                    function_frame.locals.set_checked(*local_index, value)?;
                }
                Operator::LocalTee { local_index } => {
                    let value = function_frame.stack.peek()?;
                    function_frame
                        .locals
                        .set_checked(*local_index, value.clone())?;
                }
                Operator::GlobalGet { global_index } => {
                    let global = self.module.globals.get(&GlobalIndex(*global_index))?;
                    function_frame.stack.push(global.inner.clone())?;
                }
                Operator::GlobalSet { global_index } => {
                    let value = function_frame.stack.pop()?;
                    self.module
                        .globals
                        .set(&GlobalIndex(*global_index), value)?;
                }

                Operator::MemoryGrow { mem } => {
                    let amount = function_frame.stack.pop_i32()?;
                    let result = self.module.memories.grow(*mem, amount)?;
                    function_frame.stack.push(Value::I32(result))?;
                }
                Operator::MemoryFill { mem } => {
                    let amount = function_frame.stack.pop_i32()? as u32 as usize;
                    let value = function_frame.stack.pop_i32()? as u8;
                    let offset = function_frame.stack.pop_i32()? as u32 as usize;
                    self.module.memories.fill(*mem, offset, amount, value)?;
                }

                // memory load
                Operator::I32Load { memarg } => {
                    i32_load!(function_frame.stack, self.module.memories, memarg, i32)
                }
                Operator::I32Load8U { memarg } => {
                    i32_load!(function_frame.stack, self.module.memories, memarg, u8)
                }
                Operator::I32Load8S { memarg } => {
                    i32_load!(function_frame.stack, self.module.memories, memarg, i8)
                }
                Operator::I32Load16U { memarg } => {
                    i32_load!(function_frame.stack, self.module.memories, memarg, u16)
                }
                Operator::I32Load16S { memarg } => {
                    i32_load!(function_frame.stack, self.module.memories, memarg, i16)
                }

                Operator::I64Load { memarg } => {
                    i64_load!(function_frame.stack, self.module.memories, memarg, i64);
                }
                Operator::I64Load8U { memarg } => {
                    i64_load!(function_frame.stack, self.module.memories, memarg, u8)
                }
                Operator::I64Load8S { memarg } => {
                    i64_load!(function_frame.stack, self.module.memories, memarg, i8);
                }
                Operator::I64Load16U { memarg } => {
                    i64_load!(function_frame.stack, self.module.memories, memarg, u16);
                }
                Operator::I64Load16S { memarg } => {
                    i64_load!(function_frame.stack, self.module.memories, memarg, i16);
                }
                Operator::I64Load32U { memarg } => {
                    i64_load!(function_frame.stack, self.module.memories, memarg, u32);
                }
                Operator::I64Load32S { memarg } => {
                    i64_load!(function_frame.stack, self.module.memories, memarg, i32);
                }

                Operator::F32Load { memarg } => {
                    f32_load!(function_frame.stack, self.module.memories, memarg)
                }
                Operator::F64Load { memarg } => {
                    f64_load!(function_frame.stack, self.module.memories, memarg)
                }
                Operator::V128Load { memarg } => {
                    v128_load!(function_frame.stack, self.module.memories, memarg)
                }

                // memory store
                Operator::I32Store8 { memarg } => {
                    i32_store!(function_frame.stack, self.module.memories, memarg, u8)
                }
                Operator::I32Store { memarg } => {
                    i32_store!(function_frame.stack, self.module.memories, memarg, i32)
                }
                Operator::F32Store { memarg } => {
                    f32_store!(function_frame.stack, self.module.memories, memarg)
                }
                Operator::F64Store { memarg } => {
                    f64_store!(function_frame.stack, self.module.memories, memarg)
                }
                Operator::V128Store { memarg } => {
                    v128_store!(function_frame.stack, self.module.memories, memarg)
                }

                // Handle function call
                Operator::Call { function_index } => {
                    // Add one so we resume to the next instruction
                    return Ok(FunctionScopedResult::FunctionCall {
                        function_index: *function_index,
                    });
                }
                Operator::CallIndirect {
                    type_index: _,
                    table_index,
                } => {
                    let func_index_index = function_frame.stack.pop_i32()?;
                    let table = self
                        .module
                        .tables
                        .get(&TableIndex(*table_index))
                        .ok_or_else(|| anyhow!("no table for index {}", table_index))?;

                    let table_value = table
                        .get(func_index_index as u32 as usize)
                        .ok_or_else(|| anyhow!("no function index for {:?}", func_index_index))?;

                    let func_index = match table_value {
                        TableValue::FuncRef(items) => items,
                        _ => return Err(anyhow!("referenced table must be a funcref table")),
                    };

                    return Ok(FunctionScopedResult::FunctionCall {
                        function_index: func_index.0,
                    });
                }

                // Control flow
                Operator::If { blockty: _ } => {
                    let block_index = BlockIndex(pc.inner() as u32);
                    let block = function_frame.block_lookup.get(&block_index)?.clone();

                    let condition = function_frame.stack.pop_i32()?;
                    let condition = condition != 0;

                    let (start, else_block, end) = block.if_block()?;
                    let mut start_block = false;

                    if condition {
                        // Jump to next instruction
                        pc.set_pc(start + 1);
                        start_block = true;
                    } else {
                        if let Some(else_block) = else_block {
                            start_block = true;
                            // Jump after else instruction
                            pc.set_pc(else_block + 1);
                        } else {
                            // Jump after end
                            pc.set_pc(end + 1);
                        }
                    }

                    if start_block {
                        self.module
                            .start_block(&mut function_frame.stack, &block, &block_index)?;
                    }
                }

                // Jump to end block if we hit an else
                Operator::Else => {
                    let block_index = function_frame
                        .stack
                        .peek_block()
                        .ok_or_else(|| anyhow!("hit else op not in block"))?;
                    let block = function_frame.block_lookup.get(block_index)?;

                    let (_, _, end) = block.if_block()?;
                    // Jump to end block which will handle block frame cleanup
                    pc.set_pc(end);
                }

                Operator::Block { blockty: _ } | Operator::Loop { blockty: _ } => {
                    let block_index = BlockIndex(pc.inner() as u32);
                    let block = function_frame.block_lookup.get(&block_index)?.clone();

                    self.module
                        .start_block(&mut function_frame.stack, &block, &block_index)?;
                }

                Operator::BrIf { relative_depth } => {
                    let condition = function_frame.stack.pop_i32()?;
                    let branching = condition != 0;

                    if !branching {
                        continue;
                    }

                    WackasmExecutor::block_break(
                        &mut function_frame.stack,
                        &function_frame.block_lookup,
                        &mut pc,
                        *relative_depth,
                    )?;
                }

                Operator::Br { relative_depth } => {
                    WackasmExecutor::block_break(
                        &mut function_frame.stack,
                        &function_frame.block_lookup,
                        &mut pc,
                        *relative_depth,
                    )?;
                }

                // Handle block/function return
                Operator::End => {
                    // Block return
                    if let Some((block_idx, mut leftover)) = function_frame.stack.pop_block() {
                        let block = function_frame
                            .block_lookup
                            .get(&block_idx)
                            .expect("we can't have exited from a block that doesn't exist");
                        let return_params = match block.ty() {
                            wasmparser::BlockType::Empty => None,
                            wasmparser::BlockType::Type(val_type) => Some(vec![val_type.clone()]),
                            wasmparser::BlockType::FuncType(func_type) => {
                                let func_type_index = FunctionTypeIndex(*func_type); // This is right, don't change this.
                                let func_type =
                                    self.module.function_type_idx_type(&func_type_index)?;
                                Some(func_type.results().to_vec())
                            }
                        };
                        if let Some(return_params) = return_params {
                            let return_params =
                                leftover.pull_reversed_from_stack(&return_params)?;
                            function_frame.stack.push_reversed_all(return_params)?;
                        }

                        function_frame.stack.recycle_stack(leftover);
                    }
                    // Function return
                    else {
                        return Ok(FunctionScopedResult::FunctionReturn);
                    }
                }
                Operator::Return => {
                    return Ok(FunctionScopedResult::FunctionReturn);
                }

                // i32 maths
                Operator::I32Const { value } => {
                    function_frame.stack.push(Value::I32(*value))?;
                }
                Operator::I64Const { value } => {
                    function_frame.stack.push(Value::I64(*value))?;
                }
                Operator::F32Const { value } => {
                    function_frame.stack.push(Value::F32((*value).into()))?;
                }
                Operator::F64Const { value } => {
                    function_frame.stack.push(Value::F64((*value).into()))?;
                }

                Operator::I32Add => {
                    i32_binop!(function_frame.stack, i32, |lhs, rhs| lhs.wrapping_add(rhs))
                }
                Operator::I32Mul => {
                    i32_binop!(function_frame.stack, i32, |lhs, rhs| lhs.wrapping_mul(rhs))
                }
                Operator::I32DivU => {
                    i32_binop!(function_frame.stack, u32, |lhs, rhs| lhs
                        .checked_div(rhs)
                        .ok_or_else(
                        || anyhow!("divide by 0")
                    )?)
                }
                Operator::I32Sub => {
                    i32_binop!(function_frame.stack, i32, |lhs, rhs| lhs.wrapping_sub(rhs))
                }
                Operator::I32ShrU => {
                    i32_binop!(function_frame.stack, u32, |lhs, rhs| lhs.wrapping_shr(rhs))
                }
                Operator::I32Shl => {
                    i32_binop!(function_frame.stack, u32, |lhs, rhs| lhs.wrapping_shl(rhs))
                }
                Operator::I32And => {
                    i32_binop!(function_frame.stack, i32, |lhs, rhs| lhs & rhs)
                }

                // i32 comparators
                Operator::I32GtS => {
                    i32_binop!(function_frame.stack, i32, |lhs, rhs| lhs > rhs)
                }
                Operator::I32LtU => {
                    i32_binop!(function_frame.stack, u32, |lhs, rhs| lhs < rhs)
                }
                Operator::I32GtU => {
                    i32_binop!(function_frame.stack, u32, |lhs, rhs| lhs > rhs)
                }
                Operator::I32LeU => {
                    i32_binop!(function_frame.stack, u32, |lhs, rhs| lhs <= rhs)
                }
                Operator::I32Ne => {
                    i32_binop!(function_frame.stack, i32, |a, b| a != b)
                }
                Operator::I32GeU => {
                    i32_binop!(function_frame.stack, u32, |lhs, rhs| lhs >= rhs)
                }
                Operator::I32Eqz => {
                    let a = function_frame.stack.pop_i32()?;
                    let result = if a == 0 { 1 } else { 0 };
                    function_frame.stack.push(Value::I32(result))?;
                }
                Operator::I32Eq => {
                    i32_binop!(function_frame.stack, i32, |a, b| a == b)
                }
                Operator::I32Or => {
                    i32_binop!(function_frame.stack, i32, |a, b| a | b)
                }

                // i64 maths
                Operator::I64Add => {
                    i64_binop!(function_frame.stack, i64, |lhs, rhs| lhs.wrapping_add(rhs))
                }
                Operator::I64Mul => {
                    i64_binop!(function_frame.stack, i64, |lhs, rhs| lhs.wrapping_mul(rhs))
                }
                Operator::I64Sub => {
                    i64_binop!(function_frame.stack, i64, |lhs, rhs| lhs.wrapping_sub(rhs))
                }
                Operator::I64ShrU => {
                    i64_binop!(function_frame.stack, u64, |lhs, rhs| lhs
                        .wrapping_shr(rhs as u32))
                }
                Operator::I64Shl => {
                    i64_binop!(function_frame.stack, u64, |lhs, rhs| lhs
                        .wrapping_shl(rhs as u32))
                }
                Operator::I64And => {
                    i64_binop!(function_frame.stack, i64, |lhs, rhs| lhs & rhs)
                }
                Operator::I64Or => {
                    i64_binop!(function_frame.stack, i64, |lhs, rhs| lhs | rhs)
                }

                // i64 comparators
                Operator::I64GtS => {
                    i64_cmp!(function_frame.stack, i64, |lhs, rhs| lhs > rhs)
                }
                Operator::I64LtU => {
                    i64_cmp!(function_frame.stack, u64, |lhs, rhs| lhs < rhs)
                }
                Operator::I64GtU => {
                    i64_cmp!(function_frame.stack, u64, |lhs, rhs| lhs > rhs)
                }
                Operator::I64LeU => {
                    i64_cmp!(function_frame.stack, u64, |lhs, rhs| lhs <= rhs)
                }
                Operator::I64GeU => {
                    i64_cmp!(function_frame.stack, u64, |lhs, rhs| lhs >= rhs)
                }
                Operator::I64Eq => {
                    i64_cmp!(function_frame.stack, i64, |lhs, rhs| lhs == rhs)
                }
                Operator::I64Ne => {
                    i64_cmp!(function_frame.stack, i64, |lhs, rhs| lhs != rhs)
                }
                Operator::I64Eqz => {
                    let a = function_frame.stack.pop_i64()?;
                    function_frame.stack.push(Value::I32((a == 0) as i32))?;
                }
                Operator::Select => {
                    let cond = function_frame.stack.pop_i32()?;
                    let val2 = function_frame.stack.pop()?;
                    let val1 = function_frame.stack.pop()?;

                    if cond == 0 {
                        function_frame.stack.push(val2)?;
                    } else {
                        function_frame.stack.push(val1)?;
                    }
                }
                Operator::I64ExtendI32U => {
                    let value = function_frame.stack.pop_i32()?;
                    let new_value = value as u32 as i64;
                    function_frame.stack.push(Value::I64(new_value))?;
                }
                Operator::I32WrapI64 => {
                    let value = function_frame.stack.pop_i64()?;
                    let value = value as i32;
                    function_frame.stack.push(Value::I32(value))?;
                }
                _ => return Err(anyhow!("unsupported operator: {:?}", operator)),
            }
        }

        return Ok(FunctionScopedResult::OutOfFuel);
    }

    // runs X instructions, decrementing fuel
    #[inline]
    fn incremental_execute(&mut self, instructions: usize) -> Result<isize, anyhow::Error> {
        let mut instructions_executed = 0;

        let function_result =
            self.incremental_execute_function(instructions, &mut instructions_executed)?;

        match function_result {
            FunctionScopedResult::OutOfFuel => {} // nothing, just continue
            FunctionScopedResult::FunctionReturn => {
                // Now we get to pop the function frame and push the results to
                let mut function_frame = self
                    .callstack
                    .pop()
                    .expect("incremental_execute_function would have errored by now");

                let function_type = self
                    .module
                    .function_idx_type(&function_frame.function_index)?;

                let result_types = function_type.results();
                let results = function_frame
                    .stack
                    .pull_reversed_from_stack(result_types)?;

                if let Some(mut next_frame) = self.callstack.peek_mut() {
                    next_frame.stack.push_reversed_all(results)?;
                } else {
                    let mut results = results;
                    results.reverse();
                    self.output = Some(results);
                }
            }
            FunctionScopedResult::FunctionCall { function_index } => {
                let function_index = FunctionIndex(function_index);

                let mut new_frame = self
                    .module
                    .init_function_frames
                    .get(&function_index)
                    .ok_or_else(|| anyhow!("no function for {:?}", function_index))?
                    .clone();

                let function_type = self.module.function_idx_type(&function_index)?.clone();
                let mut calling_operands = {
                    let mut calling_frame = self.callstack.peek_mut().expect("no calling frame?");
                    let calling_operands_types = function_type.params();
                    calling_frame
                        .stack
                        .pull_reversed_from_stack(calling_operands_types)?
                };
                calling_operands.reverse();

                for (index, value) in calling_operands.into_iter().enumerate() {
                    new_frame.locals.set_checked(index as u32, value)?;
                }

                self.callstack.push(new_frame);
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

    pub fn execute(&mut self) -> Result<(), anyhow::Error> {
        while self.callstack.len() > 0 {
            let remaining_fuel = self.fuel.load(Ordering::Relaxed);
            let target_fuel = remaining_fuel.min(MAX_INSTRUCTIONS_IN_LOOP);
            if target_fuel == 0 {
                break;
            }
            let executed = self.incremental_execute(target_fuel.try_into()?)?;
            self.fuel.fetch_sub(executed, Ordering::Relaxed);
        }
        Ok(())
    }

    pub fn get_result(&self) -> Option<&Vec<Value>> {
        self.output.as_ref()
    }
}
