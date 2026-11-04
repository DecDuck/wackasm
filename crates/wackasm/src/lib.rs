#![feature(vec_peek_mut)]
#![feature(option_reference_flattening)]

mod blocks;
mod const_expr;
mod executor;
mod globals;
mod macros;
mod memory;
mod module;
mod operator;
mod pc;
mod stack;
mod table;
pub use executor::WackasmExecutor;
pub use module::WackasmModule;
use serde::{Deserialize, Serialize};

use crate::module::FunctionIndex;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Value {
    /// The value type is i32.
    I32(i32),
    /// The value type is i64.
    I64(i64),
    /// The value type is f32.
    F32(f32),
    /// The value type is f64.
    F64(f64),
    /// The value type is v128.
    // V128(u128),
    /// The value type is a reference.
    FuncRef(FunctionIndex),
    Unknown,
}
