use crate::{
    HaltError, Value,
    imports::{WackasmImports, WackasmLinker, WackasmLinkerResources, WackasmResource},
};

pub trait WackasmValue<R: WackasmResource>: Sized {
    fn into_wasm_value(self) -> Value;
    type Out<'a>
    where
        R: 'a;
    fn from_wasm_value<'a>(
        ctx: &'a WackasmLinkerResources<R>,
        value: Value,
    ) -> Result<Self::Out<'a>, HaltError>;
}

impl<R: WackasmResource> WackasmValue<R> for i32 {
    fn into_wasm_value(self) -> Value {
        Value(self as u64)
    }

    type Out<'a> = i32
    where
        R: 'a;

    fn from_wasm_value<'a>(
        ctx: &'a WackasmLinkerResources<R>,
        value: Value,
    ) -> Result<Self::Out<'a>, HaltError> {
        Ok(value.as_i32())
    }
}