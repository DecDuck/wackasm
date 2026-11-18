//! `fetch URL...`: downloads several URLs at the same time through the
//! internet card, printing each one's status and size as it finishes, while a
//! second task prints how long it has been going. An example of the
//! `wackasm-opencomputers` crate's `async` feature.

use std::{cell::Cell, rc::Rc, time::Duration};

use wackasm_opencomputers::{Result, computer, internet, task};

fn main() {
    let urls: Vec<String> = std::env::args()
        .skip(1)
        .map(|url| if url.contains("://") { url } else { format!("http://{url}") })
        .collect();
    if urls.is_empty() {
        eprintln!("usage: fetch URL...");
        std::process::exit(2);
    }

    let failures = task::run(async move {
        let started = computer::uptime();
        let remaining = Rc::new(Cell::new(urls.len()));

        // Reports progress until the downloads are done.
        let left = remaining.clone();
        task::spawn(async move {
            while left.get() > 0 {
                task::sleep(Duration::from_secs(1)).await;
                if left.get() > 0 {
                    println!("  ... {} left after {:.0}s", left.get(), (computer::uptime() - started).as_secs_f64());
                }
            }
        });

        let downloads = urls.iter().map(|url| {
            let remaining = remaining.clone();
            async move {
                let result = download(url).await;
                remaining.set(remaining.get() - 1);
                let secs = (computer::uptime() - started).as_secs_f64();
                match &result {
                    Ok((status, bytes)) => println!("{status} {bytes:>7} bytes  {url}  ({secs:.1}s)"),
                    Err(e) => println!("ERR {url}: {e}"),
                }
                result.is_err()
            }
        });
        task::join_all(downloads).await.into_iter().filter(|failed| *failed).count()
    });
    std::process::exit(if failures == 0 { 0 } else { 1 });
}

async fn download(url: &str) -> Result<(u16, usize)> {
    let mut response = internet::get_async(url).await?;
    let mut body = Vec::new();
    response.read_to_end_async(&mut body).await.map_err(|e| wackasm_opencomputers::Error::Failed(e.to_string()))?;
    Ok((response.status(), body.len()))
}
