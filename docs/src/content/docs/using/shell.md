---
title: The shell
description: Built-in commands of the wackasm shell, and how it runs programs.
sidebar:
  order: 1
---

The shell floppy's `/init.wasm` is a small command line. Type a command and
press Enter. Words are split on spaces, and `"double quotes"` keep words
together.

## Built-in commands

| command | does |
|---|---|
| `help` | lists the built-ins |
| `info` | this computer's address, RAM, uptime, disks, and the disk programs use for files |
| `components` | every component and its address |
| `cd [DIR]` | changes the current directory (`/` without an argument) |
| `pwd` | prints the current directory |
| `ls [DIR]` | lists a directory, the current one by default |
| `beep` | beeps |
| `clear` | clears the screen |
| `reboot`, `shutdown` | what they say |

The prompt shows the current directory: `/bin> `.

## Running programs

Anything else runs a program. `name args...` looks for `/bin/name.wasm`, then
`/bin/name`, on the boot disk first and then on every other disk. A name with
a `/` in it is a path, relative to the current directory: `./tool.wasm`,
`../bin/x.wasm`, `/home/thing.wasm`.

While a program runs:

- its output (stdout and stderr) appears on the screen;
- when it reads input, the shell reads a line you type and passes it on.
  **Ctrl+D** sends end of file;
- when it exits with a non-zero code, the shell shows `[exit N]`. A program
  that crashes shows `[name crashed: reason]`.

Programs get the arguments you typed, and start in the shell's current
directory with the environment `PATH=/bin`, `SHELL=/init.wasm` and `PWD`.

The first run of a program reads it from disk, which takes a few ticks for a
typical 60 KB program. After that the server keeps it compiled, so running it
again, on any computer, starts almost immediately. Saving a new version of
the file (a different size or modification time) makes the next run load it
fresh.

## Limits

- There are no pipes, redirects, variables or `&&` yet. Programs can run
  other programs themselves; see `time`, `env` and `lines` in
  [Included programs](/wackasm/using/programs/).
- A program that loops forever can't be interrupted from the shell. Turn the
  computer off and on again.
- While a program waits for typed input, the shell consumes signals other
  than key presses (for example modem messages).
