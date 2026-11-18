# Changelog

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
