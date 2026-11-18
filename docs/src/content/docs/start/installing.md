---
title: Installing
description: Install the mod and what it needs.
sidebar:
  order: 1
---

wackasm is a **Fabric** mod for **Minecraft 1.21.1**. It needs:

- [Fabric Loader](https://fabricmc.net/) 0.20.1 or newer, and
  [Fabric API](https://modrinth.com/mod/fabric-api);
- [OpenComputers: Modernized](https://www.curseforge.com/minecraft/mc-mods/opencomputers-modernized)
  1.10.7 or newer (its Fabric build).

Download `wackasm-oc-<version>.jar` from the
[GitHub releases](https://github.com/DecDuck/wackasm/releases) and put it in
your `mods/` folder next to OpenComputers: Modernized, on the server and on
every client.

## Supported platforms

The interpreter is a native library bundled in the jar. Release jars include
builds for:

| OS | Architectures |
|---|---|
| Windows | x86_64 |
| Linux | x86_64, aarch64 |
| macOS | x86_64, aarch64 (Apple silicon) |

The library runs on the server (or the integrated server in single player),
so only the machine hosting the world needs a supported platform.

When the mod starts it logs a self-test of the native library. If the library
can't load on your platform, wasm computers report the error when turned on,
and Lua computers are unaffected.

## Next

[Build your first computer](/wackasm/start/first-computer/).
