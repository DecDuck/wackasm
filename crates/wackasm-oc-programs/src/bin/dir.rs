//! `dir [PATH]`: lists a directory with sizes, via ordinary `std::fs`.

// Starts the program in the directory it was run from (see `opencomputers::process`).
use opencomputers as _;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("dir: {path}: {e}");
            std::process::exit(1);
        }
    };
    let mut rows: Vec<(String, Option<u64>)> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let meta = entry.metadata().ok();
            let size = meta.filter(|m| m.is_file()).map(|m| m.len());
            (entry.file_name().to_string_lossy().into_owned(), size)
        })
        .collect();
    rows.sort();
    for (name, size) in rows {
        match size {
            Some(size) => println!("{size:>8}  {name}"),
            None => println!("   <dir>  {name}/"),
        }
    }
}
