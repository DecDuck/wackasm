---
title: HTTP and TCP
description: Use the internet card from your programs.
sidebar:
  order: 6
---

`wackasm_opencomputers::internet` uses the first internet card (or one you
choose) for HTTP(S) and TCP. `Response` and `TcpStream` are ordinary
`std::io` streams.

```rust
use std::io::Read;
use wackasm_opencomputers::internet;

let mut response = internet::get("https://example.com/")?;
println!("{} {}", response.status(), response.message());
let mut body = String::new();
response.read_to_string(&mut body)?;

let response = internet::Request::new("https://httpbin.org/post")
    .method("POST")
    .header("Content-Type", "application/json")
    .body(r#"{"hello": "world"}"#)
    .send()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`send()` waits for the status line and headers. Every status, 404 included,
is a `Response`; errors are for requests that fail (unknown host, refused,
blocked by the server's config). `Response::header(name)` looks one up
case-insensitively.

## TCP

```rust
use std::io::{Read, Write};
use wackasm_opencomputers::internet::TcpStream;

let mut stream = TcpStream::connect("example.com", 80)?;
stream.write_all(b"GET / HTTP/1.0\r\nHost: example.com\r\n\r\n")?;
let mut reply = Vec::new();
stream.read_to_end(&mut reply)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`try_read` reads what has arrived without waiting for more: `Ok(None)` if
nothing has, `Ok(Some(0))` once the other side has closed.

## Things to know

- Reads come in chunks of at most 2 KiB, one per server tick: about 40 KB/s.
  Waiting for data costs a tick per check, not fuel.
- The server's OpenComputers config decides what's reachable. By default
  local and private addresses are blocked.
- Dropping a `Response` or `TcpStream` closes the connection and frees it on
  the server. `std::process::exit` skips destructors, so drop connections
  before exiting, or they stay open until the computer stops.
