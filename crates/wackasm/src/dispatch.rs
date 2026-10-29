use std::{convert::Infallible, hint::unreachable_unchecked, ops::FromResidual};

use anyhow::anyhow;

use crate::{
    HaltError, Value, compiler::{RuntimeWackasmOperator, WackasmFunctionBody, WackasmOperator, WackasmTableTarget}, functions::FunctionManager, globals::GlobalManager, macros::{
        f32_load, f32_store, f64_load, f64_store, i32_binop, i32_load, i32_store, i64_binop,
        i64_cmp, i64_load, i64_store, trap, unsupported, v128_load, v128_store,
    }, memory::MemoryManager, module::{FunctionTypeIndex, GlobalIndex, TableIndex, TableValue}, ops, stack::{LocalsHandle, RuntimeStack, StackHandle}, table::TableManager,
};

pub(crate) struct DispatchCtx<'a> {
    pub(crate) memories: &'a mut MemoryManager,
    pub(crate) globals: &'a mut GlobalManager,
    pub(crate) tables: &'a TableManager,
    pub(crate) functions: &'a FunctionManager, // CallIndirect type check
    pub(crate) code: &'a [RuntimeWackasmOperator],
    pub(crate) fuel: isize,

    // We pass the exit value back through here, so the entirety of
    // dispatch and tail calls can stay in registers.
    pub(crate) exit: Option<Exit<'a>>
}

impl DispatchCtx<'_> {
    #[inline(always)]
    fn at(&self, index: u32) -> *const RuntimeWackasmOperator {
        // SAFETY: the compiler only emits jump targets that are inside this function's code
        unsafe { self.code.as_ptr().add(index as usize) }
    }
}

pub(crate) type WackasmOperatorHandler = for<'a, 'b> fn(
    &'b mut DispatchCtx<'a>,
    *const RuntimeWackasmOperator,
    StackHandle<'a>,
    LocalsHandle<'a>,
) -> Result<(), HaltError>;

pub(crate) enum Exit<'a> {
    OutOfFuel {
        pc: *const RuntimeWackasmOperator,
        stack: StackHandle<'a>,
    },
    Call {
        pc: *const RuntimeWackasmOperator,
        stack: StackHandle<'a>,
        function_index: u32,
    },
    Return {
        pc: *const RuntimeWackasmOperator,
        stack: StackHandle<'a>,
    },
}

ops!(|ctx, pc, stack, locals| match op {
    nop @ Nop => {}

    fuel @ ConsumeFuel => {
        if ctx.fuel == 0 {
            ctx.exit = Some(Exit::OutOfFuel { pc, stack, });
            return Ok(())
        }
        ctx.fuel -= 1;
    }

    local_get @ LocalGet { slot } => {
        stack.push(locals.get(*slot));
    }
    local_get_wide @ LocalGetWide { slot } => {
        stack.push_v128(locals.get_wide(*slot));
    }
    local_set @ LocalSet { slot } => {
        locals.set(*slot, stack.pop());
    }
    local_set_wide @ LocalSetWide { slot } => {
        locals.set_wide(*slot, stack.pop_v128());
    }

    drop @ Drop => {
        stack.pop();
    }
    drop_wide @ DropWide => {
        stack.pop_v128();
    }

    local_tee @ LocalTee { slot } => {
        let value = *stack.peek();
        locals.set(*slot, value);
    }
    local_tee_wide @ LocalTeeWide { slot } => {
        let value = stack.peek_wide();
        locals.set_wide(*slot, value);
    }

    global_get @ GlobalGet { global_index } => {
        let global = ctx.globals.get(&GlobalIndex(*global_index))?;
        stack.push(*global.inner.get());
    }
    global_get_wide @ GlobalGetWide { global_index } => {
        let global = ctx.globals.get_wide(&GlobalIndex(*global_index))?;
        stack.push_v128(global);
    }

    global_set @ GlobalSet { global_index } => {
        let value = stack.pop();
        ctx.globals.set(&GlobalIndex(*global_index), value)?;
    }
    global_set_wide @ GlobalSetWide { global_index } => {
        let value = stack.pop_v128();
        ctx.globals.set_wide(&GlobalIndex(*global_index), value)?;
    }

    memory_grow @ MemoryGrow { mem } => {
        let amount = stack.pop_i32();
        let result = ctx.memories.grow(*mem, amount)?;
        stack.push(Value(result as u64));
    }
    memory_fill @ MemoryFill { mem } => {
        let amount = stack.pop_i32() as u32 as usize;
        let value = stack.pop_i32() as u8;
        let offset = stack.pop_i32() as u32 as usize;
        ctx.memories.fill(*mem, offset, amount, value)?;
    }

    // memory load
    i32_load @ I32Load { memarg } => {
        i32_load!(stack, ctx.memories, memarg, i32)
    }
    i32_load8_u @ I32Load8U { memarg } => {
        i32_load!(stack, ctx.memories, memarg, u8)
    }
    i32_load8_s @ I32Load8S { memarg } => {
        i32_load!(stack, ctx.memories, memarg, i8)
    }
    i32_load16_u @ I32Load16U { memarg } => {
        i32_load!(stack, ctx.memories, memarg, u16)
    }
    i32_load16_s @ I32Load16S { memarg } => {
        i32_load!(stack, ctx.memories, memarg, i16)
    }

    i64_load @ I64Load { memarg } => {
        i64_load!(stack, ctx.memories, memarg, i64)
    }
    i64_load8_u @ I64Load8U { memarg } => {
        i64_load!(stack, ctx.memories, memarg, u8)
    }
    i64_load8_s @ I64Load8S { memarg } => {
        i64_load!(stack, ctx.memories, memarg, i8)
    }
    i64_load16_u @ I64Load16U { memarg } => {
        i64_load!(stack, ctx.memories, memarg, u16)
    }
    i64_load16_s @ I64Load16S { memarg } => {
        i64_load!(stack, ctx.memories, memarg, i16)
    }
    i64_load32_u @ I64Load32U { memarg } => {
        i64_load!(stack, ctx.memories, memarg, u32)
    }
    i64_load32_s @ I64Load32S { memarg } => {
        i64_load!(stack, ctx.memories, memarg, i32)
    }

    f32_load @ F32Load { memarg } => {
        f32_load!(stack, ctx.memories, memarg)
    }
    f64_load @ F64Load { memarg } => {
        f64_load!(stack, ctx.memories, memarg)
    }
    v128_load @ V128Load { memarg } => {
        v128_load!(stack, ctx.memories, memarg)
    }

    // memory store
    i32_store @ I32Store { memarg } => {
        i32_store!(stack, ctx.memories, memarg, i32)
    }
    i32_store8 @ I32Store8 { memarg } => {
        i32_store!(stack, ctx.memories, memarg, u8)
    }
    i32_store16 @ I32Store16 { memarg } => {
        i32_store!(stack, ctx.memories, memarg, u16)
    }

    i64_store @ I64Store { memarg } => {
        i64_store!(stack, ctx.memories, memarg, i64)
    }
    i64_store8 @ I64Store8 { memarg } => {
        i64_store!(stack, ctx.memories, memarg, u8)
    }
    i64_store16 @ I64Store16 { memarg } => {
        i64_store!(stack, ctx.memories, memarg, u16)
    }
    i64_store32 @ I64Store32 { memarg } => {
        i64_store!(stack, ctx.memories, memarg, u32)
    }

    f32_store @ F32Store { memarg } => {
        f32_store!(stack, ctx.memories, memarg)
    }
    f64_store @ F64Store { memarg } => {
        f64_store!(stack, ctx.memories, memarg)
    }
    v128_store @ V128Store { memarg } => {
        v128_store!(stack, ctx.memories, memarg)
    }

    call @ Call { function_index } => {
        ctx.exit = Some(Exit::Call {
            pc: unsafe { pc.add(1) },
            stack,
            function_index: *function_index,
        });
        return Ok(())
    }

    call_indirect @ CallIndirect {
        table_index,
        type_index,
    } => {
        let func_index_index = stack.pop_i32();
        let table = ctx
            .tables
            .get(&TableIndex(*table_index))
            .ok_or_else(|| anyhow!("no table for index {}", table_index))?;

        let table_value = table
            .get(func_index_index as u32 as usize)
            .ok_or_else(|| trap!("undefined element {}", func_index_index as u32))?;

        let func_index = match table_value {
            TableValue::FuncRef(items) => items,
            TableValue::NullRef => Err(trap!("uninitialized element"))?,
        };

        // Type check, per spec
        let expected = ctx
            .functions
            .function_type_idx_type(&FunctionTypeIndex(*type_index))?;
        let actual = ctx.functions.function_type_idx(&func_index)?;
        if expected != actual {
            Err(trap!("indirect call type mismatch"))?;
        }

        ctx.exit = Some(Exit::Call {
            pc: unsafe { pc.add(1) },
            stack,
            function_index: func_index.0,
        });
        return Ok(())
    }

    jump @ Jump { target } => {
        let target = ctx.at(*target);
        dispatch!(ctx, target, stack, locals)
    }

    jump_if @ JumpIf { target } => {
        if stack.pop_i32() != 0 {
            let target = ctx.at(*target);
            dispatch!(ctx, target, stack, locals)
        }
    }

    jump_if_eqz @ JumpIfEqz { target } => {
        if stack.pop_i32() == 0 {
            let target = ctx.at(*target);
            dispatch!(ctx, target, stack, locals)
        }
    }

    br @ Br { target, drop_keep } => {
        stack.drop_keep(*drop_keep);
        let target = ctx.at(*target);
        dispatch!(ctx, target, stack, locals)
    }
    br_if @ BrIf { target, drop_keep } => {
        if stack.pop_i32() != 0 {
            stack.drop_keep(*drop_keep);
            let target = ctx.at(*target);
            dispatch!(ctx, target, stack, locals)
        }
    }
    br_table @ BrTable { targets } => {
        let i = stack.pop_i32() as u32 as usize;
        match targets[i.min(targets.len() - 1)] {
            WackasmTableTarget::Return => {
                ctx.exit = Some(Exit::Return {
                    pc,
                    stack,
                });
                return Ok(())
            }
            WackasmTableTarget::Break { target, dk } => {
                stack.drop_keep(dk);
                let target = ctx.at(target);
                dispatch!(ctx, target, stack, locals)
            }
        }
    }

    ret @ Return => {
        ctx.exit = Some(Exit::Return {
            pc,
            stack,
        });
        return Ok(())
    }
    ret_if @ ReturnIf => {
        if stack.pop_i32() != 0 {
            ctx.exit = Some(Exit::Return {
                pc,
                stack,
            });
            return Ok(())
        }
    }

    // constants
    i32_const @ I32Const { value } => {
        stack.push(Value(*value as u64));
    }
    i64_const @ I64Const { value } => {
        stack.push(Value(*value as u64));
    }
    f32_const @ F32Const { value } => {
        stack.push(Value(value.to_bits() as u64));
    }
    f64_const @ F64Const { value } => {
        stack.push(Value(value.to_bits()));
    }

    // i32 maths
    i32_add @ I32Add => {
        i32_binop!(stack, i32, |lhs, rhs| lhs.wrapping_add(rhs))
    }
    i32_mul @ I32Mul => {
        i32_binop!(stack, i32, |lhs, rhs| lhs.wrapping_mul(rhs))
    }
    i32_div_u @ I32DivU => {
        i32_binop!(stack, u32, |lhs, rhs| lhs
            .checked_div(rhs)
            .ok_or_else(|| trap!("integer divide by zero"))?)
    }
    i32_sub @ I32Sub => {
        i32_binop!(stack, i32, |lhs, rhs| lhs.wrapping_sub(rhs))
    }
    i32_shr_u @ I32ShrU => {
        i32_binop!(stack, u32, |lhs, rhs| lhs.wrapping_shr(rhs))
    }
    i32_shl @ I32Shl => {
        i32_binop!(stack, u32, |lhs, rhs| lhs.wrapping_shl(rhs))
    }
    i32_and @ I32And => {
        i32_binop!(stack, i32, |lhs, rhs| lhs & rhs)
    }
    i32_xor @ I32Xor => {
        i32_binop!(stack, i32, |lhs, rhs| lhs ^ rhs)
    }
    i32_or @ I32Or => {
        i32_binop!(stack, i32, |lhs, rhs| lhs | rhs)
    }

    // i32 comparators
    i32_gt_s @ I32GtS => {
        i32_binop!(stack, i32, |lhs, rhs| lhs > rhs)
    }
    i32_lt_s @ I32LtS => {
        i32_binop!(stack, i32, |lhs, rhs| lhs < rhs)
    }
    i32_lt_u @ I32LtU => {
        i32_binop!(stack, u32, |lhs, rhs| lhs < rhs)
    }
    i32_gt_u @ I32GtU => {
        i32_binop!(stack, u32, |lhs, rhs| lhs > rhs)
    }
    i32_le_u @ I32LeU => {
        i32_binop!(stack, u32, |lhs, rhs| lhs <= rhs)
    }
    i32_ge_u @ I32GeU => {
        i32_binop!(stack, u32, |lhs, rhs| lhs >= rhs)
    }
    i32_ne @ I32Ne => {
        i32_binop!(stack, i32, |lhs, rhs| lhs != rhs)
    }
    i32_eq @ I32Eq => {
        i32_binop!(stack, i32, |lhs, rhs| lhs == rhs)
    }
    i32_eqz @ I32Eqz => {
        let a = stack.pop_i32();
        stack.push(Value((a == 0) as u64));
    }

    // i64 maths
    i64_add @ I64Add => {
        i64_binop!(stack, i64, |lhs, rhs| lhs.wrapping_add(rhs))
    }
    i64_mul @ I64Mul => {
        i64_binop!(stack, i64, |lhs, rhs| lhs.wrapping_mul(rhs))
    }
    i64_sub @ I64Sub => {
        i64_binop!(stack, i64, |lhs, rhs| lhs.wrapping_sub(rhs))
    }
    i64_shr_u @ I64ShrU => {
        i64_binop!(stack, u64, |lhs, rhs| lhs.wrapping_shr(rhs as u32))
    }
    i64_shl @ I64Shl => {
        i64_binop!(stack, u64, |lhs, rhs| lhs.wrapping_shl(rhs as u32))
    }
    i64_and @ I64And => {
        i64_binop!(stack, i64, |lhs, rhs| lhs & rhs)
    }
    i64_or @ I64Or => {
        i64_binop!(stack, i64, |lhs, rhs| lhs | rhs)
    }

    // i64 comparators
    i64_gt_s @ I64GtS => {
        i64_cmp!(stack, i64, |lhs, rhs| lhs > rhs)
    }
    i64_lt_u @ I64LtU => {
        i64_cmp!(stack, u64, |lhs, rhs| lhs < rhs)
    }
    i64_gt_u @ I64GtU => {
        i64_cmp!(stack, u64, |lhs, rhs| lhs > rhs)
    }
    i64_le_u @ I64LeU => {
        i64_cmp!(stack, u64, |lhs, rhs| lhs <= rhs)
    }
    i64_ge_u @ I64GeU => {
        i64_cmp!(stack, u64, |lhs, rhs| lhs >= rhs)
    }
    i64_eq @ I64Eq => {
        i64_cmp!(stack, i64, |lhs, rhs| lhs == rhs)
    }
    i64_ne @ I64Ne => {
        i64_cmp!(stack, i64, |lhs, rhs| lhs != rhs)
    }
    i64_eqz @ I64Eqz => {
        let a = stack.pop_i64();
        stack.push(Value((a == 0) as u64));
    }

    // misc
    select @ Select { wide } => {
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
    i64_extend_i32_u @ I64ExtendI32U => {
        let value = stack.pop_i32();
        stack.push(Value(value as u32 as u64));
    }
    i32_wrap_i64 @ I32WrapI64 => {
        let value = stack.pop_i64();
        stack.push(Value(value as i32 as u64));
    }

    unreachable @ Unreachable => {
        Err(trap!("unreachable"))?;
    }
    unsupported @ Unsupported { description } => {
        Err(unsupported!("unsupported operator: {}", description))?;
    }
});

pub fn dispatch<'a>(
    ctx: &mut DispatchCtx<'a>,
    pc: *const RuntimeWackasmOperator,
    stack: StackHandle<'a>,
    locals: LocalsHandle<'a>,
) -> Result<(), HaltError> {
    dispatch!(ctx, pc, stack, locals);
}
