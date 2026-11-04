use crate::{
    HaltError, Value,
    imports::{WackasmLinkerResources, WackasmResource},
};
use paste::paste;

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

macro_rules! impl_wackasm_value {
    ($($ty:ident),*) => {
        paste! {
            $(
                impl<R: WackasmResource> WackasmValue<R> for $ty {
                    type Out<'a> = $ty where R: 'a;

                    fn into_wasm_value(self) -> Value {
                        Value::[<from_ $ty>](self)
                    }

                    fn from_wasm_value<'a>(
                        _ctx: &'a WackasmLinkerResources<R>,
                        value: Value,
                    ) -> Result<Self::Out<'a>, HaltError> {
                        Ok(value.[<as_ $ty>]())
                    }
                }
            )*
        }
    };
}

impl_wackasm_value!(i32, i64, f32, f64, u32, u64);