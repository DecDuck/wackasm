use clap::Parser;
use wackasm_spec_tests::run_suite_with_report;

#[derive(Debug, Parser)]
#[command(about = "Run WebAssembly WAST tests against wackasm")]
struct Args {
    #[arg(value_name = "SUITE")]
    suite_path: std::path::PathBuf,

    #[arg(value_name = "FILTER")]
    filter: Option<String>,

    #[arg(long, value_name = "FILE")]
    report: Option<std::path::PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let summary = run_suite_with_report(
        &args.suite_path,
        args.filter.as_deref(),
        args.report.as_deref(),
    )?;

    println!(
        "total: {} passed, {} failed, {} unsupported",
        summary.passed, summary.failed, summary.unsupported
    );
    if let Some(report_path) = args.report {
        println!("report: {}", report_path.display());
    }
    if summary.failed > 0 {
        anyhow::bail!("{} WAST checks failed", summary.failed);
    }
    Ok(())
}
