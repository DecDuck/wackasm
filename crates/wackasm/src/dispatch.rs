use crate::{
    HaltError, Value,
    compiler::{
        I32Comparison, I64Comparison, Reg, RuntimeWackasmOperator, WackasmMemArg, WackasmOperator,
    },
    functions::{FuncInfo, FunctionManager, MAX_CALL_DEPTH},
    globals::GlobalManager,
    macros::{trap, unsupported},
    memory::{MemoryManager, MemoryNumber, MemoryView},
    module::{FunctionFrame, FunctionIndex, GlobalIndex, TableIndex, TableValue},
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

impl DispatchCtx<'_> {
    #[inline(always)]
    fn memory_view(&mut self, mem0: MemoryView, index: u32) -> MemoryView {
        if index == 0 {
            mem0
        } else {
            // Only compiled, wasmparser-validated memory indexes reach dispatch.
            unsafe { self.memories.view_validated(index) }
        }
    }

    #[inline(always)]
    fn load<T: MemoryNumber, const MEMORY0: bool>(
        &mut self,
        mem0: MemoryView,
        memarg: &WackasmMemArg,
        address: u32,
    ) -> Option<T> {
        let view = if MEMORY0 {
            mem0
        } else {
            unsafe { self.memories.view_validated(memarg.memory) }
        };
        unsafe { view.load(address, memarg.offset) }
    }

    #[inline(always)]
    fn store<T: MemoryNumber, const MEMORY0: bool>(
        &mut self,
        mem0: MemoryView,
        memarg: &WackasmMemArg,
        address: u32,
        value: T,
    ) -> Option<()> {
        let view = if MEMORY0 {
            mem0
        } else {
            unsafe { self.memories.view_validated(memarg.memory) }
        };
        unsafe { view.store(address, memarg.offset, value) }
    }
}

// The SysV ABI passes six integer arguments in registers (Win64 passes four) and has fewer
// callee-saved registers, so memory 0 stays in registers across threaded dispatch.
// Fuel deliberately stays in `DispatchCtx`: holding it in a register measured ~3% slower.
#[cfg(target_arch = "x86_64")]
#[allow(improper_ctypes_definitions)]
pub(crate) type WackasmOperatorHandler = for<'a> extern "sysv64" fn(
    &mut DispatchCtx<'a>,
    *const RuntimeWackasmOperator,
    *mut Value,
    MemoryView,
);
#[cfg(not(target_arch = "x86_64"))]
pub(crate) type WackasmOperatorHandler =
    for<'a> fn(&mut DispatchCtx<'a>, *const RuntimeWackasmOperator, *mut Value, MemoryView);

/// Defines a function with the handler ABI.
macro_rules! handler {
    ($(#[$attr:meta])* fn $($item:tt)*) => {
        #[cfg(target_arch = "x86_64")]
        #[allow(improper_ctypes_definitions)]
        $(#[$attr])* extern "sysv64" fn $($item)*
        #[cfg(not(target_arch = "x86_64"))]
        $(#[$attr])* fn $($item)*
    };
}

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
fn set_exit(ctx: &mut DispatchCtx<'_>, exit: Exit) {
    // Dispatch stops at its first exit, so there is no previous value to drop; skipping
    // the drop keeps hot handlers free of calls and stack frames.
    unsafe { std::ptr::write(&mut ctx.exit, Some(exit)) };
}

// Cold paths are tail-called with the handler signature so hot handlers need no stack frame.
macro_rules! trap_handlers {
    ($($name:ident => $message:literal),* $(,)?) => {$(
        handler! {
            #[cold]
            #[inline(never)]
            fn $name(
                ctx: &mut DispatchCtx<'_>,
                _: *const RuntimeWackasmOperator,
                _: *mut Value,
                _: MemoryView,
            ) {
                trap_exit(ctx, $message);
            }
        }
    )*};
}

trap_handlers! {
    unreachable_instruction => "unreachable",
    out_of_bounds => "out of bounds memory access",
    divide_by_zero => "integer divide by zero",
    invalid_function => "invalid function index",
    missing_frame => "dispatch requires a frame",
    call_stack_exhausted => "call stack exhausted",
    stack_overflow => "potential stack overflow",
    table_not_found => "table not found",
    uninitialized_element => "uninitialized element",
    undefined_element => "undefined element",
    signature_not_found => "no signature for function type index",
    indirect_type_mismatch => "indirect call type mismatch",
}

handler! {
    /// Ends dispatch after an exit has already been recorded.
    fn stop(_: &mut DispatchCtx<'_>, _: *const RuntimeWackasmOperator, _: *mut Value, _: MemoryView) {}
}

handler! {
    #[cold]
    #[inline(never)]
    fn out_of_fuel(
        ctx: &mut DispatchCtx<'_>,
        pc: *const RuntimeWackasmOperator,
        _: *mut Value,
        _: MemoryView,
    ) {
        save_pc(ctx, pc);
        set_exit(ctx, Exit::OutOfFuel);
    }
}

#[inline(always)]
fn save_pc(ctx: &mut DispatchCtx<'_>, pc: *const RuntimeWackasmOperator) {
    // Dispatch always runs with a valid frame; the checks only avoid panic paths.
    if let Some(frame) = ctx.frames.last_mut()
        && let Some(info) = ctx.functions.info.get(frame.function_index.0 as usize)
    {
        frame.pc = unsafe { pc.offset_from_unsigned(info.code) };
    }
}

type DispatchTarget = (*const RuntimeWackasmOperator, *mut Value);

/// Enters an internal callee, or returns the handler that finishes the call outside it.
#[inline(always)]
fn call_target(
    ctx: &mut DispatchCtx<'_>,
    pc: *const RuntimeWackasmOperator,
    function_index: u32,
    base: Reg,
    info: &FuncInfo,
) -> Result<DispatchTarget, WackasmOperatorHandler> {
    if ctx.frames.len() >= MAX_CALL_DEPTH {
        return Err(call_stack_exhausted);
    }
    let Some(caller) = ctx.frames.last() else {
        return Err(missing_frame);
    };
    let stack_base = caller.stack_base + base as usize;
    if info.frame_size > ctx.capacity.saturating_sub(stack_base) || stack_base > ctx.capacity {
        return Err(stack_overflow);
    }
    let code = info.code;
    let params_slots = info.params_slots;
    let local_defaults = info.local_defaults;
    let locals_len = info.locals_len;
    save_pc(ctx, unsafe { pc.add(1) });
    if code.is_null() {
        set_exit(
            ctx,
            Exit::ImportCall {
                function_index,
                base: stack_base,
            },
        );
        return Err(stop);
    }
    let regs = unsafe { ctx.buffer.add(stack_base) };
    unsafe {
        match locals_len {
            0 => {}
            1 => regs.add(params_slots).write(local_defaults.read()),
            2 => {
                regs.add(params_slots).write(local_defaults.read());
                regs.add(params_slots + 1)
                    .write(local_defaults.add(1).read());
            }
            _ => std::ptr::copy_nonoverlapping(local_defaults, regs.add(params_slots), locals_len),
        }
    }
    ctx.frames.push(FunctionFrame {
        function_index: FunctionIndex(function_index),
        pc: 0,
        stack_base,
    });
    Ok((code, regs))
}

/// Resumes the caller, or returns the handler that finishes this return outside dispatch.
#[inline(always)]
fn return_target(
    ctx: &mut DispatchCtx<'_>,
    regs: *mut Value,
    src: Reg,
    len: Reg,
) -> Result<DispatchTarget, WackasmOperatorHandler> {
    debug_assert_eq!(
        Some(len as usize),
        ctx.frames.last().and_then(|frame| ctx
            .functions
            .info
            .get(frame.function_index.0 as usize)
            .map(|info| info.results_slots))
    );
    unsafe {
        let source = regs.add(src as usize);
        match len {
            0 => {}
            1 => regs.write(source.read()),
            2 => {
                // Read both slots before writing: result and argument slots may overlap.
                let low = source.read();
                let high = source.add(1).read();
                regs.write(low);
                regs.add(1).write(high);
            }
            _ => std::ptr::copy(source, regs, len as usize),
        }
    };
    let Some(frame) = ctx.frames.pop() else {
        return Err(missing_frame);
    };
    let Some(caller) = ctx.frames.last() else {
        set_exit(
            ctx,
            Exit::Return {
                base: frame.stack_base,
                len: len as usize,
            },
        );
        return Err(stop);
    };
    match ctx.functions.info.get(caller.function_index.0 as usize) {
        Some(info) => Ok(unsafe { (info.code.add(caller.pc), ctx.buffer.add(caller.stack_base)) }),
        None => Err(invalid_function),
    }
}

// Comparison codes are paired so that `code ^ 1` is the negated comparison.
const EQ: u8 = 0;
const NE: u8 = 1;
const LT_S: u8 = 2;
const GE_S: u8 = 3;
const LT_U: u8 = 4;
const GE_U: u8 = 5;
const GT_S: u8 = 6;
const LE_S: u8 = 7;
const GT_U: u8 = 8;
const LE_U: u8 = 9;

impl I32Comparison {
    fn code(self, invert: bool) -> u8 {
        let code = match self {
            I32Comparison::Eq => EQ,
            I32Comparison::Ne => NE,
            I32Comparison::LtS => LT_S,
            I32Comparison::LtU => LT_U,
            I32Comparison::GtS => GT_S,
            I32Comparison::GtU => GT_U,
            I32Comparison::GeS => GE_S,
            I32Comparison::GeU => GE_U,
            I32Comparison::LeU => LE_U,
        };
        code ^ invert as u8
    }
}

impl I64Comparison {
    fn code(self, invert: bool) -> u8 {
        let code = match self {
            I64Comparison::Eq => EQ,
            I64Comparison::Ne => NE,
            I64Comparison::GtS => GT_S,
            I64Comparison::LtU => LT_U,
            I64Comparison::GtU => GT_U,
            I64Comparison::LeU => LE_U,
            I64Comparison::GeU => GE_U,
        };
        code ^ invert as u8
    }
}

macro_rules! comparison {
    ($name:ident, $signed:ty, $unsigned:ty) => {
        #[inline(always)]
        fn $name(code: u8, lhs: $signed, rhs: $signed) -> bool {
            let (unsigned_lhs, unsigned_rhs) = (lhs as $unsigned, rhs as $unsigned);
            match code {
                EQ => lhs == rhs,
                NE => lhs != rhs,
                LT_S => lhs < rhs,
                GE_S => lhs >= rhs,
                LT_U => unsigned_lhs < unsigned_rhs,
                GE_U => unsigned_lhs >= unsigned_rhs,
                GT_S => lhs > rhs,
                LE_S => lhs <= rhs,
                GT_U => unsigned_lhs > unsigned_rhs,
                LE_U => unsigned_lhs <= unsigned_rhs,
                _ => unreachable!(),
            }
        }
    };
}

comparison!(compare_i32, i32, u32);
comparison!(compare_i64, i64, u64);

/// Branch specialization bit: backward branches (loop backedges) meter fuel.
const BACKWARD: u8 = 1;

fn branch_spec(offset: i32) -> u8 {
    if offset <= 0 { BACKWARD } else { 0 }
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
macro_rules! bounded {
    ($ctx:expr, $access:expr) => {
        match $access {
            Some(value) => value,
            None => become out_of_bounds(
                $ctx,
                std::ptr::null(),
                std::ptr::null_mut(),
                MemoryView::EMPTY,
            ),
        }
    };
}
macro_rules! charge {
    ($ctx:expr, $pc:expr, $regs:expr, $mem0:expr) => {
        if $ctx.fuel <= 0 {
            become out_of_fuel($ctx, $pc, $regs, $mem0);
        }
        $ctx.fuel -= 1;
    };
}
macro_rules! branch {
    ($ctx:expr, $pc:expr, $regs:expr, $mem0:expr, $offset:expr, $spec:expr) => {{
        if $spec & BACKWARD != 0 {
            charge!($ctx, $pc, $regs, $mem0);
        }
        dispatch!($ctx, unsafe { $pc.offset(*$offset as isize) }, $regs, $mem0);
    }};
}
macro_rules! enter {
    ($ctx:expr, $pc:expr, $regs:expr, $mem0:expr, $target:expr) => {
        match $target {
            Ok((next, regs)) => dispatch!($ctx, next, regs, $mem0),
            Err(exit) => become exit($ctx, $pc, $regs, $mem0),
        }
    };
}
macro_rules! add_branch {
    ($ctx:expr, $pc:expr, $regs:expr, $mem0:expr, $dst:expr, $lhs:expr, $imm:expr, $offset:expr, |$value:ident| $taken:expr) => {{
        let $value = get!($regs, $lhs).as_i32().wrapping_add(*$imm);
        if $taken {
            // Meter before writing so an exhausted backedge can be retried.
            if SPEC & BACKWARD != 0 {
                charge!($ctx, $pc, $regs, $mem0);
            }
            set!($regs, $dst, Value::from_i32($value));
            dispatch!($ctx, unsafe { $pc.offset(*$offset as isize) }, $regs, $mem0);
        }
        set!($regs, $dst, Value::from_i32($value));
    }};
}
macro_rules! load_rel {
    ($ctx:expr, $mem0:expr, $regs:expr, $dst:expr, $base:expr, $delta:expr, $offset:expr, $ty:ty) => {{
        let address = get!($regs, $base).as_u32().wrapping_add(*$delta as u32);
        let value: $ty = bounded!($ctx, unsafe { $mem0.load(address, *$offset) });
        set!($regs, $dst, Value(value as u64));
    }};
}
macro_rules! store_imm {
    ($ctx:expr, $mem0:expr, $regs:expr, $addr:expr, $offset:expr, $value:expr) => {{
        let address = get!($regs, $addr).as_u32();
        bounded!($ctx, unsafe { $mem0.store(address, *$offset, $value) });
    }};
}
macro_rules! load_idx {
    ($ctx:expr, $mem0:expr, $regs:expr, $dst:expr, $base:expr, $index:expr, $offset:expr, $ty:ty) => {{
        let address = get!($regs, $base).as_u32().wrapping_add(get!($regs, $index).as_u32());
        let value: $ty = bounded!($ctx, unsafe { $mem0.load(address, *$offset) });
        set!($regs, $dst, Value(value as u64));
    }};
}
macro_rules! store_idx {
    ($ctx:expr, $mem0:expr, $regs:expr, $base:expr, $index:expr, $offset:expr, $value:expr) => {{
        let address = get!($regs, $base).as_u32().wrapping_add(get!($regs, $index).as_u32());
        let value = $value;
        bounded!($ctx, unsafe { $mem0.store(address, *$offset, value) });
    }};
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
    ($ctx:expr, $mem0:expr, $regs:expr, $dst:expr, $addr:expr, $memarg:expr, $ty:ty, $memory0:ident) => {{
        load!(@address $ctx, $mem0, $regs, $dst, get!($regs, $addr).as_u32(), $memarg, $ty, $memory0);
    }};
    (@address $ctx:expr, $mem0:expr, $regs:expr, $dst:expr, $addr:expr, $memarg:expr, $ty:ty, $memory0:ident) => {{
        let value = bounded!($ctx, $ctx.load::<$ty, $memory0>($mem0, $memarg, $addr));
        set!($regs, $dst, Value(value as u64));
    }};
}
macro_rules! store {
    ($ctx:expr, $mem0:expr, $regs:expr, $addr:expr, $value:expr, $memarg:expr, $ty:ty, $memory0:ident) => {{
        store!(@address $ctx, $mem0, $regs, get!($regs, $addr).as_u32(), $value, $memarg, $ty, $memory0);
    }};
    (@address $ctx:expr, $mem0:expr, $regs:expr, $addr:expr, $value:expr, $memarg:expr, $ty:ty, $memory0:ident) => {{
        bounded!(
            $ctx,
            $ctx.store::<$ty, $memory0>(
                $mem0,
                $memarg,
                $addr,
                get!($regs, $value).0 as $ty,
            )
        );
    }};
}

ops!(|ctx, pc, regs, mem0| const MEMORY0, SPEC match op {
    nop @ Nop => {}
    fuel @ ConsumeFuel => {
        charge!(ctx, pc, regs, mem0);
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
        if *mem == 0 {
            mem0 = ctx.memories.memory0();
        }
        set!(regs, dst, Value::from_i32(result));
    }
    memory_copy @ MemoryCopy {
        dst_mem,
        src_mem,
        dst,
        src,
        len,
    } => {
        let source = ctx.memory_view(mem0, *src_mem as u32);
        let destination = ctx.memory_view(mem0, *dst_mem as u32);
        let dest = get!(regs, dst).as_u32() as usize;
        let source_address = get!(regs, src).as_u32() as usize;
        let length = get!(regs, len).as_u32() as usize;
        checked!(ctx, unsafe {
            destination.copy_from(source, dest, source_address, length)
        });
    }
    memory_fill @ MemoryFill {
        mem,
        addr,
        value,
        len,
    } => {
        let memory = ctx.memory_view(mem0, *mem);
        let start = get!(regs, addr).as_u32() as usize;
        let length = get!(regs, len).as_u32() as usize;
        let byte = get!(regs, value).0 as u8;
        checked!(ctx, unsafe {
            memory.fill(start, length, byte)
        });
    }
    i32_load @ I32Load { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, i32, MEMORY0);
    }
    i32_load8_u @ I32Load8U { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, u8, MEMORY0);
    }
    i32_load8_s @ I32Load8S { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, i8, MEMORY0);
    }
    i32_load16_u @ I32Load16U { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, u16, MEMORY0);
    }
    i32_load16_s @ I32Load16S { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, i16, MEMORY0);
    }
    i64_load @ I64Load { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, i64, MEMORY0);
    }
    i64_load8_u @ I64Load8U { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, u8, MEMORY0);
    }
    i64_load8_s @ I64Load8S { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, i8, MEMORY0);
    }
    i64_load16_u @ I64Load16U { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, u16, MEMORY0);
    }
    i64_load16_s @ I64Load16S { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, i16, MEMORY0);
    }
    i64_load32_u @ I64Load32U { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, u32, MEMORY0);
    }
    i64_load32_s @ I64Load32S { dst, addr, memarg } [memory] => {
        load!(ctx, mem0, regs, dst, addr, memarg, i32, MEMORY0);
    }
    f32_load @ F32Load { dst, addr, memarg } [memory] => {
        let value = bounded!(
            ctx,
            ctx.load::<f32, MEMORY0>(mem0, memarg, get!(regs, addr).as_u32())
        );
        set!(regs, dst, Value::from_f32(value));
    }
    f64_load @ F64Load { dst, addr, memarg } [memory] => {
        let value = bounded!(
            ctx,
            ctx.load::<f64, MEMORY0>(mem0, memarg, get!(regs, addr).as_u32())
        );
        set!(regs, dst, Value::from_f64(value));
    }
    v128_load @ V128Load { dst, addr, memarg } [memory] => {
        let value = bounded!(
            ctx,
            ctx.load::<u128, MEMORY0>(mem0, memarg, get!(regs, addr).as_u32())
        );
        unsafe {
            regs.add(*dst as usize).write(Value(value as u64));
            regs.add(*dst as usize + 1)
                .write(Value((value >> 64) as u64));
        }
    }
    i32_store @ I32Store { addr, value, memarg } [memory] => {
        store!(ctx, mem0, regs, addr, value, memarg, i32, MEMORY0);
    }
    i32_store8 @ I32Store8 { addr, value, memarg } [memory] => {
        store!(ctx, mem0, regs, addr, value, memarg, u8, MEMORY0);
    }
    i32_store16 @ I32Store16 { addr, value, memarg } [memory] => {
        store!(ctx, mem0, regs, addr, value, memarg, u16, MEMORY0);
    }
    i64_store @ I64Store { addr, value, memarg } [memory] => {
        store!(ctx, mem0, regs, addr, value, memarg, i64, MEMORY0);
    }
    i64_store8 @ I64Store8 { addr, value, memarg } [memory] => {
        store!(ctx, mem0, regs, addr, value, memarg, u8, MEMORY0);
    }
    i64_store16 @ I64Store16 { addr, value, memarg } [memory] => {
        store!(ctx, mem0, regs, addr, value, memarg, u16, MEMORY0);
    }
    i64_store32 @ I64Store32 { addr, value, memarg } [memory] => {
        store!(ctx, mem0, regs, addr, value, memarg, u32, MEMORY0);
    }
    f32_store @ F32Store { addr, value, memarg } [memory] => {
        bounded!(
            ctx,
            ctx.store::<f32, MEMORY0>(
                mem0,
                memarg,
                get!(regs, addr).as_u32(),
                get!(regs, value).as_f32()
            )
        );
    }
    f64_store @ F64Store { addr, value, memarg } [memory] => {
        bounded!(
            ctx,
            ctx.store::<f64, MEMORY0>(
                mem0,
                memarg,
                get!(regs, addr).as_u32(),
                get!(regs, value).as_f64()
            )
        );
    }
    v128_store @ V128Store { addr, value, memarg } [memory] => {
        let number = unsafe {
            (*regs.add(*value as usize)).0 as u128
                | (((*regs.add(*value as usize + 1)).0 as u128) << 64)
        };
        bounded!(
            ctx,
            ctx.store::<u128, MEMORY0>(mem0, memarg, get!(regs, addr).as_u32(), number)
        );
    }
    i32_load_at @ I32LoadAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, i32, MEMORY0);
    }
    i32_load8_u_at @ I32Load8UAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, u8, MEMORY0);
    }
    i32_load8_s_at @ I32Load8SAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, i8, MEMORY0);
    }
    i32_load16_u_at @ I32Load16UAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, u16, MEMORY0);
    }
    i32_load16_s_at @ I32Load16SAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, i16, MEMORY0);
    }
    i64_load_at @ I64LoadAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, i64, MEMORY0);
    }
    i64_load8_u_at @ I64Load8UAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, u8, MEMORY0);
    }
    i64_load8_s_at @ I64Load8SAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, i8, MEMORY0);
    }
    i64_load16_u_at @ I64Load16UAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, u16, MEMORY0);
    }
    i64_load16_s_at @ I64Load16SAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, i16, MEMORY0);
    }
    i64_load32_u_at @ I64Load32UAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, u32, MEMORY0);
    }
    i64_load32_s_at @ I64Load32SAt { dst, memarg } [memory] => {
        load!(@address ctx, mem0, regs, dst, 0, memarg, i32, MEMORY0);
    }
    f32_load_at @ F32LoadAt { dst, memarg } [memory] => {
        let value = bounded!(ctx, ctx.load::<f32, MEMORY0>(mem0, memarg, 0));
        set!(regs, dst, Value::from_f32(value));
    }
    f64_load_at @ F64LoadAt { dst, memarg } [memory] => {
        let value = bounded!(ctx, ctx.load::<f64, MEMORY0>(mem0, memarg, 0));
        set!(regs, dst, Value::from_f64(value));
    }
    v128_load_at @ V128LoadAt { dst, memarg } [memory] => {
        let value = bounded!(ctx, ctx.load::<u128, MEMORY0>(mem0, memarg, 0));
        unsafe {
            regs.add(*dst as usize).write(Value(value as u64));
            regs.add(*dst as usize + 1).write(Value((value >> 64) as u64));
        }
    }
    i32_store_at @ I32StoreAt { value, memarg } [memory] => {
        store!(@address ctx, mem0, regs, 0, value, memarg, i32, MEMORY0);
    }
    i32_store8_at @ I32Store8At { value, memarg } [memory] => {
        store!(@address ctx, mem0, regs, 0, value, memarg, u8, MEMORY0);
    }
    i32_store16_at @ I32Store16At { value, memarg } [memory] => {
        store!(@address ctx, mem0, regs, 0, value, memarg, u16, MEMORY0);
    }
    i64_store_at @ I64StoreAt { value, memarg } [memory] => {
        store!(@address ctx, mem0, regs, 0, value, memarg, i64, MEMORY0);
    }
    i64_store8_at @ I64Store8At { value, memarg } [memory] => {
        store!(@address ctx, mem0, regs, 0, value, memarg, u8, MEMORY0);
    }
    i64_store16_at @ I64Store16At { value, memarg } [memory] => {
        store!(@address ctx, mem0, regs, 0, value, memarg, u16, MEMORY0);
    }
    i64_store32_at @ I64Store32At { value, memarg } [memory] => {
        store!(@address ctx, mem0, regs, 0, value, memarg, u32, MEMORY0);
    }
    f32_store_at @ F32StoreAt { value, memarg } [memory] => {
        bounded!(ctx, ctx.store::<f32, MEMORY0>(mem0, memarg, 0, get!(regs, value).as_f32()));
    }
    f64_store_at @ F64StoreAt { value, memarg } [memory] => {
        bounded!(ctx, ctx.store::<f64, MEMORY0>(mem0, memarg, 0, get!(regs, value).as_f64()));
    }
    v128_store_at @ V128StoreAt { value, memarg } [memory] => {
        let number = unsafe {
            (*regs.add(*value as usize)).0 as u128
                | (((*regs.add(*value as usize + 1)).0 as u128) << 64)
        };
        bounded!(ctx, ctx.store::<u128, MEMORY0>(mem0, memarg, 0, number));
    }
    i32_store_imm @ I32StoreImm { addr, value, offset } => {
        store_imm!(ctx, mem0, regs, addr, offset, *value);
    }
    i32_store8_imm @ I32Store8Imm { addr, value, offset } => {
        store_imm!(ctx, mem0, regs, addr, offset, *value as u8);
    }
    i32_store16_imm @ I32Store16Imm { addr, value, offset } => {
        store_imm!(ctx, mem0, regs, addr, offset, *value as u16);
    }
    i64_store_imm @ I64StoreImm { addr, value, offset } => {
        store_imm!(ctx, mem0, regs, addr, offset, *value as i64);
    }
    i32_load_rel @ I32LoadRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, i32);
    }
    i32_load8_u_rel @ I32Load8URel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, u8);
    }
    i32_load8_s_rel @ I32Load8SRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, i8);
    }
    i32_load16_u_rel @ I32Load16URel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, u16);
    }
    i32_load16_s_rel @ I32Load16SRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, i16);
    }
    i64_load_rel @ I64LoadRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, i64);
    }
    i64_load8_u_rel @ I64Load8URel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, u8);
    }
    i64_load8_s_rel @ I64Load8SRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, i8);
    }
    i64_load16_u_rel @ I64Load16URel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, u16);
    }
    i64_load16_s_rel @ I64Load16SRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, i16);
    }
    i64_load32_u_rel @ I64Load32URel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, u32);
    }
    i64_load32_s_rel @ I64Load32SRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, i32);
    }
    // Float bits are loaded as integers: `Value` holds the zero-extended bit pattern.
    f32_load_rel @ F32LoadRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, u32);
    }
    f64_load_rel @ F64LoadRel { dst, base, delta, offset } => {
        load_rel!(ctx, mem0, regs, dst, base, delta, offset, u64);
    }
    load_i8_idx @ LoadI8Idx { dst, base, index, offset } => {
        load_idx!(ctx, mem0, regs, dst, base, index, offset, i8);
    }
    load_u8_idx @ LoadU8Idx { dst, base, index, offset } => {
        load_idx!(ctx, mem0, regs, dst, base, index, offset, u8);
    }
    load_i16_idx @ LoadI16Idx { dst, base, index, offset } => {
        load_idx!(ctx, mem0, regs, dst, base, index, offset, i16);
    }
    load_u16_idx @ LoadU16Idx { dst, base, index, offset } => {
        load_idx!(ctx, mem0, regs, dst, base, index, offset, u16);
    }
    load_i32_idx @ LoadI32Idx { dst, base, index, offset } => {
        load_idx!(ctx, mem0, regs, dst, base, index, offset, i32);
    }
    load_u32_idx @ LoadU32Idx { dst, base, index, offset } => {
        load_idx!(ctx, mem0, regs, dst, base, index, offset, u32);
    }
    load_i64_idx @ LoadI64Idx { dst, base, index, offset } => {
        load_idx!(ctx, mem0, regs, dst, base, index, offset, i64);
    }
    store8_idx @ Store8Idx { base, index, value, offset } => {
        store_idx!(ctx, mem0, regs, base, index, offset, get!(regs, value).0 as u8);
    }
    store16_idx @ Store16Idx { base, index, value, offset } => {
        store_idx!(ctx, mem0, regs, base, index, offset, get!(regs, value).0 as u16);
    }
    store32_idx @ Store32Idx { base, index, value, offset } => {
        store_idx!(ctx, mem0, regs, base, index, offset, get!(regs, value).0 as u32);
    }
    store64_idx @ Store64Idx { base, index, value, offset } => {
        store_idx!(ctx, mem0, regs, base, index, offset, get!(regs, value).0);
    }
    store8_imm_idx @ Store8ImmIdx { base, index, value, offset } => {
        store_idx!(ctx, mem0, regs, base, index, offset, *value as u8);
    }
    store16_imm_idx @ Store16ImmIdx { base, index, value, offset } => {
        store_idx!(ctx, mem0, regs, base, index, offset, *value as u16);
    }
    store32_imm_idx @ Store32ImmIdx { base, index, value, offset } => {
        store_idx!(ctx, mem0, regs, base, index, offset, *value);
    }
    store64_imm_idx @ Store64ImmIdx { base, index, value, offset } => {
        store_idx!(ctx, mem0, regs, base, index, offset, *value as i64);
    }
    i32_add_imm_jump_if_nez @ I32AddImmJumpIfNez { dst, lhs, imm, offset } [branch] => {
        add_branch!(ctx, pc, regs, mem0, dst, lhs, imm, offset, |value| value != 0);
    }
    i32_add_imm_jump_if_eqz @ I32AddImmJumpIfEqz { dst, lhs, imm, offset } [branch] => {
        add_branch!(ctx, pc, regs, mem0, dst, lhs, imm, offset, |value| value == 0);
    }
    call @ Call {
        function_index,
        base,
    } => {
        charge!(ctx, pc, regs, mem0);
        let Some(info) = ctx.functions.info.get(*function_index as usize) else {
            become invalid_function(ctx, pc, regs, mem0);
        };
        enter!(ctx, pc, regs, mem0, call_target(ctx, pc, *function_index, *base, info));
    }
    call_indirect @ CallIndirect {
        index,
        table_index,
        type_index,
        base,
    } => {
        // Meter before resolving the table so retrying an exhausted call cannot trap.
        charge!(ctx, pc, regs, mem0);
        let Some(table) = ctx.tables.get(&TableIndex(*table_index)) else {
            become table_not_found(ctx, pc, regs, mem0);
        };
        let target = match table.get(get!(regs, index).as_u32() as usize) {
            Some(TableValue::FuncRef(index)) => *index,
            Some(TableValue::NullRef) => become uninitialized_element(ctx, pc, regs, mem0),
            None => become undefined_element(ctx, pc, regs, mem0),
        };
        let Some(&expected) = ctx.functions.signature_ids.get(*type_index as usize) else {
            become signature_not_found(ctx, pc, regs, mem0);
        };
        let Some(info) = ctx.functions.info.get(target.0 as usize) else {
            become invalid_function(ctx, pc, regs, mem0);
        };
        if expected != info.signature_id {
            become indirect_type_mismatch(ctx, pc, regs, mem0);
        }
        enter!(ctx, pc, regs, mem0, call_target(ctx, pc, target.0, *base, info));
    }
    jump @ Jump { offset } [branch] => {
        branch!(ctx, pc, regs, mem0, offset, SPEC);
    }
    jump_if_nez @ JumpIfNez { cond, offset } [branch] => {
        if get!(regs, cond).as_i32() != 0 {
            branch!(ctx, pc, regs, mem0, offset, SPEC);
        }
    }
    jump_if_eqz @ JumpIfEqz { cond, offset } [branch] => {
        if get!(regs, cond).as_i32() == 0 {
            branch!(ctx, pc, regs, mem0, offset, SPEC);
        }
    }
    // Compare branches specialize on `kind.code(invert) << 1 | backward`.
    jump_if_i32_cmp @ JumpIfI32Cmp { lhs, rhs, offset } [compare] => {
        if compare_i32(SPEC >> 1, get!(regs, lhs).as_i32(), get!(regs, rhs).as_i32()) {
            branch!(ctx, pc, regs, mem0, offset, SPEC);
        }
    }
    jump_if_i32_cmp_imm @ JumpIfI32CmpImm { lhs, imm, offset } [compare] => {
        if compare_i32(SPEC >> 1, get!(regs, lhs).as_i32(), *imm) {
            branch!(ctx, pc, regs, mem0, offset, SPEC);
        }
    }
    jump_if_i64_cmp @ JumpIfI64Cmp { lhs, rhs, offset } [compare] => {
        if compare_i64(SPEC >> 1, get!(regs, lhs).as_i64(), get!(regs, rhs).as_i64()) {
            branch!(ctx, pc, regs, mem0, offset, SPEC);
        }
    }
    jump_if_i64_cmp_imm @ JumpIfI64CmpImm { lhs, imm, offset } [compare] => {
        if compare_i64(SPEC >> 1, get!(regs, lhs).as_i64(), *imm as i64) {
            branch!(ctx, pc, regs, mem0, offset, SPEC);
        }
    }
    br_table @ BrTable { index, len } => {
        let entry = get!(regs, index).as_u32().min(*len - 1) as usize;
        dispatch!(ctx, unsafe { pc.add(1 + entry) }, regs, mem0);
    }
    ret @ Return { src, len } => {
        enter!(ctx, pc, regs, mem0, return_target(ctx, regs, *src, *len));
    }
    ret_if @ ReturnIfNez { cond, src, len } => {
        if get!(regs, cond).as_i32() != 0 {
            enter!(ctx, pc, regs, mem0, return_target(ctx, regs, *src, *len));
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
            become divide_by_zero(ctx, pc, regs, mem0);
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
    i64_cmp_imm @ I64CmpImm { dst, lhs, imm, kind } => {
        set!(regs, dst, Value(compare_i64(kind.code(false), get!(regs, lhs).as_i64(), *imm as i64) as u64));
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
        become unreachable_instruction(ctx, pc, regs, mem0);
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
    mem0: MemoryView,
) {
    let handler = unsafe { (*pc).handler() };
    handler(ctx, pc, regs, mem0);
}
