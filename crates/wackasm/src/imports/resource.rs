use anyhow::anyhow;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    HaltError, Value,
    imports::{WackasmImports, WackasmLinker, WackasmLinkerResources, WackasmValue},
};

/// A rebuildable handle to some external resource,
/// to allow host-specific resources to survive serialization.
pub trait WackasmResource: Serialize + DeserializeOwned {
    /// MUST return the same value across serialization and deserialization
    fn opaque_handle(&self) -> Value;

    /// Run after deserialization, useful if you need to re-open some
    /// sockets or something
    fn reactivate(self) -> Result<Self, anyhow::Error> {
        Ok(self)
    }
}

#[derive(Serialize, Deserialize)]
pub enum NoResources {}
impl WackasmResource for NoResources {
    fn opaque_handle(&self) -> Value {
        match *self {}
    }
}

impl<R: WackasmResource> WackasmValue<R> for &R {
    fn into_wasm_value(self) -> Value {
        self.opaque_handle()
    }

    type Out<'a>
        = &'a R
    where
        R: 'a;

    fn from_wasm_value<'a>(ctx: &'a WackasmLinkerResources<R>, value: Value) -> Result<Self::Out<'a>, HaltError> {
        Ok(ctx
            .get_opaque_resource(&value)
            .ok_or_else(|| anyhow!("persistent resource missing"))?)
    }
}
