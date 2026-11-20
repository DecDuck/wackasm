---
title: AE2 autocrafting
description: Give Applied Energistics 2 crafting patterns that your program fulfils.
sidebar:
  order: 10
---

With [Applied Energistics 2](https://modrinth.com/mod/ae2) installed, the mod
adds the **Computer Pattern Provider**. It's an ME network device and an
OpenComputers component at once. A program gives it processing patterns,
and AE2's autocrafting uses them like any others: they show up as craftable
in terminals, and the crafting planner includes them in jobs. When a job
needs one, the program gets the inputs and does the processing however it
likes, with robots, machines, other mods or anything else.

Craft it from an AE2 Pattern Provider, a tier 1 microchip and a cable
(shapeless). Connect it to the ME network like any other device (it uses a
channel), and to the computer with a cable or by placing it next to it.

## How a job runs

1. The program declares patterns: these inputs, processed, give these
   outputs.
2. A crafting job needs a pattern. AE2 sends the inputs to the provider,
   which **holds** them as a job, and the computer gets a `pattern_request`
   signal.
3. The program moves the inputs into an adjacent inventory or tank
   (`push_inputs`) and does the processing.
4. The outputs have to be **imported into the ME network**, the same as for
   any processing pattern: through an ME Interface, an import bus, or by
   putting them into the provider itself (with a transposer, hopper or
   pipe), which forwards whatever it receives to the network. AE2's crafting
   CPUs take the outputs they're waiting for as they arrive.

The provider never creates items. A job only finishes once real items reach
the network.

```rust
use wackasm_opencomputers::prelude::*;
use wackasm_opencomputers::pattern_provider::{Pattern, Stack};

fn main() -> Result<()> {
    let provider = PatternProvider::first().ok_or(Error::NoSuchComponent)?;
    provider.set_patterns(&[
        Pattern::new(
            vec![Stack::item("minecraft:cobblestone", 1)],
            vec![Stack::item("minecraft:stone", 1)],
        ),
        Pattern::new(
            vec![Stack::fluid("minecraft:water", 1000)],
            vec![Stack::item("minecraft:ice", 1)],
        ),
    ])?;

    loop {
        // Check every job, not just the one that signalled: jobs from before
        // this program started are waiting too.
        for job in provider.jobs()? {
            // A furnace above the provider; its output goes to an ME interface.
            if !provider.push_inputs(job.id, Side::Top)? {
                break; // full: try again after the next signal
            }
        }
        signal::pull_filtered(None, |s| matches!(s, Signal::PatternRequest { .. }));
    }
}
```

## Patterns and stacks

A stack is an item or fluid and an amount: `Stack::item(name, count)` or
`Stack::fluid(name, millibuckets)`. Items with data components (enchanted
books, named items, ...) can't be described by name alone. Stacks the
component returns carry a `key` for them (AE2's description of the exact
stack), so a stack read from `patterns()` or `jobs()` can be passed back as
it is.

AE2 needs every pattern to have at least one input (up to 81) and one output
(up to 27). The first output is the one AE2 lists the pattern under. Two
identical patterns aren't allowed. A pattern that produces something from
nothing needs a token input, such as a "work order" item that the program
returns along with the outputs.

The patterns are saved with the block, so they survive restarts. A program
can set them once, or every time it starts.

## Jobs

`jobs()` lists the jobs waiting, oldest first, each with its `id`, the
inputs still held, and the `outputs` AE2 expects. The `pattern_request`
signal is only a hint: a job stays held until the program deals with it, so
a program that was off, or missed the signal, catches up with `jobs()`.

- `push_inputs(id, side)` moves the inputs into the inventory or tank on that
  side of the provider. It returns `true` once everything is out, which ends
  the job. If something didn't fit, it returns `false` and keeps the rest.
- `cancel_job(id)` returns the inputs to the network. AE2's crafting job
  keeps waiting for its outputs until it's cancelled in a terminal.

## Things to know

- The provider tells AE2 it's busy while no running computer is connected,
  so inputs don't pile up with nobody to process them, and while it holds 64
  jobs.
- Breaking the block drops the items it holds. Fluids it holds are lost.
- From Lua the component works the same way: `setPatterns`, `addPattern`,
  `removePattern`, `getPatterns`, `getJobs`, `getJob`, `pushInputs`,
  `cancelJob` and `isOnline`. Stacks are tables like
  `{name="minecraft:iron_ingot", count=4}` or
  `{name="minecraft:water", count=1000, fluid=true}`.
