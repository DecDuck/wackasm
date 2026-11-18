---
title: Files
description: std::fs on OpenComputers disks, and the working directory.
sidebar:
  order: 2
---

`std::fs` works on one disk, which your program sees as `/`. See
[Files and directories](/wackasm/using/files/) for how that disk is chosen.
`wackasm_opencomputers::computer::files_disk()` returns its address.

```rust
use std::fs;

fs::create_dir_all("logs")?;
fs::write("logs/today.txt", "it works\n")?;
let text = fs::read_to_string("logs/today.txt")?;
for entry in fs::read_dir("/")? {
    println!("{}", entry?.file_name().to_string_lossy());
}
# Ok::<(), std::io::Error>(())
```

Supported: opening, reading, writing, seeking, directory listing, metadata,
creating and removing directories, removing and renaming files. Errors map
to the usual kinds: not found, read-only filesystem, out of space.

## How OpenComputers files differ

OpenComputers file handles are **read-only, truncating or appending**:

- opening an existing file for writing without truncating it appends;
- a file can't be open for reading and writing at once.

Every file operation is a component call and costs game time, as it does for
Lua. Reads come in 2 KiB chunks. Use a `BufReader` or read whole files.

## The working directory

`std::env::current_dir` and `std::env::set_current_dir` work, and relative
paths start at the current directory.

WASI programs always start in `/`, so something has to change into the
directory the program was run from. That directory arrives as `PWD`, and
`wackasm-opencomputers` changes into it in a constructor that runs before
`main`.

The constructor runs whenever the linker keeps it: always with LTO (as in the
release profile on [Your first program](/wackasm/programs/first-program/)),
and whenever your program calls into the crate's `process` module. To be sure
in every build, including debug builds:

```rust
fn main() {
    wackasm_opencomputers::process::enter_working_dir();
    // ...
}
```

A program that only needs the directory, and otherwise doesn't use the crate,
can link it with `use wackasm_opencomputers as _;`. A program without the
crate starts in `/`, and can read `PWD` itself.
