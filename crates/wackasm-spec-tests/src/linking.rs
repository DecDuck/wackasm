use std::collections::{HashMap, HashSet};

use wasmparser::{ExternalKind, Payload, TypeRef};

pub(super) const SPECTEST_MODULE: &str = "spectest";

/// Function exports of the reference interpreter's `spectest` module. They only print, so the
/// runner implements them as no-ops.
const SPECTEST_FUNCTIONS: &[&str] = &[
    "print",
    "print_i32",
    "print_i64",
    "print_f32",
    "print_f64",
    "print_i32_f32",
    "print_f64_f64",
];

pub(super) fn is_spectest_function(name: &str) -> bool {
    SPECTEST_FUNCTIONS.contains(&name)
}

#[derive(Clone, Debug)]
pub(super) enum ImportTarget {
    Spectest,
    Export { instance: String, export: String },
}

/// Function imports of an instance, resolved when it was instantiated.
#[derive(Default)]
pub(super) struct Links {
    pub(super) imports: HashMap<(String, String), ImportTarget>,
    /// Exports that re-export an imported function.
    pub(super) reexports: HashMap<String, ImportTarget>,
    /// Exports of functions defined by the module itself.
    pub(super) function_exports: HashSet<String>,
}

pub(super) struct ModuleInterface<'a> {
    pub(super) function_imports: Vec<(&'a str, &'a str)>,
    pub(super) function_exports: Vec<(&'a str, u32)>,
}

pub(super) fn module_interface(bytes: &[u8]) -> Result<ModuleInterface<'_>, String> {
    let mut interface = ModuleInterface {
        function_imports: Vec::new(),
        function_exports: Vec::new(),
    };
    let mut read = || -> wasmparser::Result<()> {
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            match payload? {
                Payload::ImportSection(section) => {
                    for import in section.into_imports() {
                        let import = import?;
                        if let TypeRef::Func(_) = import.ty {
                            interface
                                .function_imports
                                .push((import.module, import.name));
                        }
                    }
                }
                Payload::ExportSection(section) => {
                    for export in section {
                        let export = export?;
                        if export.kind == ExternalKind::Func {
                            interface.function_exports.push((export.name, export.index));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    };
    read().map_err(|error| error.to_string())?;
    Ok(interface)
}
