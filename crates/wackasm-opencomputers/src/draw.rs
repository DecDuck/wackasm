//! Easy, efficient drawing: draw into a [`Canvas`] in your program's memory,
//! then [`Canvas::present`] sends only what changed to the GPU, in as few
//! calls as it can.
//!
//! ```no_run
//! use wackasm_opencomputers::{draw::{Canvas, Rgb, Shapes}, prelude::*};
//!
//! let mut canvas = Canvas::new(Gpu::first().ok_or(Error::NoSuchComponent)?)?;
//! canvas.clear(Rgb::BLACK);
//! canvas.text(1, 0, "hello", Rgb::WHITE, Rgb::BLACK);
//!
//! // Half-block pixels: two square-ish pixels per character cell.
//! let mut px = canvas.pixels();
//! px.fill_circle(20, 12, 6, Rgb::hsv(0.12, 1.0, 1.0));
//! px.line(0, 2, 40, 20, Rgb::new(0x33, 0xCC, 0xFF));
//!
//! // Braille dots: 2x4 per cell, one colour per cell.
//! let mut dots = canvas.dots();
//! dots.circle(60, 30, 10, Rgb::WHITE);
//!
//! let calls = canvas.present()?; // later presents only send changes
//! # Ok::<(), Error>(())
//! ```
//!
//! # Coordinates
//!
//! Everything here counts from **0** at the top left (OpenComputers' own GPU
//! calls count from 1): cells are `0..width() x 0..height()`, half-block
//! pixels `0..width() x 0..height() * 2`, and braille dots
//! `0..width() * 2 x 0..height() * 4`. Drawing outside is clipped.
//!
//! # How drawing modes mix
//!
//! A cell holds one character with a foreground and a background colour.
//! Half-block pixels turn a cell into `▀` (top pixel in front, bottom pixel
//! behind); braille dots turn it into a braille character in one colour,
//! over the cell's background. Writing one kind of graphics over another
//! replaces it, keeping the cell's background colour.
//!
//! # What `present` sends
//!
//! It compares the canvas with what it last sent, and for each row writes
//! the changed cells as runs: one `gpu.set` per run of cells sharing colours
//! (a space matches any foreground), plus a colour change only when the
//! colour differs from the last one set. So redrawing a moving sprite costs a
//! few calls, not a screen's worth. With [`Canvas::double_buffered`], changes
//! go to a GPU buffer first and reach the screen in one `bitblt`, which
//! avoids tearing. If something else draws on the screen, call
//! [`Canvas::invalidate`] so the next present redraws everything.

#[cfg(feature = "async")]
use crate::component::Component;
use crate::{
    components::Gpu,
    error::Result,
    value::Number,
};

/// A 24-bit colour. The GPU shows the nearest colour its tier supports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb(pub u32);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0x000000);
    pub const WHITE: Rgb = Rgb(0xFFFFFF);
    pub const GREY: Rgb = Rgb(0x808080);
    pub const RED: Rgb = Rgb(0xFF0000);
    pub const GREEN: Rgb = Rgb(0x00FF00);
    pub const BLUE: Rgb = Rgb(0x0000FF);
    pub const YELLOW: Rgb = Rgb(0xFFFF00);
    pub const CYAN: Rgb = Rgb(0x00FFFF);
    pub const MAGENTA: Rgb = Rgb(0xFF00FF);

    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    /// From hue (0..1, wrapping), saturation and value (0..1).
    pub fn hsv(hue: f64, saturation: f64, value: f64) -> Rgb {
        let h = hue.rem_euclid(1.0) * 6.0;
        let (s, v) = (saturation.clamp(0.0, 1.0), value.clamp(0.0, 1.0));
        let c = v * s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        let byte = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Rgb::new(byte(r), byte(g), byte(b))
    }

    /// The colour `t` (0..1) of the way from `self` to `other`.
    pub fn lerp(self, other: Rgb, t: f64) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
        Rgb::new(mix(self.r(), other.r()), mix(self.g(), other.g()), mix(self.b(), other.b()))
    }
}

impl From<Rgb> for Number {
    fn from(c: Rgb) -> Number {
        Number::Int(c.0 as i64)
    }
}

const HALF: char = '▀';

#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    ch: char,
    fg: Rgb,
    bg: Rgb,
}

/// One GPU call `present` makes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Op {
    Fg(Rgb),
    Bg(Rgb),
    /// 1-based, as the GPU wants.
    Text { x: i64, y: i64, text: String, vertical: bool },
    Fill { x: i64, y: i64, w: i64, h: i64, ch: char },
}

struct Run {
    len: usize,
    changed: usize,
    fg: Option<Rgb>,
}

struct Rect {
    w: usize,
    h: usize,
    changed: usize,
}

/// A framebuffer for one GPU's screen; see the [module docs](self).
pub struct Canvas {
    gpu: Gpu,
    w: usize,
    h: usize,
    cells: Vec<Cell>,
    /// What the GPU shows (`None`: unknown, so it is sent next time).
    shown: Vec<Option<Cell>>,
    gpu_fg: Option<Rgb>,
    gpu_bg: Option<Rgb>,
    /// The GPU buffer changes go to first, with `double_buffered`.
    vram: Option<Number>,
}

impl Canvas {
    /// A canvas the size of the GPU's current resolution, at its deepest
    /// colour depth.
    pub fn new(gpu: Gpu) -> Result<Canvas> {
        let (w, h) = gpu.get_resolution()?;
        if let Ok(depth) = gpu.max_depth() {
            let _ = gpu.set_depth(depth);
        }
        Ok(Canvas::with_size(gpu, w.as_i64().max(1) as usize, h.as_i64().max(1) as usize))
    }

    /// A canvas of `w` x `h` cells, drawn from the screen's top left corner.
    /// Makes no GPU calls until [`Canvas::present`].
    pub fn with_size(gpu: Gpu, w: usize, h: usize) -> Canvas {
        let blank = Cell { ch: ' ', fg: Rgb::WHITE, bg: Rgb::BLACK };
        Canvas {
            gpu,
            w,
            h,
            cells: vec![blank; w * h],
            shown: vec![None; w * h],
            gpu_fg: None,
            gpu_bg: None,
            vram: None,
        }
    }

    /// Width in cells.
    pub fn width(&self) -> usize {
        self.w
    }

    /// Height in cells.
    pub fn height(&self) -> usize {
        self.h
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// Composes each present in a GPU buffer and copies it to the screen in
    /// one `bitblt`, so the screen never shows a half-drawn frame. Returns
    /// `false` (and keeps drawing directly) if the GPU has no video memory to
    /// spare. `double_buffered(false)` frees the buffer.
    pub fn double_buffered(&mut self, on: bool) -> Result<bool> {
        match (on, self.vram) {
            (true, Some(_)) => Ok(true),
            (true, None) => match self.gpu.allocate_buffer(Some(Number::from(self.w)), Some(Number::from(self.h))) {
                Ok(id) => {
                    self.vram = Some(id);
                    // The buffer starts empty: send everything next time.
                    self.invalidate();
                    Ok(true)
                }
                Err(_) => Ok(false),
            },
            (false, Some(id)) => {
                self.vram = None;
                self.invalidate();
                self.gpu.free_buffer(id).map(|_| false)
            }
            (false, None) => Ok(false),
        }
    }

    /// Forgets what the screen shows, so the next present redraws every
    /// cell. Call it after anything else draws on the screen.
    pub fn invalidate(&mut self) {
        self.shown.fill(None);
        self.gpu_fg = None;
        self.gpu_bg = None;
    }

    fn index(&self, x: i64, y: i64) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h).then(|| y as usize * self.w + x as usize)
    }

    /// The character and colours of cell (x, y).
    pub fn get(&self, x: i64, y: i64) -> Option<(char, Rgb, Rgb)> {
        self.index(x, y).map(|i| (self.cells[i].ch, self.cells[i].fg, self.cells[i].bg))
    }

    /// Sets one cell.
    pub fn set(&mut self, x: i64, y: i64, ch: char, fg: Rgb, bg: Rgb) {
        if let Some(i) = self.index(x, y) {
            self.cells[i] = Cell { ch, fg, bg };
        }
    }

    /// Every cell blank in `bg`.
    pub fn clear(&mut self, bg: Rgb) {
        self.cells.fill(Cell { ch: ' ', fg: Rgb::WHITE, bg });
    }

    /// Fills a rectangle of cells with `ch`.
    #[allow(clippy::too_many_arguments)]
    pub fn fill(&mut self, x: i64, y: i64, w: i64, h: i64, ch: char, fg: Rgb, bg: Rgb) {
        for yy in y.max(0)..(y + h).min(self.h as i64) {
            for xx in x.max(0)..(x + w).min(self.w as i64) {
                self.set(xx, yy, ch, fg, bg);
            }
        }
    }

    /// Writes `text` from cell (x, y) rightwards, clipped at the edge.
    pub fn text(&mut self, x: i64, y: i64, text: &str, fg: Rgb, bg: Rgb) {
        for (i, ch) in text.chars().enumerate() {
            self.set(x + i as i64, y, ch, fg, bg);
        }
    }

    /// `text` centred on row `y`.
    pub fn text_centered(&mut self, y: i64, text: &str, fg: Rgb, bg: Rgb) {
        let x = (self.w as i64 - text.chars().count() as i64) / 2;
        self.text(x.max(0), y, text, fg, bg);
    }

    /// Writes `text` from cell (x, y) downwards.
    pub fn text_vertical(&mut self, x: i64, y: i64, text: &str, fg: Rgb, bg: Rgb) {
        for (i, ch) in text.chars().enumerate() {
            self.set(x, y + i as i64, ch, fg, bg);
        }
    }

    /// The canvas as half-block pixels: `width() x height() * 2`.
    pub fn pixels(&mut self) -> Pixels<'_> {
        Pixels { canvas: self }
    }

    /// The canvas as braille dots: `width() * 2 x height() * 4`.
    pub fn dots(&mut self) -> Dots<'_> {
        Dots { canvas: self }
    }

    /// The GPU calls that bring the screen up to date, recording that they
    /// were made.
    ///
    /// Greedy: from each changed cell not yet covered, in reading order, it
    /// takes whichever covers the most changed cells of a horizontal run, a
    /// vertical run (one `set` each) or a rectangle of identical cells (one
    /// `fill`). Runs share a background and, except for spaces, a foreground.
    pub(crate) fn plan(&mut self) -> Vec<Op> {
        let (w, h) = (self.w, self.h);
        let mut done: Vec<bool> = (0..w * h).map(|i| self.shown[i] == Some(self.cells[i])).collect();
        let mut ops = Vec::new();
        for y in 0..h {
            for x in 0..w {
                if done[y * w + x] {
                    continue;
                }
                let across = self.run(&done, x, y, true);
                let down = self.run(&done, x, y, false);
                let rect = self.rect(&done, x, y);
                let start = self.cells[y * w + x];
                let (cells, op) = if rect.changed > across.changed.max(down.changed) {
                    let cells: Vec<usize> =
                        (y..y + rect.h).flat_map(|yy| (x..x + rect.w).map(move |xx| yy * w + xx)).collect();
                    let op = Op::Fill { x: x as i64 + 1, y: y as i64 + 1, w: rect.w as i64, h: rect.h as i64, ch: start.ch };
                    (cells, (op, start.bg, (start.ch != ' ').then_some(start.fg)))
                } else {
                    let (run, vertical) = if down.changed > across.changed { (down, true) } else { (across, false) };
                    let cells: Vec<usize> =
                        (0..run.len).map(|k| if vertical { (y + k) * w + x } else { y * w + x + k }).collect();
                    let text: String = cells.iter().map(|&i| self.cells[i].ch).collect();
                    (cells, (Op::Text { x: x as i64 + 1, y: y as i64 + 1, text, vertical }, start.bg, run.fg))
                };
                let (op, bg, fg) = op;
                if self.gpu_bg != Some(bg) {
                    ops.push(Op::Bg(bg));
                    self.gpu_bg = Some(bg);
                }
                if let Some(fg) = fg
                    && self.gpu_fg != Some(fg)
                {
                    ops.push(Op::Fg(fg));
                    self.gpu_fg = Some(fg);
                }
                ops.push(op);
                for i in cells {
                    done[i] = true;
                    self.shown[i] = Some(self.cells[i]);
                }
            }
        }
        ops
    }

    /// The run from (x, y) across or down: cells sharing the start's
    /// background and a foreground (spaces match any), up to its last
    /// changed cell.
    fn run(&self, done: &[bool], x: usize, y: usize, across: bool) -> Run {
        let bg = self.cells[y * self.w + x].bg;
        let mut fg = None;
        let (mut len, mut changed, mut fg_at_end) = (0, 0, None);
        let limit = if across { self.w - x } else { self.h - y };
        let mut seen = 0;
        for k in 0..limit {
            let i = if across { y * self.w + x + k } else { (y + k) * self.w + x };
            let cell = self.cells[i];
            if cell.bg != bg {
                break;
            }
            if cell.ch != ' ' {
                if fg.is_some_and(|f| f != cell.fg) {
                    break;
                }
                fg = Some(cell.fg);
            }
            if !done[i] {
                seen += 1;
                len = k + 1;
                changed = seen;
                fg_at_end = fg;
            }
        }
        Run { len, changed, fg: fg_at_end }
    }

    /// The rectangle from (x, y) of cells identical to (x, y): as wide as the
    /// row allows, then as tall as every row of that width matches.
    fn rect(&self, done: &[bool], x: usize, y: usize) -> Rect {
        let start = self.cells[y * self.w + x];
        let width = (x..self.w).take_while(|&xx| self.cells[y * self.w + xx] == start).count();
        let height = (y..self.h)
            .take_while(|&yy| (x..x + width).all(|xx| self.cells[yy * self.w + xx] == start))
            .count();
        let changed = (y..y + height).flat_map(|yy| (x..x + width).map(move |xx| yy * self.w + xx)).filter(|&i| !done[i]).count();
        Rect { w: width, h: height, changed }
    }

    /// Sends what changed since the last present to the screen. Returns the
    /// number of GPU calls it made.
    pub fn present(&mut self) -> Result<usize> {
        let ops = self.plan();
        if ops.is_empty() {
            return Ok(0);
        }
        let result = self.send(&ops);
        if result.is_err() {
            self.invalidate(); // the screen is in an unknown state
        }
        result
    }

    fn send(&self, ops: &[Op]) -> Result<usize> {
        let mut calls = ops.len();
        if let Some(buffer) = self.vram {
            self.gpu.set_active_buffer(buffer)?;
        }
        for op in ops {
            match op {
                Op::Fg(c) => self.gpu.set_foreground(*c, None).map(|_| ())?,
                Op::Bg(c) => self.gpu.set_background(*c, None).map(|_| ())?,
                Op::Text { x, y, text, vertical } => {
                    self.gpu.set(*x, *y, text.as_str(), vertical.then_some(true)).map(|_| ())?
                }
                Op::Fill { x, y, w, h, ch } => self.gpu.fill(*x, *y, *w, *h, ch.to_string()).map(|_| ())?,
            }
        }
        if let Some(buffer) = self.vram {
            self.gpu.set_active_buffer(0)?;
            self.gpu.bitblt(Some(Number::Int(0)), None, None, None, None, Some(buffer), None, None)?;
            calls += 3;
        }
        Ok(calls)
    }

    /// [`Canvas::present`] without blocking: every call is queued at once
    /// (they run in order), and other tasks keep running. Requires the
    /// `async` feature; see [`crate::task`].
    #[cfg(feature = "async")]
    pub async fn present_async(&mut self) -> Result<usize> {
        let ops = self.plan();
        if ops.is_empty() {
            return Ok(0);
        }
        let gpu = &self.gpu;
        let mut calls: Vec<crate::task::Call> = Vec::with_capacity(ops.len() + 3);
        let (fg, bg) = ("setForeground", "setBackground");
        if let Some(buffer) = self.vram {
            calls.push(gpu.invoke_async("setActiveBuffer", vec![buffer.into()]));
        }
        for op in &ops {
            calls.push(match op {
                Op::Fg(c) => gpu.invoke_async(fg, vec![Number::from(*c).into()]),
                Op::Bg(c) => gpu.invoke_async(bg, vec![Number::from(*c).into()]),
                Op::Text { x, y, text, vertical } => {
                    let mut args = vec![(*x).into(), (*y).into(), text.as_str().into()];
                    if *vertical {
                        args.push(true.into());
                    }
                    gpu.invoke_async("set", args)
                }
                Op::Fill { x, y, w, h, ch } => gpu.invoke_async(
                    "fill",
                    vec![(*x).into(), (*y).into(), (*w).into(), (*h).into(), ch.to_string().as_str().into()],
                ),
            });
        }
        if let Some(buffer) = self.vram {
            calls.push(gpu.invoke_async("setActiveBuffer", vec![0.into()]));
            calls.push(gpu.invoke_async("bitblt", vec![0.into(), 1.into(), 1.into(), (self.w as i64).into(), (self.h as i64).into(), buffer.into()]));
        }
        let count = calls.len();
        let results = crate::task::join_all(calls).await;
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.invalidate();
            return Err(e);
        }
        Ok(count)
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        if let Some(buffer) = self.vram.take() {
            let _ = self.gpu.free_buffer(buffer);
        }
    }
}

/// Lines, rectangles and circles, for anything that can set a point.
pub trait Shapes {
    /// Size in points.
    fn size(&self) -> (i64, i64);
    /// Sets one point; outside the surface it does nothing.
    fn put(&mut self, x: i64, y: i64, colour: Rgb);

    fn line(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, colour: Rgb) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.put(x, y, colour);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    fn rect(&mut self, x: i64, y: i64, w: i64, h: i64, colour: Rgb) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.line(x, y, x + w - 1, y, colour);
        self.line(x, y + h - 1, x + w - 1, y + h - 1, colour);
        self.line(x, y, x, y + h - 1, colour);
        self.line(x + w - 1, y, x + w - 1, y + h - 1, colour);
    }

    fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, colour: Rgb) {
        let (sw, sh) = self.size();
        for yy in y.max(0)..(y + h).min(sh) {
            for xx in x.max(0)..(x + w).min(sw) {
                self.put(xx, yy, colour);
            }
        }
    }

    fn circle(&mut self, cx: i64, cy: i64, r: i64, colour: Rgb) {
        let (mut x, mut y, mut err) = (r, 0, 1 - r);
        while x >= y {
            for (px, py) in [(x, y), (y, x), (-y, x), (-x, y), (-x, -y), (-y, -x), (y, -x), (x, -y)] {
                self.put(cx + px, cy + py, colour);
            }
            y += 1;
            if err < 0 {
                err += 2 * y + 1;
            } else {
                x -= 1;
                err += 2 * (y - x) + 1;
            }
        }
    }

    fn fill_circle(&mut self, cx: i64, cy: i64, r: i64, colour: Rgb) {
        // Radius r + 1/2 (r^2 + r), so the edges are round rather than
        // ending in single-pixel points.
        for dy in -r..=r {
            let half = ((r * r + r - dy * dy) as f64).sqrt() as i64;
            for dx in -half..=half {
                self.put(cx + dx, cy + dy, colour);
            }
        }
    }

    /// `f` from 0 to 1 across the width, with -1 at the bottom and 1 at the
    /// top, joined so steep parts have no gaps.
    fn plot(&mut self, f: impl Fn(f64) -> f64, colour: Rgb)
    where
        Self: Sized,
    {
        let (w, h) = self.size();
        if w < 1 || h < 1 {
            return;
        }
        let y_at = |x: i64| ((1.0 - f(x as f64 / (w - 1).max(1) as f64)) / 2.0 * (h - 1) as f64).round() as i64;
        let mut prev = y_at(0);
        for x in 0..w {
            let y = y_at(x);
            for yy in prev.min(y)..=prev.max(y) {
                self.put(x, yy, colour);
            }
            prev = y;
        }
    }
}

/// The canvas as half-block pixels, two per cell (top and bottom); see
/// [`Canvas::pixels`].
pub struct Pixels<'a> {
    canvas: &'a mut Canvas,
}

impl Pixels<'_> {
    /// The colour of pixel (x, y).
    pub fn get(&self, x: i64, y: i64) -> Option<Rgb> {
        let (ch, fg, bg) = self.canvas.get(x, y.div_euclid(2))?;
        Some(if ch == HALF && y % 2 == 0 { fg } else { bg })
    }

    pub fn clear(&mut self, colour: Rgb) {
        self.canvas.clear(colour);
    }
}

impl Shapes for Pixels<'_> {
    fn size(&self) -> (i64, i64) {
        (self.canvas.w as i64, self.canvas.h as i64 * 2)
    }

    fn put(&mut self, x: i64, y: i64, colour: Rgb) {
        if y < 0 {
            return;
        }
        let cy = y / 2;
        let Some(i) = self.canvas.index(x, cy) else { return };
        let cell = self.canvas.cells[i];
        let (mut top, mut bottom) = if cell.ch == HALF { (cell.fg, cell.bg) } else { (cell.bg, cell.bg) };
        if y % 2 == 0 {
            top = colour;
        } else {
            bottom = colour;
        }
        // Two equal halves are just a background, which merges into runs.
        self.canvas.cells[i] = if top == bottom {
            Cell { ch: ' ', fg: cell.fg, bg: top }
        } else {
            Cell { ch: HALF, fg: top, bg: bottom }
        };
    }
}

/// The canvas as braille dots, 2x4 per cell; see [`Canvas::dots`]. A cell's
/// dots share one colour: the last one drawn there.
pub struct Dots<'a> {
    canvas: &'a mut Canvas,
}

/// Braille dot bits by column and row within a cell.
const BRAILLE: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

impl Dots<'_> {
    /// Turns dot (x, y) off.
    pub fn unset(&mut self, x: i64, y: i64) {
        self.update(x, y, None);
    }

    /// Whether dot (x, y) is on.
    pub fn is_set(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 {
            return false;
        }
        match self.canvas.get(x / 2, y / 4) {
            Some((ch, _, _)) if ('\u{2800}'..='\u{28FF}').contains(&ch) => {
                (ch as u32 - 0x2800) & BRAILLE[(x % 2) as usize][(y % 4) as usize] != 0
            }
            _ => false,
        }
    }

    /// Turns every dot off, keeping cell backgrounds.
    pub fn clear(&mut self) {
        for cell in &mut self.canvas.cells {
            if ('\u{2800}'..='\u{28FF}').contains(&cell.ch) {
                cell.ch = ' ';
            }
        }
    }

    fn update(&mut self, x: i64, y: i64, colour: Option<Rgb>) {
        if x < 0 || y < 0 {
            return;
        }
        let Some(i) = self.canvas.index(x / 2, y / 4) else { return };
        let cell = &mut self.canvas.cells[i];
        let mut bits = if ('\u{2800}'..='\u{28FF}').contains(&cell.ch) { cell.ch as u32 - 0x2800 } else { 0 };
        let bit = BRAILLE[(x % 2) as usize][(y % 4) as usize];
        match colour {
            Some(c) => {
                bits |= bit;
                cell.fg = c;
            }
            None => bits &= !bit,
        }
        cell.ch = if bits == 0 { ' ' } else { char::from_u32(0x2800 + bits).expect("braille range") };
    }
}

impl Shapes for Dots<'_> {
    fn size(&self) -> (i64, i64) {
        (self.canvas.w as i64 * 2, self.canvas.h as i64 * 4)
    }

    fn put(&mut self, x: i64, y: i64, colour: Rgb) {
        self.update(x, y, Some(colour));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{Address, Component};

    fn canvas(w: usize, h: usize) -> Canvas {
        Canvas::with_size(Gpu::from_address(Address("gpu".into())), w, h)
    }

    fn text(x: i64, y: i64, text: &str) -> Op {
        Op::Text { x, y, text: text.into(), vertical: false }
    }

    fn down(x: i64, y: i64, text: &str) -> Op {
        Op::Text { x, y, text: text.into(), vertical: true }
    }

    #[test]
    fn the_first_present_sends_everything_then_only_changes() {
        let mut c = canvas(4, 2);
        c.text(0, 0, "ab", Rgb::WHITE, Rgb::BLACK);
        assert_eq!(c.plan(), [Op::Bg(Rgb::BLACK), Op::Fg(Rgb::WHITE), text(1, 1, "ab  "), text(1, 2, "    ")]);
        assert_eq!(c.plan(), []);
        c.set(1, 1, 'x', Rgb::WHITE, Rgb::BLACK);
        // Colours are already set: one call.
        assert_eq!(c.plan(), [text(2, 2, "x")]);
    }

    #[test]
    fn runs_break_on_colour_and_spaces_match_any_foreground() {
        let mut c = canvas(5, 1);
        c.text(0, 0, "a", Rgb::RED, Rgb::BLACK);
        c.text(2, 0, "b", Rgb::RED, Rgb::BLACK); // the space between joins the run
        c.text(3, 0, "c", Rgb::BLUE, Rgb::BLACK);
        assert_eq!(
            c.plan(),
            [Op::Bg(Rgb::BLACK), Op::Fg(Rgb::RED), text(1, 1, "a b"), Op::Fg(Rgb::BLUE), text(4, 1, "c ")]
        );
    }

    #[test]
    fn unchanged_cells_inside_a_run_are_resent_rather_than_split() {
        let mut c = canvas(5, 1);
        c.plan();
        c.set(0, 0, 'x', Rgb::WHITE, Rgb::BLACK);
        c.set(3, 0, 'y', Rgb::WHITE, Rgb::BLACK);
        assert_eq!(c.plan(), [Op::Fg(Rgb::WHITE), text(1, 1, "x  y")]);
    }

    #[test]
    fn columns_of_one_colour_go_down_and_blocks_are_filled() {
        let mut c = canvas(3, 4);
        c.plan();
        // A gradient: each column its own background, so one call per column
        // (a vertical set; a 1-wide fill would cost the same).
        for (x, colour) in [Rgb::RED, Rgb::GREEN, Rgb::BLUE].into_iter().enumerate() {
            c.fill(x as i64, 0, 1, 4, ' ', Rgb::WHITE, colour);
        }
        assert_eq!(
            c.plan(),
            [
                Op::Bg(Rgb::RED),
                down(1, 1, "    "),
                Op::Bg(Rgb::GREEN),
                down(2, 1, "    "),
                Op::Bg(Rgb::BLUE),
                down(3, 1, "    "),
            ]
        );
        // A clear is one fill.
        c.clear(Rgb::GREY);
        assert_eq!(c.plan(), [Op::Bg(Rgb::GREY), Op::Fill { x: 1, y: 1, w: 3, h: 4, ch: ' ' }]);
        // A vertical bar of mixed characters is one vertical set.
        c.text_vertical(1, 0, "█▄", Rgb::YELLOW, Rgb::GREY);
        assert_eq!(c.plan(), [Op::Fg(Rgb::YELLOW), down(2, 1, "█▄")]);
    }

    #[test]
    fn half_block_pixels() {
        let mut c = canvas(2, 1);
        let mut px = c.pixels();
        px.put(0, 0, Rgb::RED);
        px.put(0, 1, Rgb::BLUE);
        px.put(1, 0, Rgb::GREEN);
        px.put(1, 1, Rgb::GREEN);
        assert_eq!(px.get(0, 0), Some(Rgb::RED));
        assert_eq!(px.get(0, 1), Some(Rgb::BLUE));
        assert_eq!(c.get(0, 0), Some(('▀', Rgb::RED, Rgb::BLUE)));
        // Equal halves become a plain background.
        assert_eq!(c.get(1, 0).map(|(ch, _, bg)| (ch, bg)), Some((' ', Rgb::GREEN)));
    }

    #[test]
    fn braille_dots() {
        let mut c = canvas(1, 1);
        let mut dots = c.dots();
        dots.put(0, 0, Rgb::WHITE); // dot 1
        dots.put(1, 3, Rgb::WHITE); // dot 8
        assert!(dots.is_set(1, 3) && !dots.is_set(1, 2));
        assert_eq!(c.get(0, 0).map(|(ch, ..)| ch), Some('\u{2881}'));
        let mut dots = c.dots();
        dots.unset(0, 0);
        dots.unset(1, 3);
        assert_eq!(c.get(0, 0).map(|(ch, ..)| ch), Some(' '));
    }

    #[test]
    fn shapes_are_clipped() {
        let mut c = canvas(3, 2);
        let mut px = c.pixels();
        px.fill_circle(1, 1, 5, Rgb::RED);
        px.line(-10, -10, 10, 10, Rgb::BLUE);
        assert_eq!(px.size(), (3, 4));
        assert_eq!(px.get(2, 3), Some(Rgb::RED));
        assert_eq!(px.get(1, 1), Some(Rgb::BLUE));
    }

    #[test]
    fn hsv_and_lerp() {
        assert_eq!(Rgb::hsv(0.0, 1.0, 1.0), Rgb::RED);
        assert_eq!(Rgb::hsv(1.0 / 3.0, 1.0, 1.0), Rgb::GREEN);
        assert_eq!(Rgb::hsv(0.5, 0.0, 0.5), Rgb::new(128, 128, 128));
        assert_eq!(Rgb::BLACK.lerp(Rgb::WHITE, 0.5), Rgb::new(128, 128, 128));
    }
}
