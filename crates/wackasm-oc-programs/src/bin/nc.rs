//! `nc [-C] [-w SECS] HOST PORT`: a TCP connection through the internet card.
//!
//! Each line typed is sent as it is entered (ending in `\r\n` with `-C`, as
//! HTTP and most text protocols want), and whatever the other side has sent
//! by then is printed. After Ctrl+D, it prints what arrives until the other
//! side closes the connection, or nothing has come for `-w` seconds
//! (default 10).
//!
//! ```text
//! > nc -C example.com 80
//! GET / HTTP/1.0
//! Host: example.com
//!
//! ^D
//! ```

use std::{
    io::{self, BufRead, Write},
    process::exit,
    time::Duration,
};

use opencomputers::{computer, internet::TcpStream};

fn usage() -> ! {
    eprintln!("usage: nc [-C] [-w SECS] HOST PORT");
    exit(2);
}

fn print(out: &mut impl Write, bytes: &[u8]) {
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

/// Prints what has arrived. `false` once the connection is closed.
fn drain(stream: &mut TcpStream, out: &mut impl Write) -> bool {
    let mut buf = [0u8; 2048];
    loop {
        match stream.try_read(&mut buf) {
            Ok(None) => return true,
            Ok(Some(0)) => return false,
            Ok(Some(n)) => print(out, &buf[..n]),
            Err(e) => {
                eprintln!("nc: {e}");
                return false;
            }
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut crlf, mut wait, mut rest) = (false, Duration::from_secs(10), Vec::new());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-C" => crlf = true,
            "-w" => {
                let secs: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
                wait = Duration::from_secs_f64(secs.max(0.0));
            }
            _ => rest.push(arg),
        }
    }
    let [host, port] = rest.as_slice() else { usage() };
    let Ok(port) = port.parse::<u16>() else { usage() };

    let mut stream = TcpStream::connect(host, port).unwrap_or_else(|e| {
        eprintln!("nc: {host}:{port}: {e}");
        exit(1);
    });
    eprintln!("connected to {host}:{port}; Ctrl+D ends input");
    let mut out = io::stdout();
    if !drain(&mut stream, &mut out) {
        return;
    }

    let ending = if crlf { "\r\n" } else { "\n" };
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Err(e) = stream.write_all(format!("{line}{ending}").as_bytes()) {
            eprintln!("nc: {e}");
            exit(1);
        }
        if !drain(&mut stream, &mut out) {
            return;
        }
    }

    // Input is done; print the rest until the connection closes or goes quiet.
    let mut buf = [0u8; 2048];
    let mut last = computer::uptime();
    loop {
        match stream.try_read(&mut buf) {
            Ok(Some(0)) => return,
            Ok(Some(n)) => {
                print(&mut out, &buf[..n]);
                last = computer::uptime();
            }
            Ok(None) if computer::uptime() - last >= wait => {
                eprintln!("nc: nothing for {}s, closing", wait.as_secs_f64());
                return;
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("nc: {e}");
                exit(1);
            }
        }
    }
}
