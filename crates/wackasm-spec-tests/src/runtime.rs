use std::{
    any::Any,
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
};

use wackasm::{HaltError, Value, WackasmExecutor, WackasmModule};

use crate::values::convert_args;

#[derive(Default)]
pub(super) struct ScriptRunner {
    pub(super) definitions: HashMap<String, Vec<u8>>,
    instances: HashMap<String, WackasmExecutor>,
    /// Instances whose module failed to load, mapped to the load error.
    unavailable: HashMap<String, RuntimeError>,
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

    pub(super) fn install(&mut self, name: String, bytes: Vec<u8>) -> Result<(), RuntimeError> {
        let executor = match load_instance(&bytes) {
            Ok(executor) => executor,
            Err(error) => return Err(self.mark_unavailable(name, error)),
        };
        self.unavailable.remove(&name);
        self.definitions.insert(name.clone(), bytes);
        self.instances.insert(name.clone(), executor);
        self.current_definition = Some(name.clone());
        self.current_instance = Some(name);
        Ok(())
    }

    pub(super) fn instantiate(
        &mut self,
        definition: &str,
        instance: String,
    ) -> Result<(), RuntimeError> {
        let bytes = self.definitions.get(definition).ok_or_else(|| {
            RuntimeError::Failed(format!("unknown module definition {definition}"))
        })?;
        let executor = match load_instance(bytes) {
            Ok(executor) => executor,
            Err(error) => return Err(self.mark_unavailable(instance, error)),
        };
        self.unavailable.remove(&instance);
        self.instances.insert(instance.clone(), executor);
        self.current_definition = Some(definition.to_owned());
        self.current_instance = Some(instance);
        Ok(())
    }

    /// Makes `name` the active instance so later directives report the load failure
    /// instead of silently running against the previous module.
    fn mark_unavailable(&mut self, name: String, error: RuntimeError) -> RuntimeError {
        self.instances.remove(&name);
        self.unavailable.insert(name.clone(), error.clone());
        self.current_instance = Some(name);
        error
    }

    pub(super) fn instance_name(&self, name: Option<&str>) -> Result<String, RuntimeError> {
        let instance = match name {
            Some(name) => self
                .registrations
                .get(name)
                .map(String::as_str)
                .unwrap_or(name),
            None => self
                .current_instance
                .as_deref()
                .ok_or_else(|| RuntimeError::Failed("no module instance is active".to_owned()))?,
        };
        if let Some(error) = self.unavailable.get(instance) {
            let message = format!("module instance {instance} failed to load: {}", error.message());
            // A trap while loading must not satisfy a later assert_trap against this instance.
            return Err(match error {
                RuntimeError::Unsupported(_) => RuntimeError::Unsupported(message),
                RuntimeError::Failed(_) | RuntimeError::Trap(_) => RuntimeError::Failed(message),
            });
        }
        if self.instances.contains_key(instance) {
            return Ok(instance.to_owned());
        }
        Err(RuntimeError::Failed(format!(
            "unknown module instance {instance}"
        )))
    }

    pub(super) fn register(&mut self, name: String, instance: String) {
        self.registrations.insert(name, instance);
    }

    pub(super) fn invoke(
        &mut self,
        module: Option<&str>,
        export: &str,
        args: &[wast::WastArg<'_>],
    ) -> Result<Vec<Value>, RuntimeError> {
        let name = self.instance_name(module)?;
        let args = convert_args(args).map_err(RuntimeError::Unsupported)?;
        let executor = self
            .instances
            .get_mut(&name)
            .ok_or_else(|| RuntimeError::Failed(format!("unknown module instance {name}")))?;
        catch_unwind(AssertUnwindSafe(|| {
            executor.call_export(export, args)?;
            executor.execute()?;
            executor.get_result().cloned().ok_or_else(|| {
                RuntimeError::Failed("function completed without a result".to_owned())
            })
        }))
        .map_err(runtime_panic)?
    }
}

#[derive(Clone, Debug)]
pub(super) enum RuntimeError {
    Failed(String),
    Unsupported(String),
    Trap(String),
}

impl RuntimeError {
    pub(super) fn message(&self) -> &str {
        match self {
            Self::Failed(message) | Self::Unsupported(message) | Self::Trap(message) => message,
        }
    }

    pub(super) fn into_message(self) -> String {
        match self {
            Self::Failed(message) | Self::Unsupported(message) | Self::Trap(message) => message,
        }
    }
}

impl From<HaltError> for RuntimeError {
    fn from(error: HaltError) -> Self {
        let message = error.to_string();
        match error {
            HaltError::Trap(_) => Self::Trap(message),
            HaltError::Unsupported(_) => Self::Unsupported(message),
            HaltError::Error(_) => Self::Failed(message),
        }
    }
}

/// Outcome for directives that are expected to succeed, so traps count as failures.
pub(super) fn classify_runtime_error(error: RuntimeError) -> crate::Outcome {
    match error {
        RuntimeError::Failed(error) => crate::Outcome::Failed(error),
        RuntimeError::Unsupported(error) => crate::Outcome::Unsupported(error),
        RuntimeError::Trap(error) => crate::Outcome::Failed(format!("unexpected trap: {error}")),
    }
}

fn load_instance(bytes: &[u8]) -> Result<WackasmExecutor, RuntimeError> {
    catch_unwind(AssertUnwindSafe(|| {
        let module = WackasmModule::new(bytes)?;
        Ok(WackasmExecutor::new(module)?)
    }))
    .map_err(runtime_panic)?
}

/// Panics come from unimplemented paths (`todo!()` and friends), so they count as unsupported.
fn runtime_panic(payload: Box<dyn Any + Send>) -> RuntimeError {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic");
    RuntimeError::Unsupported(format!("runtime panicked: {message}"))
}
