use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use crate::{Outcome, Summary, script::run_source_with_report};

pub fn run_suite(path: &Path, filter: Option<&str>) -> anyhow::Result<Summary> {
    run_suite_with_report(path, filter, None)
}

pub fn run_suite_with_report(
    path: &Path,
    filter: Option<&str>,
    report_path: Option<&Path>,
) -> anyhow::Result<Summary> {
    let mut files = Vec::new();
    collect_wast_files(path, &mut files)?;
    files.sort();

    let mut report = report_path
        .map(File::create)
        .transpose()?
        .map(BufWriter::new);
    let mut total = Summary::default();
    for file in files {
        let display = if path.is_file() {
            file.as_path()
        } else {
            file.strip_prefix(path).unwrap_or(&file)
        };
        if filter.is_some_and(|filter| !display.to_string_lossy().contains(filter)) {
            continue;
        }
        let source = fs::read_to_string(&file)?;
        let mut issues = Vec::new();
        let summary = run_source_with_report(&source, |index, outcome| {
            println!("  directive {index}: {outcome:?}");
            if matches!(outcome, Outcome::Failed(_) | Outcome::Unsupported(_)) {
                issues.push((index, outcome.clone()));
            }
        });
        if let Some(report) = &mut report {
            for (index, outcome) in issues {
                write_issue(report, display, index, &outcome)?;
            }
            writeln!(
                report,
                "SUMMARY\t{}\tpassed={}\tfailed={}\tunsupported={}",
                escape_field(&display.to_string_lossy()),
                summary.passed,
                summary.failed,
                summary.unsupported
            )?;
        }
        println!(
            "{}: {} passed, {} failed, {} unsupported",
            display.display(),
            summary.passed,
            summary.failed,
            summary.unsupported
        );
        total.merge(summary);
    }
    if let Some(report) = &mut report {
        writeln!(
            report,
            "TOTAL\tpassed={}\tfailed={}\tunsupported={}",
            total.passed, total.failed, total.unsupported
        )?;
        report.flush()?;
    }
    Ok(total)
}

fn write_issue(
    report: &mut impl Write,
    file: &Path,
    directive: usize,
    outcome: &Outcome,
) -> anyhow::Result<()> {
    let (status, detail) = match outcome {
        Outcome::Failed(detail) => ("FAIL", detail),
        Outcome::Unsupported(detail) => ("UNSUPPORTED", detail),
        Outcome::Passed => return Ok(()),
    };
    writeln!(
        report,
        "{status}\t{}\tdirective={}\t{}",
        escape_field(&file.to_string_lossy()),
        directive,
        escape_field(detail)
    )?;
    Ok(())
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

fn collect_wast_files(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if path.is_file() {
        if path
            .extension()
            .is_some_and(|extension| extension == "wast")
        {
            files.push(path.to_owned());
        }
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_wast_files(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "wast")
        {
            files.push(path);
        }
    }
    Ok(())
}
