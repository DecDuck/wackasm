use wackasm::Value;
use wast::{
    Wast, WastDirective, WastExecute, WastRet,
    parser::{self, ParseBuffer},
};

use crate::{
    Outcome, Summary,
    runtime::{RuntimeError, ScriptRunner, classify_runtime_error, is_trap_error},
    values::{convert_expected, value_matches},
};

pub fn run_source(source: &str) -> Summary {
    run_source_with_report(source, |_, _| {})
}

pub(super) fn run_source_with_report(
    source: &str,
    mut report: impl FnMut(usize, &Outcome),
) -> Summary {
    let buffer = match ParseBuffer::new(source) {
        Ok(buffer) => buffer,
        Err(error) => {
            let outcome = Outcome::Failed(error.to_string());
            report(0, &outcome);
            let mut summary = Summary::default();
            summary.record(outcome);
            return summary;
        }
    };
    let script = match parser::parse::<Wast>(&buffer) {
        Ok(script) => script,
        Err(error) => {
            let outcome = Outcome::Failed(error.to_string());
            report(0, &outcome);
            let mut summary = Summary::default();
            summary.record(outcome);
            return summary;
        }
    };

    let mut runner = ScriptRunner::default();
    let mut summary = Summary::default();
    for (index, directive) in script.directives.into_iter().enumerate() {
        let outcome = run_directive(&mut runner, directive);
        report(index + 1, &outcome);
        summary.record(outcome);
    }
    summary
}

fn run_directive(runner: &mut ScriptRunner, directive: WastDirective<'_>) -> Outcome {
    match directive {
        WastDirective::Module(mut module) => {
            let name = runner.definition_name(module.name().map(|name| name.name().to_owned()));
            match module.encode().map_err(|error| error.to_string()) {
                Ok(bytes) => match validate_module(&bytes) {
                    Err(error) => Outcome::Failed(error),
                    Ok(()) => match runner.install(name, bytes) {
                        Ok(()) => Outcome::Passed,
                        Err(error) => classify_runtime_error(error),
                    },
                },
                Err(error) => Outcome::Failed(error),
            }
        }
        WastDirective::ModuleDefinition(mut module) => {
            let name = runner.definition_name(module.name().map(|name| name.name().to_owned()));
            match module.encode().map_err(|error| error.to_string()) {
                Ok(bytes) => match validate_module(&bytes) {
                    Ok(()) => {
                        runner.definitions.insert(name.clone(), bytes);
                        runner.current_definition = Some(name);
                        Outcome::Passed
                    }
                    Err(error) => Outcome::Failed(error),
                },
                Err(error) => Outcome::Failed(error),
            }
        }
        WastDirective::ModuleInstance {
            instance, module, ..
        } => {
            let Some(definition) = module
                .map(|name| name.name().to_owned())
                .or_else(|| runner.current_definition.clone())
            else {
                return Outcome::Failed("no module definition is available".to_owned());
            };
            let instance = instance
                .map(|name| name.name().to_owned())
                .unwrap_or_else(|| runner.next_name("instance-"));
            match runner.instantiate(&definition, instance) {
                Ok(()) => Outcome::Passed,
                Err(error) => classify_runtime_error(error),
            }
        }
        WastDirective::Register { name, module, .. } => {
            match runner.instance_name(module.map(|name| name.name())) {
                Ok(instance) => {
                    runner.register(name.to_owned(), instance);
                    Outcome::Passed
                }
                Err(error) => Outcome::Unsupported(error),
            }
        }
        WastDirective::Invoke(invoke) => match runner.invoke(
            invoke.module.map(|name| name.name()),
            invoke.name,
            &invoke.args,
        ) {
            Ok(_) => Outcome::Passed,
            Err(error) => classify_runtime_error(error),
        },
        WastDirective::AssertReturn { exec, results, .. } => {
            run_assert_return(runner, exec, &results)
        }
        WastDirective::AssertTrap { exec, .. } => match run_execute(runner, exec) {
            Ok(_) => Outcome::Failed("expected execution to trap".to_owned()),
            Err(error) => match error {
                RuntimeError::Unsupported(error) => Outcome::Unsupported(error),
                RuntimeError::Trap(_) => Outcome::Passed,
                RuntimeError::Failed(error) => {
                    Outcome::Failed(format!("expected a trap, got an execution error: {error}"))
                }
            },
        },
        WastDirective::AssertMalformed { mut module, .. }
        | WastDirective::AssertMalformedCustom { mut module, .. } => match module.encode() {
            Err(_) => Outcome::Passed,
            Ok(bytes) if decode_module(&bytes).is_err() => Outcome::Passed,
            Ok(_) => Outcome::Failed("module was not malformed".to_owned()),
        },
        WastDirective::AssertInvalid { mut module, .. }
        | WastDirective::AssertInvalidCustom { mut module, .. } => match module.encode() {
            Err(error) => Outcome::Failed(error.to_string()),
            Ok(bytes) => match decode_module(&bytes) {
                Err(error) => Outcome::Failed(format!("module is malformed, not invalid: {error}")),
                Ok(_) if wasmparser::Validator::new().validate_all(&bytes).is_err() => {
                    Outcome::Passed
                }
                Ok(_) => Outcome::Failed("module validated successfully".to_owned()),
            },
        },
        WastDirective::AssertUnlinkable { .. } => {
            Outcome::Unsupported("module linking is not implemented by wackasm".to_owned())
        }
        WastDirective::AssertExhaustion { .. } => {
            Outcome::Unsupported("resource exhaustion assertions are not implemented".to_owned())
        }
        WastDirective::AssertException { .. } => {
            Outcome::Unsupported("exception assertions are not implemented".to_owned())
        }
        WastDirective::AssertSuspension { .. } => {
            Outcome::Unsupported("suspension assertions are not implemented".to_owned())
        }
        WastDirective::Thread(_) | WastDirective::Wait { .. } => {
            Outcome::Unsupported("thread directives are not implemented".to_owned())
        }
    }
}

/// Decodes every section item and function body, since `parse_all` alone leaves section contents unread.
fn decode_module(bytes: &[u8]) -> Result<(), String> {
    use wasmparser::{ElementItems, Operator, Payload};

    fn drain<T>(items: impl IntoIterator<Item = wasmparser::Result<T>>) -> wasmparser::Result<()> {
        items.into_iter().try_for_each(|item| item.map(drop))
    }

    let mut unknown_section = None;
    let mut has_data_count = false;
    let mut uses_data_index = false;
    let mut decode = || -> wasmparser::Result<()> {
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            match payload? {
                // wasmparser tolerates unknown section ids, but the spec treats them as malformed.
                Payload::UnknownSection { id, .. } => {
                    unknown_section = unknown_section.or(Some(id))
                }
                Payload::DataCountSection { .. } => has_data_count = true,
                Payload::TypeSection(section) => drain(section)?,
                Payload::ImportSection(section) => drain(section)?,
                Payload::FunctionSection(section) => drain(section)?,
                Payload::TableSection(section) => drain(section)?,
                Payload::MemorySection(section) => drain(section)?,
                Payload::TagSection(section) => drain(section)?,
                Payload::GlobalSection(section) => drain(section)?,
                Payload::ExportSection(section) => drain(section)?,
                Payload::DataSection(section) => drain(section)?,
                Payload::ElementSection(section) => {
                    for element in section {
                        match element?.items {
                            ElementItems::Functions(items) => drain(items)?,
                            ElementItems::Expressions(_, items) => drain(items)?,
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    drain(body.get_locals_reader()?)?;
                    let mut operators = body.get_operators_reader()?;
                    while !operators.eof() {
                        if let Operator::MemoryInit { .. } | Operator::DataDrop { .. } =
                            operators.read()?
                        {
                            uses_data_index = true;
                        }
                    }
                    operators.finish()?;
                }
                _ => {}
            }
        }
        Ok(())
    };
    decode().map_err(|error| error.to_string())?;

    if let Some(id) = unknown_section {
        return Err(format!("malformed section id {id}"));
    }
    // `OperatorsReader::finish` does not enforce this binary-format rule.
    if uses_data_index && !has_data_count {
        return Err("data count section required".to_owned());
    }
    Ok(())
}

fn validate_module(bytes: &[u8]) -> Result<(), String> {
    wasmparser::Validator::new()
        .validate_all(bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn run_execute(
    runner: &mut ScriptRunner,
    exec: WastExecute<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    match exec {
        WastExecute::Invoke(invoke) => runner
            .invoke(
                invoke.module.map(|name| name.name()),
                invoke.name,
                &invoke.args,
            )
            .map_err(|error| {
                if error.contains("not implemented") || error.contains("unsupported operator") {
                    RuntimeError::Unsupported(error)
                } else if is_trap_error(&error) {
                    RuntimeError::Trap(error)
                } else {
                    RuntimeError::Failed(error)
                }
            }),
        WastExecute::Get { .. } => Err(RuntimeError::Unsupported(
            "global.get assertions need a global access API".to_owned(),
        )),
        WastExecute::Wat(_) => Err(RuntimeError::Unsupported(
            "module-valued executions are not implemented".to_owned(),
        )),
    }
}

fn run_assert_return(
    runner: &mut ScriptRunner,
    exec: WastExecute<'_>,
    expected: &[WastRet<'_>],
) -> Outcome {
    let expected = match expected
        .iter()
        .map(convert_expected)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(expected) => expected,
        Err(error) => return Outcome::Unsupported(error),
    };
    let actual = match run_execute(runner, exec) {
        Ok(actual) => actual,
        Err(RuntimeError::Failed(error)) => return Outcome::Failed(error),
        Err(RuntimeError::Unsupported(error)) => return Outcome::Unsupported(error),
        Err(RuntimeError::Trap(error)) => {
            return Outcome::Failed(format!("expected a return value, got a trap: {error}"));
        }
    };
    if actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| {
                expected
                    .iter()
                    .any(|expected| value_matches(actual, expected))
            })
    {
        Outcome::Passed
    } else {
        Outcome::Failed(format!("expected {expected:?}, got {actual:?}"))
    }
}
