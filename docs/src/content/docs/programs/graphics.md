---
title: Graphics
description: Draw text, colours, pixels, braille and shapes with the draw module, and let it work out the GPU calls.
sidebar:
  order: 4
---

The GPU's own API is a handful of calls: set a colour, write a string, fill
a rectangle. Each one costs a little of the computer's per-tick call budget,
so drawing a picture one cell at a time is slow.

`wackasm_opencomputers::draw` handles that for you. You draw into a
`Canvas` in your program's memory, as freely as you like, then call
`present()`. It compares the canvas with what's on the screen and sends only
the cells that changed, merged into as few GPU calls as it can.

```rust
use wackasm_opencomputers::{draw::{Canvas, Rgb, Shapes}, prelude::*};

fn main() -> Result<()> {
    let mut canvas = Canvas::new(Gpu::first().ok_or(Error::NoSuchComponent)?)?;
    canvas.clear(Rgb::BLACK);
    canvas.text_centered(0, "hello from wasm", Rgb::WHITE, Rgb::BLACK);

    let mut px = canvas.pixels();
    px.fill_circle(20, 12, 6, Rgb::hsv(0.12, 1.0, 1.0));
    px.line(0, 2, 40, 20, Rgb::new(0x33, 0xCC, 0xFF));

    canvas.present()?;
    Ok(())
}
```

## Drawing

Coordinates count from **0** at the top left, and anything outside the
canvas is clipped.

| | does |
|---|---|
| `Canvas::new(gpu)` | a canvas the size of the screen, at the GPU's deepest colour depth |
| `clear(bg)`, `fill(x, y, w, h, ch, fg, bg)`, `set(x, y, ch, fg, bg)` | cells |
| `text(x, y, s, fg, bg)`, `text_centered(y, ...)`, `text_vertical(x, y, ...)` | text |
| `pixels()` | **half-block pixels**: two per cell, top and bottom, each with its own colour. A 80x25 screen is 80x50 pixels |
| `dots()` | **braille dots**: 2x4 per cell, sharing one colour per cell. A 80x25 screen is 160x100 dots |

Both `pixels()` and `dots()` support the `Shapes` methods: `put`, `line`,
`rect`, `fill_rect`, `circle`, `fill_circle`, and `plot(f, colour)` for a
function from 0..1 across to -1..1 up. Pixels also have `get`; dots have
`unset` and `is_set`.

Colours are `Rgb`: `Rgb::new(r, g, b)`, `Rgb(0xRRGGBB)`, `Rgb::hsv(h, s, v)`
and `a.lerp(b, t)`, with constants such as `Rgb::WHITE`. The GPU shows the
nearest colour its tier supports.

## Presenting

`present()` returns how many GPU calls it made. For each row it sends
runs of changed cells that share colours as one `set` call each (spaces
match any text colour), and changes a colour only when it differs from the
last one set. So a frame where a small sprite moves costs a few calls, even
if you redrew the whole scene in memory.

- `double_buffered(true)` composes each present in a GPU buffer, then copies
  it to the screen with one `bitblt`, so the screen never shows a half-drawn
  frame. It returns `false` if the GPU has no video memory to spare.
- `invalidate()` makes the next present redraw everything. Call it if
  something else drew on the screen.
- With the [async feature](/wackasm/programs/async/), `present_async()` queues
  every call at once, and other tasks keep running.

## Animation

Redraw the whole frame, present, wait, repeat:

```rust
use std::time::Duration;

let (w, h) = (canvas.width() as i64, canvas.height() as i64 * 2);
let mut x = 0;
loop {
    let mut px = canvas.pixels();
    px.fill_rect(0, 0, w, h, Rgb(0x102030));
    px.fill_circle(x, h / 2, 6, Rgb::YELLOW);
    canvas.present()?; // only the cells around the ball
    x = (x + 1) % w;
    if signal::pull(Some(Duration::from_millis(50))).is_some() {
        break;
    }
}
```

The `gfx` program on the shell floppy shows all of this: `gfx` runs a
slideshow, and `gfx 1` to `gfx 4` show one scene (colour, braille, bars,
animation). The animation's caption shows how many GPU calls each frame took.

Every type and method is in the [reference for `draw`](/wackasm/rustdoc/wackasm_opencomputers/draw/index.html).

## The raw GPU

Everything the GPU can do is also on the `Gpu` component wrapper
directly: `set`, `fill`, `copy`, colours and palettes, depth and
resolution, and video buffers (`allocate_buffer`, `bitblt`, ...). See
[Components and signals](/wackasm/programs/components/).
