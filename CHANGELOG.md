# Changelog

## Unreleased

### Mod

- With **Applied Energistics 2** (19.2.17 or newer) installed: the
  **Computer Pattern Provider**, an ME device and OpenComputers component
  (`pattern_provider`). Computers declare processing patterns, which AE2
  autocrafting uses like any others; the computer gets each job's inputs and
  does the processing, and the outputs are imported into the network as for
  any processing pattern. Crafted from an AE2 Pattern Provider, a tier 1
  microchip and a cable. AE2 remains optional.

### `wackasm-opencomputers` crate

- `pattern_provider`: typed bindings for the Computer Pattern Provider, and a
  `Signal::PatternRequest` variant (code matching `Signal` exhaustively needs
  a new arm).

## 0.3.0

### Mod

- Moved from Fabric to **NeoForge** (Minecraft 1.21.1). Requires NeoForge
  21.1.249 or newer and the NeoForge build of OpenComputers: Modernized
  1.10.7 or newer. Fabric API is no longer needed. The config file
  (`config/wackasm.properties`), items and recipes are unchanged.

### `wackasm-opencomputers` crate

- No API changes. The component bindings' source is now the NeoForge build of
  OpenComputers: Modernized, which provides the same components.

## 0.2.0

### Mod

- Ported from Forge 1.12.2 / OpenComputers 1.7.5 to **Fabric 1.21.1** and
  [OpenComputers: Modernized](https://github.com/habzg/OpenComputers-Modernized)
  1.10.7. Requires Fabric API and Java 21.
- Configuration moved to `config/wackasm.properties`, written with commented
  defaults on first start.
- Machine state is kept in `<world>/wackasm/`.
- The WebAssembly BIOS EEPROM appears in the OpenComputers creative tab, and
  both it and the "wackasm shell" floppy can be crafted (EEPROM or floppy +
  paper).
- The shell floppy is served straight from the mod's resources, so it works
  in development runs as well as from the jar.
- Fixed: Enter not registering in the shell (OpenComputers: Modernized sends
  LF rather than CR).
- Fixed: reading past the end of a file on OpenComputers: Modernized crashed
  the program.
- Fixed: calling an unknown component method now reports "no such method"
  instead of a Java error.
- The BIOS reports the first reason it couldn't boot, rather than the last.

### `wackasm-opencomputers` crate

- Component bindings regenerated from OpenComputers: Modernized. Many methods
  were added (wireless network card strength and repeater, energy, upgrade
  containers, tank and fluid-container queries, `max_packet_size`, …).
- **Breaking:** signatures follow OpenComputers: Modernized, where it changed
  them. For example, the debug card's `get_world` no longer takes a dimension
  id, `get_worlds` is gone, `scan_contents_at` lost its `world_id`, and and
  `Robot::drain` / `Drone::drain` take a side number instead of a `bool`.
