//! `curl [OPTIONS] URL`: an HTTP(S) request through the internet card.
//!
//! * `-i`: print the status line and headers before the body
//! * `-I`: a `HEAD` request; print only the status line and headers
//! * `-X METHOD`: the request method (default `GET`, or `POST` with `-d`)
//! * `-H "Name: value"`: add a header (repeatable)
//! * `-d DATA`: send DATA as the body; `-d @FILE` sends a file
//! * `-o FILE`: save the body to FILE instead of printing it
//! * `-O`: save it under the URL's file name (`.../tool.wasm` saves `tool.wasm`)
//! * `-f`: exit with 22 on an HTTP error status (400 and above), like curl
//!
//! Exits with 1 if the request fails (no card, unknown host, blocked, ...).

use std::{
    io::{self, Read, Write},
    process::exit,
};

use opencomputers::{computer, internet::Request};

fn usage() -> ! {
    eprintln!("usage: curl [-i] [-I] [-f] [-X METHOD] [-H \"Name: value\"] [-d DATA|@FILE] [-o FILE | -O] URL");
    exit(2);
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("curl: {message}");
    exit(1);
}

/// The last path segment of `url`, without a query or fragment.
fn file_name(url: &str) -> Option<String> {
    let path = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = path.split(['?', '#']).next()?;
    let (_, path) = path.split_once('/')?;
    path.rsplit('/').next().filter(|name| !name.is_empty() && *name != "." && *name != "..").map(str::to_owned)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut include, mut head, mut fail_on_error) = (false, false, false);
    let (mut method, mut data, mut output, mut url) = (None, None, None, None);
    let mut headers = Vec::new();
    let mut remote_name = false;
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "-i" => include = true,
            "-I" => head = true,
            "-f" => fail_on_error = true,
            "-X" => method = Some(value()),
            "-H" => {
                let header = value();
                let Some((name, val)) = header.split_once(':') else { fail(format!("bad header {header:?}")) };
                headers.push((name.trim().to_owned(), val.trim().to_owned()));
            }
            "-d" => {
                let text = value();
                data = Some(match text.strip_prefix('@') {
                    Some(file) => std::fs::read(file).unwrap_or_else(|e| fail(format!("{file}: {e}"))),
                    None => text.into_bytes(),
                });
            }
            "-o" => output = Some(value()),
            "-O" => remote_name = true,
            flag if flag.starts_with('-') => usage(),
            _ if url.is_some() => usage(),
            _ => url = Some(arg),
        }
    }
    let Some(url) = url else { usage() };
    let url = if url.contains("://") { url } else { format!("http://{url}") };
    if remote_name && output.is_none() {
        output = Some(file_name(&url).unwrap_or_else(|| fail(format!("{url}: no file name in the URL; use -o FILE"))));
    }

    let mut request = Request::new(&url);
    if let Some(method) = method.or(head.then(|| "HEAD".to_owned())) {
        request = request.method(method);
    }
    for (name, value) in headers {
        request = request.header(name, value);
    }
    if let Some(data) = data {
        request = request.body(data);
    }

    let started = computer::uptime();
    let mut response = request.send().unwrap_or_else(|e| fail(format!("{url}: {e}")));
    let status = response.status();
    if include || head {
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "HTTP {status} {}", response.message());
        for (name, value) in response.headers() {
            let _ = writeln!(stdout, "{name}: {value}");
        }
        let _ = writeln!(stdout);
        let _ = stdout.flush();
    }
    if fail_on_error && status >= 400 {
        // `exit` skips destructors, and dropping is what releases the request.
        drop(response);
        eprintln!("curl: the requested URL returned error: {status}");
        exit(22);
    }
    if head {
        return;
    }

    let mut sink: Box<dyn Write> = match &output {
        Some(file) => Box::new(std::fs::File::create(file).unwrap_or_else(|e| fail(format!("{file}: {e}")))),
        None => Box::new(io::stdout().lock()),
    };
    let mut buf = [0u8; 2048];
    let mut total = 0usize;
    loop {
        match response.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if let Err(e) = sink.write_all(&buf[..n]) {
                    fail(e);
                }
            }
            Err(e) => fail(format!("reading the response: {e}")),
        }
    }
    let _ = sink.flush();
    drop(sink);
    if let Some(file) = output {
        let secs = (computer::uptime() - started).as_secs_f64();
        eprintln!("saved {total} bytes to {file} in {secs:.1}s");
    }
}
