mod module;
mod resource;
mod value;

use std::{collections::HashMap, fmt::Arguments};

use anyhow::anyhow;
pub use module::*;
pub use resource::*;
use serde::{Deserialize, Deserializer, Serialize};
pub use value::*;

use crate::{HaltError, Value};
#[derive(Serialize, Deserialize)]
pub struct WackasmLinkerResources<R: WackasmResource> {
    #[serde(deserialize_with = "deserialize_reactivated")]
    resources: HashMap<Value, R>,
}

#[derive(Serialize, Deserialize)]
pub struct WackasmLinker<Ctx, I: WackasmImports<Ctx, R>, R: WackasmResource> {
    ctx: Ctx,
    imports: I,
    resources: WackasmLinkerResources<R>,
}

impl<R: WackasmResource> WackasmLinkerResources<R> {
    pub(crate) fn get_opaque_resource<'a>(&'a self, key: &Value) -> Option<&'a R> {
        self.resources.get(key)
    }

    pub(crate) fn append_opaque_resource(&mut self, resource: R) {
        self.resources.insert(resource.opaque_handle(), resource);
    }
}

impl<Ctx, I: WackasmImports<Ctx, R>, R: WackasmResource> WackasmLinker<Ctx, I, R> {
    pub fn new(ctx: Ctx, imports: I) -> Self {
        Self {
            ctx,
            imports,
            resources: WackasmLinkerResources {
                resources: HashMap::new(),
            },
        }
    }

    pub(crate) fn get_resources_handle(&mut self) -> &mut WackasmLinkerResources<R> {
        &mut self.resources
    }

    pub(crate) fn call_untyped(
        &mut self,
        module: &str,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, HaltError> {
        let func = self
            .imports
            .get_import_func(module, name)
            .ok_or_else(|| anyhow!("{}.{} not found", module, name))?;
        let result = func.call(&mut self.ctx, &self.resources, args)?;
        Ok(result)
    }
}

fn deserialize_reactivated<'de, D, R>(d: D) -> Result<HashMap<Value, R>, D::Error>
where
    D: Deserializer<'de>,
    R: WackasmResource,
{
    use serde::de::Error;

    HashMap::<Value, R>::deserialize(d)?
        .into_iter()
        .map(|(k, v)| {
            v.reactivate()
                .map(|v| (k, v))
                .map_err(|e| D::Error::custom(format_args!("failed to reactivate resource: {e:#}")))
        })
        .collect()
}
