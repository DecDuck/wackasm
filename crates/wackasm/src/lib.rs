#![feature(vec_peek_mut)]
#![feature(option_reference_flattening)]
#![feature(explicit_tail_calls)]
#![feature(try_trait_v2)]
#![feature(macro_metavar_expr)]

mod blocks;
mod compiler;
mod const_expr;
mod executor;
mod functions;
mod globals;
mod macros;
mod memory;
mod module;
mod pc;
mod stack;
mod table;
mod dispatch;
pub mod imports;
#[macro_use]
mod dispatch_macros;

#[cfg(test)]
mod tests;

pub use executor::WackasmExecutor;
pub use module::WackasmModule;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Value(pub u64);

impl Value {
    pub const NULL_REF: Value = Value(u64::MAX);

    #[inline]
    pub const fn from_i32(value: i32) -> Self {
        Self(value as u64)
    }

    #[inline]
    pub const fn from_i64(value: i64) -> Self {
        Self(value as u64)
    }

    #[inline]
    pub const fn from_f32(value: f32) -> Self {
        Self(value.to_bits() as u64)
    }

    #[inline]
    pub const fn from_f64(value: f64) -> Self {
        Self(value.to_bits())
    }

    #[inline]
    pub const fn as_i32(self) -> i32 {
        self.0 as i32
    }

    #[inline]
    pub const fn as_i64(self) -> i64 {
        self.0 as i64
    }

    #[inline]
    pub const fn as_f32(self) -> f32 {
        f32::from_bits(self.0 as u32)
    }

    #[inline]
    pub const fn as_f64(self) -> f64 {
        f64::from_bits(self.0)
    }

    #[inline]
    pub const fn is_null_ref(self) -> bool {
        self.0 == Self::NULL_REF.0
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct InlineWideValue(Value, Option<Value>);

impl InlineWideValue {
    pub fn new(value: Value) -> Self {
        Self(value, None)
    }

    pub fn set(&mut self, value: Value) {
        self.0 = value;
        self.1 = None;
    }
    pub fn set_wide(&mut self, value: u128) {
        let high = (value >> 64) as u64;
        let low = value as u64;
        self.0 = Value(high);
        self.1 = Some(Value(low));
    }

    pub fn get(&self) -> &Value {
        &self.0
    }
    pub fn get_wide(&self) -> Option<u128> {
        let high = self.0.0;
        let low = self.1?.0;
        let combined = ((high as u128) << 64) | (low as u128);
        Some(combined)
    }

    pub fn into_value_checked(self) -> Option<Value> {
        if self.1.is_some() {
            return None;
        }
        Some(self.0)
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
