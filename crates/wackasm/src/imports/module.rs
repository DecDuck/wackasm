use std::marker::PhantomData;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    HaltError, Value,
    imports::{WackasmLinkerResources, WackasmResource, WackasmValue},
};
use anyhow::anyhow;
use paste::paste;

pub trait WackasmImports<Ctx, R: WackasmResource>: Serialize + DeserializeOwned {
    fn get_import_func(
        &self,
        module: &str,
        name: &str,
    ) -> Option<Box<dyn WackasmUntypedCall<Ctx, R>>>;
}

#[derive(Serialize, Deserialize)]
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
        resources: &WackasmLinkerResources<R>,
        arguments: Vec<Value>,
    ) -> Result<Vec<Value>, HaltError>;
}

macro_rules! impl_typed_fn {
    ($($P:ident),* ; $($R:ident),*) => {
        paste! {
            pub struct [<TypedWackasmFn $($P)* $($R)*>]<Ctx, F, R, $($P),* $($R,)*>
            where
                F: for<'a> Fn(&mut Ctx, $($P::Out<'a>),*) -> ($($R,)*),
                R: WackasmResource,
                $( $P: WackasmValue<R>, )*
                $( $R: WackasmValue<R>, )*
            {
                pub f: F,
                _marker: PhantomData<(for<'a> fn(&mut Ctx, $($P::Out<'a>),*) -> ($($R,)*), R)>,
            }

            impl<Ctx, F, R, $($P),* $($R,)*> WackasmUntypedCall<Ctx, R> for [<TypedWackasmFn $($P)* $($R)*>]<Ctx, F, R, $($P),* $($R,)*>
            where
                F: for<'a> Fn(&mut Ctx, $($P::Out<'a>),*) -> ($($R,)*),
                R: WackasmResource,
                $( $P: WackasmValue<R>, )*
                $( $R: WackasmValue<R>, )*
            {
                fn call(
                        &self,
                        ctx: &mut Ctx,
                        _resources: &WackasmLinkerResources<R>,
                        _arguments: Vec<Value>,
                    ) -> Result<Vec<Value>, HaltError> {
                    $(
                    let [<$P:lower>] = _arguments.get(${index()}).ok_or_else(|| anyhow!("invalid arguments"))?;
                    let [<$P:lower>] = $P::from_wasm_value(_resources, *[<$P:lower>])?;
                    )*

                    let _result = (self.f)(ctx, $( [<$P:lower>] )*);

                    $(
                    let [<$R:lower>] = _result.${index()};
                    let [<$R:lower>] = $R::into_wasm_value([<$R:lower>]);
                    )*

                    Ok(vec![ $([<$R:lower>])* ])
                }
            }

            impl<Ctx: 'static, R: 'static, F, $($P: 'static),* $($R: 'static)*> IntoWackasmCall<Ctx, R, ( ($($P,)*), ($($R,)*) )> for F
            where
                F: Fn(&mut Ctx, $($P),*) -> ($($R,)*)
                    + for<'a> Fn(&mut Ctx, $($P::Out<'a>),*) -> ($($R,)*)
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

impl_typed_fn!(P1;);
impl_typed_fn!(;R1);
