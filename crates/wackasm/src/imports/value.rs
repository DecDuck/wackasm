use crate::{
    HaltError, Value,
    imports::{WackasmLinkerResources, WackasmResource},
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

macro_rules! impl_wackasm_value {
    ($($ty:ident),*) => {
        $(
            impl<R: WackasmResource> WackasmValue<R> for $ty {
                fn into_wasm_value(self) -> Value {
                    Value(self as u64)
                }

                type Out<'a> = $ty
                where
                    R: 'a;

                    fn from_wasm_value<'a>(
                        _ctx: &'a WackasmLinkerResources<R>,
                        value: Value,
                    ) -> Result<Self::Out<'a>, HaltError> {
                        Ok(value.0 as $ty)
                    }
            }
        )*
    };
}

impl_wackasm_value!(i8, u8, i16, u16, i32, i64, u32, u64, usize, isize);
