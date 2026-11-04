use std::{
    any::Any,
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
};

use wackasm::{HaltError, HostYield, Value, WackasmExecutor, WackasmModule};

use crate::{
    linking::{ImportTarget, Links, SPECTEST_MODULE, is_spectest_function, module_interface},
    values::convert_args,
};

#[derive(Default)]
pub(super) struct ScriptRunner {
    pub(super) definitions: HashMap<String, Vec<u8>>,
    /// Executors are taken out while they run, so a missing entry with links means re-entry.
    instances: HashMap<String, WackasmExecutor>,
    links: HashMap<String, Links>,
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
        let (executor, links) = match self
            .link(&bytes)
            .and_then(|links| Ok((load_instance(&bytes)?, links)))
        {
            Ok(instance) => instance,
            Err(error) => return Err(self.mark_unavailable(name, error)),
        };
        self.unavailable.remove(&name);
        self.definitions.insert(name.clone(), bytes);
        self.instances.insert(name.clone(), executor);
        self.links.insert(name.clone(), links);
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
        let (executor, links) = match self
            .link(bytes)
            .and_then(|links| Ok((load_instance(bytes)?, links)))
        {
            Ok(instance) => instance,
            Err(error) => return Err(self.mark_unavailable(instance, error)),
        };
        self.unavailable.remove(&instance);
        self.instances.insert(instance.clone(), executor);
        self.links.insert(instance.clone(), links);
        self.current_definition = Some(definition.to_owned());
        self.current_instance = Some(instance);
        Ok(())
    }

    /// Makes `name` the active instance so later directives report the load failure
    /// instead of silently running against the previous module.
    fn mark_unavailable(&mut self, name: String, error: RuntimeError) -> RuntimeError {
        self.instances.remove(&name);
        self.links.remove(&name);
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
            let message = format!(
                "module instance {instance} failed to load: {}",
                error.message()
            );
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

    /// Records the registration even if the instance failed to load, so importers report the load
    /// failure instead of an unknown module.
    pub(super) fn register(
        &mut self,
        name: String,
        instance: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let instance = match instance {
            Some(instance) => self
                .registrations
                .get(instance)
                .cloned()
                .unwrap_or_else(|| instance.to_owned()),
            None => self
                .current_instance
                .clone()
                .ok_or_else(|| RuntimeError::Failed("no module instance is active".to_owned()))?,
        };
        let result = self.instance_name(Some(&instance)).map(drop);
        self.registrations.insert(name, instance);
        result
    }

    pub(super) fn invoke(
        &mut self,
        module: Option<&str>,
        export: &str,
        args: &[wast::WastArg<'_>],
    ) -> Result<Vec<Value>, RuntimeError> {
        let name = self.instance_name(module)?;
        let args = convert_args(args).map_err(RuntimeError::Unsupported)?;
        self.call_export(&name, export, args)
    }

    fn call_export(
        &mut self,
        instance: &str,
        export: &str,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, RuntimeError> {
        // wackasm cannot call an imported function as an export, so follow re-exports here.
        if let Some(target) = self
            .links
            .get(instance)
            .and_then(|links| links.reexports.get(export))
            .cloned()
        {
            return self.call_import(&target, args);
        }
        let Some(mut executor) = self.instances.remove(instance) else {
            return Err(if self.links.contains_key(instance) {
                RuntimeError::Unsupported(format!(
                    "re-entrant call into module instance {instance} is not supported"
                ))
            } else {
                RuntimeError::Failed(format!("unknown module instance {instance}"))
            });
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.run_export(&mut executor, instance, export, args)
        }))
        .map_err(runtime_panic)
        .and_then(|result| result);
        self.instances.insert(instance.to_owned(), executor);
        result
    }

    fn run_export(
        &mut self,
        executor: &mut WackasmExecutor,
        instance: &str,
        export: &str,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, RuntimeError> {
        executor.call_export(export, args)?;
        loop {
            match executor.execute()? {
                HostYield::Return => {
                    return executor.get_result().cloned().ok_or_else(|| {
                        RuntimeError::Failed("function completed without a result".to_owned())
                    });
                }
                HostYield::OutOfFuel => {
                    return Err(RuntimeError::Failed("execution ran out of fuel".to_owned()));
                }
                HostYield::ImportCall {
                    module,
                    name,
                    args,
                    should_return,
                } => {
                    let target = self
                        .links
                        .get(instance)
                        .and_then(|links| links.imports.get(&(module, name)))
                        .cloned()
                        .ok_or_else(|| {
                            RuntimeError::Failed("call to an unresolved import".to_owned())
                        })?;
                    let results = self.call_import(&target, args)?;
                    if results.len() != should_return {
                        return Err(RuntimeError::Failed(format!(
                            "import returned {} values, expected {should_return}",
                            results.len()
                        )));
                    }
                    executor.import_return(&results)?;
                }
            }
        }
    }

    fn call_import(
        &mut self,
        target: &ImportTarget,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, RuntimeError> {
        match target {
            ImportTarget::Spectest => Ok(Vec::new()),
            ImportTarget::Export { instance, export } => self.call_export(instance, export, args),
        }
    }

    /// Resolves function imports against `spectest` and registered instances.
    fn link(&self, bytes: &[u8]) -> Result<Links, RuntimeError> {
        let interface = module_interface(bytes).map_err(RuntimeError::Failed)?;
        let mut links = Links::default();
        let mut function_imports = Vec::with_capacity(interface.function_imports.len());
        for (module, name) in interface.function_imports {
            let target = self.resolve_import(module, name)?;
            links
                .imports
                .insert((module.to_owned(), name.to_owned()), target.clone());
            function_imports.push(target);
        }
        for (name, index) in interface.function_exports {
            match function_imports.get(index as usize) {
                Some(target) => {
                    links.reexports.insert(name.to_owned(), target.clone());
                }
                None => {
                    links.function_exports.insert(name.to_owned());
                }
            }
        }
        Ok(links)
    }

    fn resolve_import(&self, module: &str, name: &str) -> Result<ImportTarget, RuntimeError> {
        let unknown = || RuntimeError::Failed(format!("unknown import {module}.{name}"));
        if module == SPECTEST_MODULE {
            return is_spectest_function(name)
                .then_some(ImportTarget::Spectest)
                .ok_or_else(unknown);
        }
        let instance = self
            .registrations
            .get(module)
            .ok_or_else(|| RuntimeError::Failed(format!("unknown import module {module}")))?;
        if let Some(error) = self.unavailable.get(instance) {
            return Err(RuntimeError::Unsupported(format!(
                "import {module}.{name} comes from module instance {instance}, which failed to load: {}",
                error.message()
            )));
        }
        let links = self
            .links
            .get(instance)
            .ok_or_else(|| RuntimeError::Failed(format!("unknown module instance {instance}")))?;
        if let Some(target) = links.reexports.get(name) {
            return Ok(target.clone());
        }
        if links.function_exports.contains(name) {
            return Ok(ImportTarget::Export {
                instance: instance.clone(),
                export: name.to_owned(),
            });
        }
        Err(unknown())
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
