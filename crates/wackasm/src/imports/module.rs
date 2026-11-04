use std::marker::PhantomData;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    HaltError, Value, WackasmExecutor,
    imports::{WackasmLinkerResources, WackasmResource, WackasmValue},
};
use anyhow::anyhow;
use paste::paste;

pub trait WackasmImports<Ctx, R: WackasmResource>: Serialize + DeserializeOwned + Clone {
    fn get_import_func(
        &self,
        module: &str,
        name: &str,
    ) -> Option<Box<dyn WackasmUntypedCall<Ctx, R>>>;
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NoWackasmImports {}
impl<R: WackasmResource> WackasmImports<(), R> for NoWackasmImports {
    fn get_import_func(
        &self,
        module: &str,
        name: &str,
    ) -> Option<Box<dyn WackasmUntypedCall<(), R>>> {
        None
    }
}

pub trait IntoWackasmCall<Ctx, R: WackasmResource, Marker> {
    fn into_call(self) -> Box<dyn WackasmUntypedCall<Ctx, R>>;
}
pub trait WackasmUntypedCall<Ctx, R: WackasmResource> {
    fn call(
        &self,
        ctx: &mut Ctx,
        executor: &mut WackasmExecutor,
        resources: &WackasmLinkerResources<R>,
        arguments: Vec<Value>,
    ) -> Result<Vec<Value>, HaltError>;
}

macro_rules! impl_typed_fn {
    ($($P:ident),* ; $($R:ident),*) => {
        paste! {
            pub struct [<TypedWackasmFn $($P)* $($R)*>]<Ctx, F, R, $($P,)* $($R,)*>
            where
                F: for<'a> Fn(&mut Ctx, &mut WackasmExecutor, $($P::Out<'a>),*) -> Result<($($R,)*), HaltError>,
                R: WackasmResource,
                $( $P: WackasmValue<R>, )*
                $( $R: WackasmValue<R>, )*
            {
                pub f: F,
                _marker: PhantomData<(for<'a> fn(&mut Ctx, $($P::Out<'a>),*) -> ($($R,)*), R)>,
            }

            impl<Ctx, F, R, $($P,)* $($R,)*> WackasmUntypedCall<Ctx, R> for [<TypedWackasmFn $($P)* $($R)*>]<Ctx, F, R, $($P,)* $($R,)*>
            where
                F: for<'a> Fn(&mut Ctx, &mut WackasmExecutor, $($P::Out<'a>),*) -> Result<($($R,)*), HaltError>,
                R: WackasmResource,
                $( $P: WackasmValue<R>, )*
                $( $R: WackasmValue<R>, )*
            {
                fn call(
                        &self,
                        ctx: &mut Ctx,
                        executor: &mut WackasmExecutor,
                        _resources: &WackasmLinkerResources<R>,
                        _arguments: Vec<Value>,
                    ) -> Result<Vec<Value>, HaltError> {
                    $(
                    let [<$P:lower>] = _arguments.get(${index()}).ok_or_else(|| anyhow!("invalid arguments"))?;
                    let [<$P:lower>] = $P::from_wasm_value(_resources, *[<$P:lower>])?;
                    )*

                    let _result = (self.f)(ctx, executor, $( [<$P:lower>] ),*)?;

                    $(
                    let [<$R:lower>] = _result.${index()};
                    let [<$R:lower>] = $R::into_wasm_value([<$R:lower>]);
                    )*

                    Ok(vec![ $([<$R:lower>]),* ])
                }
            }

            impl<Ctx: 'static, R: 'static, F, $($P: 'static,)* $($R: 'static,)*> IntoWackasmCall<Ctx, R, ( ($($P,)*), ($($R,)*) )> for F
            where
                F: Fn(&mut Ctx, &mut WackasmExecutor, $($P),*) -> Result<($($R,)*), HaltError>
                    + for<'a> Fn(&mut Ctx, &mut WackasmExecutor, $($P::Out<'a>),*) -> Result<($($R,)*), HaltError>
                    + 'static,
                R: WackasmResource,
                $( $P: WackasmValue<R>, )*
                $( $R: WackasmValue<R>, )*
            {
                fn into_call(self) -> Box<dyn WackasmUntypedCall<Ctx, R>> {
                    Box::new([<TypedWackasmFn $($P)* $($R)*>]::<Ctx, F, R, $($P,)* $($R,)*> {
                        f: self,
                        _marker: PhantomData
                    })
                }
            }
        }
    };
}

macro_rules! impl_typed_fn_returns {
    ([$($P:ident),*]; [$($R:ident),*]) => {
        impl_typed_fn!($($P),* ; $($R),*);
    };
    ([$($P:ident),*]; [$($R:ident),*] $next:ident $($rest:ident)*) => {
        impl_typed_fn!($($P),* ; $($R),*);
        impl_typed_fn_returns!([$($P),*]; [$($R,)* $next] $($rest)*);
    };
}

// Growing params; for each param prefix, run through every return prefix.
macro_rules! impl_typed_fn_matrix {
    ([$($P:ident),*] [] ; $($R:ident)*) => {
        impl_typed_fn_returns!([$($P),*]; [] $($R)*);
    };
    ([$($P:ident),*] [$next:ident $($prest:ident)*] ; $($R:ident)*) => {
        impl_typed_fn_returns!([$($P),*]; [] $($R)*);
        impl_typed_fn_matrix!([$($P,)* $next] [$($prest)*] ; $($R)*);
    };
}

impl_typed_fn_matrix!(
    [] [P1 P2 P3 P4 P5 P6 P7 P8 P9 P10 P11 P12 P13 P14 P15 P16] ;
    R1 R2 R3 R4 R5 R6 R7 R8 R9 R10 R11 R12 R13 R14 R15 R16
);