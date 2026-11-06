use crate::{
    HaltError, Value,
    compiler::{I32Comparison, Reg, RuntimeWackasmOperator, WackasmOperator},
    functions::{FunctionManager, MAX_CALL_DEPTH},
    globals::GlobalManager,
    macros::{trap, unsupported},
    memory::MemoryManager,
    module::{
        FunctionFrame, FunctionIndex, FunctionTypeIndex, GlobalIndex, TableIndex, TableValue,
    },
    ops,
    table::TableManager,
};

pub(crate) struct DispatchCtx<'a> {
    pub(crate) memories: &'a mut MemoryManager,
    pub(crate) globals: &'a mut GlobalManager,
    pub(crate) tables: &'a TableManager,
    pub(crate) functions: &'a FunctionManager,
    pub(crate) frames: &'a mut Vec<FunctionFrame>,
    pub(crate) buffer: *mut Value,
    pub(crate) capacity: usize,
    pub(crate) fuel: isize,
    pub(crate) exit: Option<Exit>,
}

pub(crate) type WackasmOperatorHandler =
    for<'a> fn(&mut DispatchCtx<'a>, *const RuntimeWackasmOperator, *mut Value);

pub(crate) enum Exit {
    OutOfFuel,
    ImportCall { function_index: u32, base: usize },
    Return { base: usize, len: usize },
    Trap(HaltError),
}

#[cold]
#[inline(never)]
fn halt(ctx: &mut DispatchCtx<'_>, error: HaltError) {
    ctx.exit = Some(Exit::Trap(error));
}

#[cold]
#[inline(never)]
fn trap_exit(ctx: &mut DispatchCtx<'_>, message: &'static str) {
    halt(ctx, trap!("{}", message));
}

#[cold]
#[inline(never)]
fn unsupported_exit(ctx: &mut DispatchCtx<'_>, description: &str) {
    halt(ctx, unsupported!("unsupported operator: {}", description));
}

#[inline(always)]
fn save_pc(ctx: &mut DispatchCtx<'_>, pc: *const RuntimeWackasmOperator) {
    let frame = ctx.frames.last_mut().expect("dispatch requires a frame");
    let code = ctx.functions.info[frame.function_index.0 as usize].code;
    frame.pc = unsafe { pc.offset_from_unsigned(code) };
}

#[inline(always)]
fn take_fuel(ctx: &mut DispatchCtx<'_>, pc: *const RuntimeWackasmOperator) -> bool {
    if ctx.fuel <= 0 {
        save_pc(ctx, pc);
        ctx.exit = Some(Exit::OutOfFuel);
        false
    } else {
        ctx.fuel -= 1;
        true
    }
}

type DispatchTarget = (*const RuntimeWackasmOperator, *mut Value);

#[inline(always)]
fn call_target(
    ctx: &mut DispatchCtx<'_>,
    pc: *const RuntimeWackasmOperator,
    function_index: u32,
    base: Reg,
) -> Option<DispatchTarget> {
    if ctx.frames.len() >= MAX_CALL_DEPTH {
        trap_exit(ctx, "call stack exhausted");
        return None;
    }
    let Some(info) = ctx.functions.info.get(function_index as usize) else {
        trap_exit(ctx, "invalid function index");
        return None;
    };
    let caller_base = ctx
        .frames
        .last()
        .expect("dispatch requires a frame")
        .stack_base;
    let stack_base = caller_base + base as usize;
    if info.frame_size > ctx.capacity.saturating_sub(stack_base) || stack_base > ctx.capacity {
        trap_exit(ctx, "potential stack overflow");
        return None;
    }
    let code = info.code;
    let params_slots = info.params_slots;
    let local_defaults = info.local_defaults;
    let locals_len = info.locals_len;
    save_pc(ctx, unsafe { pc.add(1) });
    if code.is_null() {
        ctx.exit = Some(Exit::ImportCall {
            function_index,
            base: stack_base,
        });
        return None;
    }
    let regs = unsafe { ctx.buffer.add(stack_base) };
    unsafe {
        std::ptr::copy_nonoverlapping(local_defaults, regs.add(params_slots), locals_len);
    }
    ctx.frames.push(FunctionFrame {
        function_index: FunctionIndex(function_index),
        pc: 0,
        stack_base,
    });
    Some((code, regs))
}

#[inline(always)]
fn return_target(
    ctx: &mut DispatchCtx<'_>,
    regs: *mut Value,
    src: Reg,
    len: Reg,
) -> Option<DispatchTarget> {
    let function_index = ctx
        .frames
        .last()
        .expect("dispatch requires a frame")
        .function_index;
    debug_assert_eq!(
        len as usize,
        ctx.functions.info[function_index.0 as usize].results_slots
    );
    unsafe { std::ptr::copy(regs.add(src as usize), regs, len as usize) };
    let frame = ctx.frames.pop().expect("dispatch requires a frame");
    if let Some(caller) = ctx.frames.last() {
        let info = &ctx.functions.info[caller.function_index.0 as usize];
        Some(unsafe { (info.code.add(caller.pc), ctx.buffer.add(caller.stack_base)) })
    } else {
        ctx.exit = Some(Exit::Return {
            base: frame.stack_base,
            len: len as usize,
        });
        None
    }
}

#[inline(always)]
fn compare_i32(kind: I32Comparison, lhs: i32, rhs: i32) -> bool {
    match kind {
        I32Comparison::Eq => lhs == rhs,
        I32Comparison::Ne => lhs != rhs,
        I32Comparison::LtS => lhs < rhs,
        I32Comparison::LtU => (lhs as u32) < rhs as u32,
        I32Comparison::GtS => lhs > rhs,
        I32Comparison::GtU => (lhs as u32) > rhs as u32,
        I32Comparison::GeS => lhs >= rhs,
        I32Comparison::GeU => (lhs as u32) >= rhs as u32,
        I32Comparison::LeU => (lhs as u32) <= rhs as u32,
    }
}

macro_rules! get {
    ($regs:expr, $reg:expr) => {
        unsafe { *$regs.add(*$reg as usize) }
    };
}
macro_rules! set {
    ($regs:expr, $reg:expr, $value:expr) => {{
        let value = $value;
        unsafe { $regs.add(*$reg as usize).write(value) };
    }};
}
macro_rules! checked {
    ($ctx:expr, $result:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => {
                halt($ctx, error.into());
                return;
            }
        }
    };
}
macro_rules! bin {
    ($regs:expr, $dst:expr, $lhs:expr, $rhs:expr, $ty:ty, |$a:ident, $b:ident| $value:expr) => {{
        let $a = get!($regs, $lhs).0 as $ty;
        let $b = get!($regs, $rhs).0 as $ty;
        set!($regs, $dst, Value(($value) as u64));
    }};
}
macro_rules! imm {
    ($regs:expr, $dst:expr, $lhs:expr, $rhs:expr, $ty:ty, |$a:ident, $b:ident| $value:expr) => {{
        let $a = get!($regs, $lhs).0 as $ty;
        let $b = *$rhs as $ty;
        set!($regs, $dst, Value(($value) as u64));
    }};
}
macro_rules! load {
    ($ctx:expr, $regs:expr, $dst:expr, $addr:expr, $memarg:expr, $ty:ty) => {{
        let value = checked!(
            $ctx,
            $ctx.memories
                .load::<$ty>($memarg, get!($regs, $addr).as_u32())
        );
        set!($regs, $dst, Value(value as u64));
    }};
}
macro_rules! store {
    ($ctx:expr, $regs:expr, $addr:expr, $value:expr, $memarg:expr, $ty:ty) => {{
        checked!(
            $ctx,
            $ctx.memories.store(
                $memarg,
                get!($regs, $addr).as_u32(),
                get!($regs, $value).0 as $ty,
            )
        );
    }};
}

ops!(|ctx, pc, regs| match op {
    nop @ Nop => {}
    fuel @ ConsumeFuel => {
        if !take_fuel(ctx, pc) {
            return;
        }
    }
    copy @ Copy { dst, src } => {
        set!(regs, dst, get!(regs, src));
    }
    copy_wide @ CopyWide { dst, src } => {
        unsafe { std::ptr::copy(regs.add(*src as usize), regs.add(*dst as usize), 2) };
    }
    copy_span @ CopySpan { dst, src, len } => {
        unsafe {
            std::ptr::copy(
                regs.add(*src as usize),
                regs.add(*dst as usize),
                *len as usize,
            )
        };
    }
    copy_imm32 @ CopyImm32 { dst, value } => {
        set!(regs, dst, Value(*value as u64));
    }
    copy_imm64 @ CopyImm64 { dst, value } => {
        set!(
            regs,
            dst,
            Value(value[0] as u64 | ((value[1] as u64) << 32))
        );
    }
    global_get @ GlobalGet { dst, global_index } => {
        set!(
            regs,
            dst,
            *ctx.globals.get(&GlobalIndex(*global_index)).inner.get()
        );
    }
    global_get_wide @ GlobalGetWide { dst, global_index } => {
        let value = checked!(ctx, ctx.globals.get_wide(&GlobalIndex(*global_index)));
        unsafe {
            regs.add(*dst as usize).write(Value(value as u64));
            regs.add(*dst as usize + 1)
                .write(Value((value >> 64) as u64));
        }
    }
    global_set @ GlobalSet { src, global_index } => {
        ctx.globals
            .set(&GlobalIndex(*global_index), get!(regs, src));
    }
    global_set_wide @ GlobalSetWide { src, global_index } => {
        let value = unsafe {
            (*regs.add(*src as usize)).0 as u128
                | (((*regs.add(*src as usize + 1)).0 as u128) << 64)
        };
        checked!(
            ctx,
            ctx.globals.set_wide(&GlobalIndex(*global_index), value)
        );
    }
    memory_grow @ MemoryGrow { dst, amount, mem } => {
        let result = checked!(ctx, ctx.memories.grow(*mem, get!(regs, amount).as_i32()));
        set!(regs, dst, Value::from_i32(result));
    }
    memory_copy @ MemoryCopy {
        dst_mem,
        src_mem,
        dst,
        src,
        len,
    } => {
        checked!(
            ctx,
            ctx.memories.memcpy(
                *src_mem as u32,
                *dst_mem as u32,
                get!(regs, dst).as_u32() as usize,
                get!(regs, src).as_u32() as usize,
                get!(regs, len).as_u32() as usize,
            )
        );
    }
    memory_fill @ MemoryFill {
        mem,
        addr,
        value,
        len,
    } => {
        checked!(
            ctx,
            ctx.memories.fill(
                *mem,
                get!(regs, addr).as_u32() as usize,
                get!(regs, len).as_u32() as usize,
                get!(regs, value).0 as u8,
            )
        );
    }
    i32_load @ I32Load { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, i32);
    }
    i32_load8_u @ I32Load8U { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, u8);
    }
    i32_load8_s @ I32Load8S { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, i8);
    }
    i32_load16_u @ I32Load16U { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, u16);
    }
    i32_load16_s @ I32Load16S { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, i16);
    }
    i64_load @ I64Load { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, i64);
    }
    i64_load8_u @ I64Load8U { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, u8);
    }
    i64_load8_s @ I64Load8S { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, i8);
    }
    i64_load16_u @ I64Load16U { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, u16);
    }
    i64_load16_s @ I64Load16S { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, i16);
    }
    i64_load32_u @ I64Load32U { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, u32);
    }
    i64_load32_s @ I64Load32S { dst, addr, memarg } => {
        load!(ctx, regs, dst, addr, memarg, i32);
    }
    f32_load @ F32Load { dst, addr, memarg } => {
        let value = checked!(
            ctx,
            ctx.memories.load::<f32>(memarg, get!(regs, addr).as_u32())
        );
        set!(regs, dst, Value::from_f32(value));
    }
    f64_load @ F64Load { dst, addr, memarg } => {
        let value = checked!(
            ctx,
            ctx.memories.load::<f64>(memarg, get!(regs, addr).as_u32())
        );
        set!(regs, dst, Value::from_f64(value));
    }
    v128_load @ V128Load { dst, addr, memarg } => {
        let value = checked!(
            ctx,
            ctx.memories.load::<u128>(memarg, get!(regs, addr).as_u32())
        );
        unsafe {
            regs.add(*dst as usize).write(Value(value as u64));
            regs.add(*dst as usize + 1)
                .write(Value((value >> 64) as u64));
        }
    }
    i32_store @ I32Store {
        addr,
        value,
        memarg,
    } => {
        store!(ctx, regs, addr, value, memarg, i32);
    }
    i32_store8 @ I32Store8 {
        addr,
        value,
        memarg,
    } => {
        store!(ctx, regs, addr, value, memarg, u8);
    }
    i32_store16 @ I32Store16 {
        addr,
        value,
        memarg,
    } => {
        store!(ctx, regs, addr, value, memarg, u16);
    }
    i64_store @ I64Store {
        addr,
        value,
        memarg,
    } => {
        store!(ctx, regs, addr, value, memarg, i64);
    }
    i64_store8 @ I64Store8 {
        addr,
        value,
        memarg,
    } => {
        store!(ctx, regs, addr, value, memarg, u8);
    }
    i64_store16 @ I64Store16 {
        addr,
        value,
        memarg,
    } => {
        store!(ctx, regs, addr, value, memarg, u16);
    }
    i64_store32 @ I64Store32 {
        addr,
        value,
        memarg,
    } => {
        store!(ctx, regs, addr, value, memarg, u32);
    }
    f32_store @ F32Store {
        addr,
        value,
        memarg,
    } => {
        checked!(
            ctx,
            ctx.memories.store(
                memarg,
                get!(regs, addr).as_u32(),
                get!(regs, value).as_f32()
            )
        );
    }
    f64_store @ F64Store {
        addr,
        value,
        memarg,
    } => {
        checked!(
            ctx,
            ctx.memories.store(
                memarg,
                get!(regs, addr).as_u32(),
                get!(regs, value).as_f64()
            )
        );
    }
    v128_store @ V128Store {
        addr,
        value,
        memarg,
    } => {
        let number = unsafe {
            (*regs.add(*value as usize)).0 as u128
                | (((*regs.add(*value as usize + 1)).0 as u128) << 64)
        };
        checked!(
            ctx,
            ctx.memories
                .store(memarg, get!(regs, addr).as_u32(), number)
        );
    }
    call @ Call {
        function_index,
        base,
    } => {
        if !take_fuel(ctx, pc) {
            return;
        }
        if let Some((next, callee_regs)) = call_target(ctx, pc, *function_index, *base) {
            dispatch!(ctx, next, callee_regs);
        }
        return;
    }
    call_indirect @ CallIndirect {
        index,
        table_index,
        type_index,
        base,
    } => {
        // Meter before resolving the table so retrying an exhausted call cannot trap.
        if !take_fuel(ctx, pc) {
            return;
        }
        let table = match ctx.tables.get(&TableIndex(*table_index)) {
            Some(table) => table,
            None => {
                trap_exit(ctx, "table not found");
                return;
            }
        };
        let target = match table.get(get!(regs, index).as_u32() as usize) {
            Some(TableValue::FuncRef(index)) => *index,
            Some(TableValue::NullRef) => {
                trap_exit(ctx, "uninitialized element");
                return;
            }
            None => {
                trap_exit(ctx, "undefined element");
                return;
            }
        };
        let expected = checked!(
            ctx,
            ctx.functions
                .function_type_idx_type(&FunctionTypeIndex(*type_index))
        );
        let actual = checked!(ctx, ctx.functions.function_type_idx(&target));
        if expected != actual {
            trap_exit(ctx, "indirect call type mismatch");
            return;
        }
        if let Some((next, callee_regs)) = call_target(ctx, pc, target.0, *base) {
            dispatch!(ctx, next, callee_regs);
        }
        return;
    }
    jump @ Jump { offset } => {
        dispatch!(ctx, unsafe { pc.offset(*offset as isize) }, regs);
    }
    jump_if_nez @ JumpIfNez { cond, offset } => {
        if get!(regs, cond).as_i32() != 0 {
            dispatch!(ctx, unsafe { pc.offset(*offset as isize) }, regs);
        }
    }
    jump_if_eqz @ JumpIfEqz { cond, offset } => {
        if get!(regs, cond).as_i32() == 0 {
            dispatch!(ctx, unsafe { pc.offset(*offset as isize) }, regs);
        }
    }
    jump_if_i32_cmp @ JumpIfI32Cmp {
        lhs,
        rhs,
        kind,
        invert,
        offset,
    } => {
        if compare_i32(*kind, get!(regs, lhs).as_i32(), get!(regs, rhs).as_i32()) != *invert {
            dispatch!(ctx, unsafe { pc.offset(*offset as isize) }, regs);
        }
    }
    br_table @ BrTable { index, len } => {
        let entry = get!(regs, index).as_u32().min(*len - 1) as usize;
        dispatch!(ctx, unsafe { pc.add(1 + entry) }, regs);
    }
    ret @ Return { src, len } => {
        if let Some((next, caller_regs)) = return_target(ctx, regs, *src, *len) {
            dispatch!(ctx, next, caller_regs);
        }
        return;
    }
    ret_if @ ReturnIfNez { cond, src, len } => {
        if get!(regs, cond).as_i32() != 0 {
            if let Some((next, caller_regs)) = return_target(ctx, regs, *src, *len) {
                dispatch!(ctx, next, caller_regs);
            }
            return;
        }
    }
    i32_add @ I32Add { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a.wrapping_add(b));
    }
    i32_mul @ I32Mul { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a.wrapping_mul(b));
    }
    i32_div_u @ I32DivU { dst, lhs, rhs } => {
        let a = get!(regs, lhs).as_u32();
        let b = get!(regs, rhs).as_u32();
        if b == 0 {
            trap_exit(ctx, "integer divide by zero");
            return;
        }
        set!(regs, dst, Value((a / b) as u64));
    }
    i32_sub @ I32Sub { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a.wrapping_sub(b));
    }
    i32_shr_u @ I32ShrU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u32, |a, b| a.wrapping_shr(b));
    }
    i32_shl @ I32Shl { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u32, |a, b| a.wrapping_shl(b));
    }
    i32_and @ I32And { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a & b);
    }
    i32_xor @ I32Xor { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a ^ b);
    }
    i32_or @ I32Or { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a | b);
    }
    i32_rotr @ I32Rotr { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a.rotate_right(b as u32));
    }
    i32_gt_s @ I32GtS { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a > b);
    }
    i32_ge_s @ I32GeS { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a >= b);
    }
    i32_lt_s @ I32LtS { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a < b);
    }
    i32_lt_u @ I32LtU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u32, |a, b| a < b);
    }
    i32_gt_u @ I32GtU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u32, |a, b| a > b);
    }
    i32_le_u @ I32LeU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u32, |a, b| a <= b);
    }
    i32_ge_u @ I32GeU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u32, |a, b| a >= b);
    }
    i32_ne @ I32Ne { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a != b);
    }
    i32_eq @ I32Eq { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i32, |a, b| a == b);
    }
    i32_eqz @ I32Eqz { dst, src } => {
        set!(regs, dst, Value((get!(regs, src).as_i32() == 0) as u64));
    }
    i64_add @ I64Add { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i64, |a, b| a.wrapping_add(b));
    }
    i64_mul @ I64Mul { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i64, |a, b| a.wrapping_mul(b));
    }
    i64_sub @ I64Sub { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i64, |a, b| a.wrapping_sub(b));
    }
    i64_shr_u @ I64ShrU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u64, |a, b| a.wrapping_shr(b as u32));
    }
    i64_shl @ I64Shl { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u64, |a, b| a.wrapping_shl(b as u32));
    }
    i64_and @ I64And { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i64, |a, b| a & b);
    }
    i64_or @ I64Or { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i64, |a, b| a | b);
    }
    i64_gt_s @ I64GtS { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i64, |a, b| a > b);
    }
    i64_lt_u @ I64LtU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u64, |a, b| a < b);
    }
    i64_gt_u @ I64GtU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u64, |a, b| a > b);
    }
    i64_le_u @ I64LeU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u64, |a, b| a <= b);
    }
    i64_ge_u @ I64GeU { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, u64, |a, b| a >= b);
    }
    i64_eq @ I64Eq { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i64, |a, b| a == b);
    }
    i64_ne @ I64Ne { dst, lhs, rhs } => {
        bin!(regs, dst, lhs, rhs, i64, |a, b| a != b);
    }
    i64_eqz @ I64Eqz { dst, src } => {
        set!(regs, dst, Value((get!(regs, src).as_i64() == 0) as u64));
    }
    i32_extend8_s @ I32Extend8S { dst, src } => {
        set!(regs, dst, Value(get!(regs, src).0 as i8 as u64));
    }
    i64_extend_i32_u @ I64ExtendI32U { dst, src } => {
        set!(regs, dst, Value(get!(regs, src).as_u32() as u64));
    }
    i32_wrap_i64 @ I32WrapI64 { dst, src } => {
        set!(regs, dst, Value::from_i32(get!(regs, src).as_i32()));
    }
    i32_add_imm @ I32AddImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i32, |a, b| a.wrapping_add(b));
    }
    i32_sub_imm @ I32SubImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i32, |a, b| a.wrapping_sub(b));
    }
    i32_and_imm @ I32AndImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i32, |a, b| a & b);
    }
    i32_shl_imm @ I32ShlImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, u32, |a, b| a.wrapping_shl(b));
    }
    i32_shr_u_imm @ I32ShrUImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, u32, |a, b| a.wrapping_shr(b));
    }
    i64_add_imm @ I64AddImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i64, |a, b| a.wrapping_add(b));
    }
    i32_gt_s_imm @ I32GtSImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i32, |a, b| a > b);
    }
    i32_ge_s_imm @ I32GeSImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i32, |a, b| a >= b);
    }
    i32_lt_s_imm @ I32LtSImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i32, |a, b| a < b);
    }
    i32_lt_u_imm @ I32LtUImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, u32, |a, b| a < b);
    }
    i32_gt_u_imm @ I32GtUImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, u32, |a, b| a > b);
    }
    i32_le_u_imm @ I32LeUImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, u32, |a, b| a <= b);
    }
    i32_ge_u_imm @ I32GeUImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, u32, |a, b| a >= b);
    }
    i32_ne_imm @ I32NeImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i32, |a, b| a != b);
    }
    i32_eq_imm @ I32EqImm { dst, lhs, imm } => {
        imm!(regs, dst, lhs, imm, i32, |a, b| a == b);
    }
    select @ Select { dst, cond, a, b } => {
        let value = if get!(regs, cond).as_i32() != 0 {
            get!(regs, a)
        } else {
            get!(regs, b)
        };
        set!(regs, dst, value);
    }
    select_wide @ SelectWide { dst, cond, a, b } => {
        let src = if get!(regs, cond).as_i32() != 0 {
            *a
        } else {
            *b
        };
        unsafe { std::ptr::copy(regs.add(src as usize), regs.add(*dst as usize), 2) };
    }
    unreachable @ Unreachable => {
        trap_exit(ctx, "unreachable");
        return;
    }
    unsupported @ Unsupported { description } => {
        unsupported_exit(ctx, description);
        return;
    }
});

pub(crate) fn dispatch(
    ctx: &mut DispatchCtx<'_>,
    pc: *const RuntimeWackasmOperator,
    regs: *mut Value,
) {
    dispatch!(ctx, pc, regs);
}
