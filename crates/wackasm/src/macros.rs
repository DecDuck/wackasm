/// Builds a [`HaltError::Trap`](crate::HaltError::Trap) for WebAssembly-defined
/// runtime traps (out of bounds accesses, division by zero, etc).
macro_rules! trap {
    ($($arg:tt)*) => {
        $crate::HaltError::Trap(::anyhow::anyhow!($($arg)*))
    };
}

pub(crate) use trap;

/// Builds a [`HaltError::Unsupported`](crate::HaltError::Unsupported) for valid
/// WebAssembly that wackasm does not implement yet.
macro_rules! unsupported {
    ($($arg:tt)*) => {
        $crate::HaltError::Unsupported(::anyhow::anyhow!($($arg)*))
    };
}

pub(crate) use unsupported;

/// Pops `rhs`, reads `lhs` in-place from the top of the stack (both as i32,
/// cast to `$ty`), evaluates `$body`, and overwrites `lhs` with the result as an
/// i32. Works for arithmetic (`$ty` result) and comparisons (`bool` result).
///
/// ```ignore
/// i32_binop!(stack, i32, |lhs, rhs| lhs.wrapping_add(rhs));
/// i32_binop!(stack, u32, |lhs, rhs| lhs < rhs);
/// ```
macro_rules! i32_binop {
    ($stack:expr, $ty:ty, |$lhs:ident, $rhs:ident| $body:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let $rhs = stack.pop_i32()? as $ty;
        let slot = stack.peek_mut()?;
        let $lhs = slot.0 as i32 as $ty;
        *slot = $crate::Value(($body) as u64);
    }};
}

pub(crate) use i32_binop;

/// Like [`i32_binop`] but for i64 operands, pushing an i64 result.
///
/// ```ignore
/// i64_binop!(stack, i64, |lhs, rhs| lhs.wrapping_add(rhs));
/// ```
macro_rules! i64_binop {
    ($stack:expr, $ty:ty, |$lhs:ident, $rhs:ident| $body:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let $rhs = stack.pop_i64()? as $ty;
        let slot = stack.peek_mut()?;
        let $lhs = slot.0 as i64 as $ty;
        *slot = $crate::Value(($body) as u64);
    }};
}

pub(crate) use i64_binop;

/// i64 comparison; wasm pushes the result as an i32.
///
/// ```ignore
/// i64_cmp!(stack, u64, |lhs, rhs| lhs < rhs);
/// ```
macro_rules! i64_cmp {
    ($stack:expr, $ty:ty, |$lhs:ident, $rhs:ident| $body:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let $rhs = stack.pop_i64()? as $ty;
        let slot = stack.peek_mut()?;
        let $lhs = slot.0 as i64 as $ty;
        *slot = $crate::Value(($body) as u64);
    }};
}

pub(crate) use i64_cmp;

/// Pops an address, loads `size_of::<$width>()` bytes, and extends them to i32.
///
/// ```ignore
/// i32_load!(stack, memories, memarg, u8);
/// ```
macro_rules! i32_load {
    ($stack:expr, $memories:expr, $memarg:expr, $width:ty) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let address = stack.pop_i32()? as u32;
        let value: $width = $memories.load($memarg, address)?;
        stack.push($crate::Value(value as u64))?;
    }};
}

pub(crate) use i32_load;

/// Pops an address, loads `size_of::<$width>()` bytes, and extends them to i64.
///
/// ```ignore
/// i64_load!(stack, memories, memarg, u8);
/// ```
macro_rules! i64_load {
    ($stack:expr, $memories:expr, $memarg:expr, $width:ty) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let address = stack.pop_i32()? as u32;
        let value: $width = $memories.load($memarg, address)?;
        stack.push($crate::Value(value as u64))?;
    }};
}

pub(crate) use i64_load;

macro_rules! f32_load {
    ($stack:expr, $memories:expr, $memarg:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let address = stack.pop_i32()? as u32;
        let value: f32 = $memories.load($memarg, address)?;
        stack.push($crate::Value::from_f32(value))?;
    }};
}

pub(crate) use f32_load;

macro_rules! f64_load {
    ($stack:expr, $memories:expr, $memarg:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let address = stack.pop_i32()? as u32;
        let value: f64 = $memories.load($memarg, address)?;
        stack.push($crate::Value::from_f64(value))?;
    }};
}

pub(crate) use f64_load;

macro_rules! v128_load {
    ($stack:expr, $memories:expr, $memarg:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let address = stack.pop_i32()? as u32;
        let value: u128 = $memories.load($memarg, address)?;
        stack.push_v128(value)?;
    }};
}

pub(crate) use v128_load;

/// Pops a value then an address, and stores the low `size_of::<$width>()` bytes.
///
/// ```ignore
/// i32_store!(stack, memories, memarg, u8);
/// ```
macro_rules! i32_store {
    ($stack:expr, $memories:expr, $memarg:expr, $width:ty) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let value = stack.pop_i32()?;
        let address = stack.pop_i32()? as u32;
        $memories.store($memarg, address, value as $width)?;
    }};
}

pub(crate) use i32_store;

/// Like [`i32_store`] but pops an i64 value.
///
/// ```ignore
/// i64_store!(stack, memories, memarg, u32);
/// ```
macro_rules! i64_store {
    ($stack:expr, $memories:expr, $memarg:expr, $width:ty) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let value = stack.pop_i64()?;
        let address = stack.pop_i32()? as u32;
        $memories.store($memarg, address, value as $width)?;
    }};
}

pub(crate) use i64_store;

macro_rules! f32_store {
    ($stack:expr, $memories:expr, $memarg:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let value = stack.pop_f32()?;
        let address = stack.pop_i32()? as u32;
        $memories.store($memarg, address, value)?;
    }};
}

pub(crate) use f32_store;

macro_rules! f64_store {
    ($stack:expr, $memories:expr, $memarg:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let value = stack.pop_f64()?;
        let address = stack.pop_i32()? as u32;
        $memories.store($memarg, address, value)?;
    }};
}

pub(crate) use f64_store;

macro_rules! v128_store {
    ($stack:expr, $memories:expr, $memarg:expr) => {{
        use $crate::stack::GlobalStack as _;
        let stack = &mut $stack;
        let value = stack.pop_v128()?;
        let address = stack.pop_i32()? as u32;
        $memories.store($memarg, address, value)?;
    }};
}

pub(crate) use v128_store;
