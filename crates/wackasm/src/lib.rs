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
use thiserror::Error;

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
    V128(u128),
    /// The value type is a reference.
    FuncRef(FunctionIndex),
    Unknown,
}

impl Value {
    /// Returns whether this value contains a non-null function reference.
    pub fn is_func_ref(&self) -> bool {
        matches!(self, Self::FuncRef(_))
    }
}

#[derive(Debug, Error)]
pub enum HaltError {
    #[error("trap triggered: {0}")]
    Trap(anyhow::Error),
    #[error("executor error: {0}")]
    Error(anyhow::Error),
    #[error("unsupported: {0}")]
    Unsupported(anyhow::Error),
}

impl From<anyhow::Error> for HaltError {
    fn from(value: anyhow::Error) -> Self {
        HaltError::Error(value)
    }
}

impl From<wasmparser::BinaryReaderError> for HaltError {
    fn from(value: wasmparser::BinaryReaderError) -> Self {
        HaltError::Error(value.into())
    }
}

impl From<std::num::TryFromIntError> for HaltError {
    fn from(value: std::num::TryFromIntError) -> Self {
        HaltError::Error(value.into())
    }
}