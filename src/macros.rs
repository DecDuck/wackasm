/// Pops `rhs` then `lhs` as i32 (cast to `$ty`), evaluates `$body`, and pushes
/// the result back as an i32. Works for arithmetic (`$ty` result) and
/// comparisons (`bool` result).
///
/// ```ignore
/// i32_binop!(stack, i32, |lhs, rhs| lhs.wrapping_add(rhs));
/// i32_binop!(stack, u32, |lhs, rhs| lhs < rhs);
/// ```
macro_rules! i32_binop {
    ($stack:expr, $ty:ty, |$lhs:ident, $rhs:ident| $body:expr) => {{
        use $crate::stack::OperandStackFns as _;
        let stack = &mut $stack;
        let $rhs = stack.pop_i32()? as $ty;
        let $lhs = stack.pop_i32()? as $ty;
        stack.push($crate::Value::I32(($body) as i32))?;
    }};
}

pub(crate) use i32_binop;

/// Pops an address, loads `size_of::<$width>()` bytes from memory, and pushes the i32.
///
/// ```ignore
/// i32_load!(stack, memories, memarg, u8);
/// ```
macro_rules! i32_load {
    ($stack:expr, $memories:expr, $memarg:expr, $width:ty) => {{
        use $crate::stack::OperandStackFns as _;
        let stack = &mut $stack;
        let address = stack.pop_i32()? as u32;
        let value = $memories.load_i32::<{ ::core::mem::size_of::<$width>() }>($memarg, address)?;
        stack.push($crate::Value::I32(value))?;
    }};
}

pub(crate) use i32_load;

/// Pops a value then an address, and stores the low `size_of::<$width>()` bytes.
///
/// ```ignore
/// i32_store!(stack, memories, memarg, u8);
/// ```
macro_rules! i32_store {
    ($stack:expr, $memories:expr, $memarg:expr, $width:ty) => {{
        use $crate::stack::OperandStackFns as _;
        let stack = &mut $stack;
        let value = stack.pop_i32()?;
        let address = stack.pop_i32()? as u32;
        $memories.store_i32::<{ ::core::mem::size_of::<$width>() }>($memarg, address, value)?;
    }};
}

pub(crate) use i32_store;
