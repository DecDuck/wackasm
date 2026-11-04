#![feature(vec_peek_mut)]

mod module;
mod executor;
mod stack;
mod blocks;
mod operator;
mod pc;
pub use module::WackasmModule;
pub use executor::WackasmExecutor;
use serde::{Deserialize, Serialize};

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
    V128(u128),
    /// The value type is a reference.
    Ref(u32),
}
