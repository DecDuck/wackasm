use crate::{
    HaltError, Value, compiler::WackasmOperator, functions::FunctionManager,
    globals::GlobalManager, memory::MemoryManager, table::TableManager,
};

pub(crate) struct DispatchCtx<'a> {
    memories: &'a mut MemoryManager,
    globals: &'a mut GlobalManager,
    tables: &'a TableManager,
    functions: &'a FunctionManager, // CallIndirect type check
    locals: *mut Value,             // = stack start + FunctionFrame.stack_base
    code: *mut WackasmOperator,
}

pub(crate) enum Exit {
    OutOfFuel {
        ip: *const WackasmOperator,
        stack: *mut Value,
        fuel: usize,
    },
    Call {
        ip: *const WackasmOperator,
        stack: *mut Value,
        fuel: usize,
        function_index: u32,
    },
    Return {
        ip: *const WackasmOperator,
        stack: *mut Value,
        fuel: usize,
    },
    Halt(HaltError),
}

type Handler = fn(&mut DispatchCtx, *const WackasmOperator, *mut Value, usize) -> Exit;

macro_rules! dispatch {
    ($ctx:expr, $ip:expr, $stack:expr, $fuel:expr) => {{
        let (ctx, ip, stack, fuel) = ($ctx, $ip, $stack, $fuel);
        if fuel == 0 {
            return Exit::OutOfFuel { ip, stack, fuel };
        }
        match unsafe { &*ip } {
            WackasmOperator::Nop => become nop(ctx, ip, stack, fuel),

        }
    }};
}

fn nop(ctx: &mut DispatchCtx, ip: *const WackasmOperator, stack: *mut Value, fuel: usize) -> Exit {
    unsafe { dispatch!(ctx, ip.add(1), stack, fuel - 1) }
}