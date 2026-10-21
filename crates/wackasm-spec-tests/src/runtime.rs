use std::{
    any::Any,
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
};

use wackasm::{Value, WackasmExecutor, WackasmModule};

use crate::values::convert_arg;

#[derive(Default)]
pub(super) struct ScriptRunner {
    pub(super) definitions: HashMap<String, Vec<u8>>,
    instances: HashMap<String, WackasmExecutor>,
    registrations: HashMap<String, String>,
    pub(super) current_definition: Option<String>,
    current_instance: Option<String>,
    anonymous_index: usize,
}

impl ScriptRunner {
    pub(super) fn next_name(&mut self, prefix: &str) -> String {
        let name = format!("{prefix}{}", self.anonymous_index);
        self.anonymous_index += 1;
        name
    }

    pub(super) fn definition_name(&mut self, name: Option<String>) -> String {
        name.unwrap_or_else(|| self.next_name("module-"))
    }

    pub(super) fn install(&mut self, name: String, bytes: Vec<u8>) -> Result<(), String> {
        let module = catch_unwind(AssertUnwindSafe(|| WackasmModule::new(&bytes)))
            .map_err(runtime_panic)?
            .map_err(|error| error.to_string())?;
        self.definitions.insert(name.clone(), bytes);
        self.instances
            .insert(name.clone(), WackasmExecutor::new(module).map_err(|e| e.to_string())?);
        self.current_definition = Some(name.clone());
        self.current_instance = Some(name);
        Ok(())
    }

    pub(super) fn instantiate(&mut self, definition: &str, instance: String) -> Result<(), String> {
        let bytes = self
            .definitions
            .get(definition)
            .ok_or_else(|| format!("unknown module definition {definition}"))?;
        let module = catch_unwind(AssertUnwindSafe(|| WackasmModule::new(bytes)))
            .map_err(runtime_panic)?
            .map_err(|error| error.to_string())?;
        self.instances
            .insert(instance.clone(), WackasmExecutor::new(module).map_err(|e| e.to_string())?);
        self.current_definition = Some(definition.to_owned());
        self.current_instance = Some(instance);
        Ok(())
    }

    pub(super) fn instance_name(&self, name: Option<&str>) -> Result<String, String> {
        if let Some(name) = name {
            let instance = self
                .registrations
                .get(name)
                .map(String::as_str)
                .unwrap_or(name);
            if self.instances.contains_key(instance) {
                return Ok(instance.to_owned());
            }
            return Err(format!("unknown module instance {name}"));
        }
        self.current_instance
            .clone()
            .ok_or_else(|| "no module instance is active".to_owned())
    }

    pub(super) fn register(&mut self, name: String, instance: String) {
        self.registrations.insert(name, instance);
    }

    pub(super) fn invoke(
        &mut self,
        module: Option<&str>,
        export: &str,
        args: &[wast::WastArg<'_>],
    ) -> Result<Vec<Value>, String> {
        let name = self.instance_name(module)?;
        let args = args
            .iter()
            .map(convert_arg)
            .collect::<Result<Vec<_>, _>>()?;
        let executor = self
            .instances
            .get_mut(&name)
            .ok_or_else(|| format!("unknown module instance {name}"))?;
        catch_unwind(AssertUnwindSafe(|| {
            executor
                .call_export(export, args)
                .map_err(|error| error.to_string())?;
            executor.execute().map_err(|error| error.to_string())?;
            executor
                .get_result()
                .cloned()
                .ok_or_else(|| "function completed without a result".to_owned())
        }))
        .map_err(runtime_panic)?
    }
}

pub(super) enum RuntimeError {
    Failed(String),
    Unsupported(String),
    Trap(String),
}

pub(super) fn classify_runtime_error(error: String) -> crate::Outcome {
    if error.starts_with("runtime panicked:")
        || error.contains("unsupported section")
        || error.contains("unsupported operator")
        || error.contains("not implemented")
    {
        crate::Outcome::Unsupported(error)
    } else {
        crate::Outcome::Failed(error)
    }
}

fn runtime_panic(payload: Box<dyn Any + Send>) -> String {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic");
    format!("runtime panicked: {message}")
}

pub(super) fn is_trap_error(error: &str) -> bool {
    [
        "divide by 0",
        "out of bounds",
        "address overflow",
        "unreachable",
        "integer overflow",
        "invalid conversion to integer",
    ]
    .iter()
    .any(|message| error.contains(message))
}
