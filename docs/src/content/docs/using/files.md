---
title: Files and directories
description: Which disk programs see, and how the current directory works.
sidebar:
  order: 3
---

## The files disk

A program sees one disk as `/`. That disk is chosen the first time anything
touches a file:

1. the disk the computer booted from, if it's writable;
2. otherwise the first writable disk. So with the read-only shell floppy
   booted, files go to your hard drive;
3. otherwise a read-only disk.

The computer's tmpfs is never used. The shell's `cd` and `ls` work on the
same disk, and `info` shows which one it is (`files:`). A program and every
program it runs share the same files disk.

Programs are looked up differently: `/bin` is searched on the boot disk and
then on every other disk. That's why the floppy's programs run even though
files go to the hard drive.

## The current directory

`cd` changes the shell's current directory, which the prompt shows. Programs
start in it, and relative paths start there:

```
/> cd notes
/notes> write todo.txt buy redstone
wrote 13 bytes to todo.txt
/notes> cat ../notes/todo.txt
buy redstone
```

The directory is passed to programs as the `PWD` environment variable, and
programs built with the
[`wackasm-opencomputers`](/wackasm/programs/first-program/) crate change into
it before `main`. A WASI program that doesn't use the crate starts in `/`;
see [Files in your programs](/wackasm/programs/files/).

## Copying files onto a disk

- **With an internet card**, download them:
  `curl -o notes.txt https://example.com/notes.txt`. See
  [Internet](/wackasm/using/internet/).
- **On the server**, every OpenComputers disk is a folder:
  `<world>/opencomputers/<disk address>/`. Find the address with
  `components` or `info`.
- **From a Lua computer** with OpenOS: put the disk in it and use OpenOS's
  tools (`wget`, `cp`), then move the disk back.
