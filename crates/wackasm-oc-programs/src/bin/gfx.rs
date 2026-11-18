//! `gfx [SCENE]`: a tour of what the GPU can draw, using the
//! `wackasm-opencomputers` crate's `draw` module.
//!
//! 1. colour: a hue gradient of background colours;
//! 2. braille: sine and cosine plotted at 2x4 dots per character;
//! 3. bars: a bar chart with eighth-height block characters;
//! 4. animation: a ball bouncing among stars in half-block pixels, for 10
//!    seconds. Each frame redraws the whole scene in memory, and `present`
//!    sends only the cells that changed (the caption shows how many GPU
//!    calls a frame takes).
//!
//! Without SCENE it shows each scene in turn: any key moves on, `q` quits,
//! and scenes move on by themselves after a few seconds. `gfx N` shows only
//! scene N and waits for a key. The screen is cleared on exit.

use std::{f64::consts::TAU, time::Duration};

use wackasm_opencomputers::{
    computer,
    draw::{Canvas, Rgb, Shapes},
    prelude::*,
    signal,
};

type Res<T> = wackasm_opencomputers::Result<T>;

const SCENES: [fn(&mut Canvas) -> Res<()>; 4] = [colour, braille, bars, animation];

fn colour(c: &mut Canvas) -> Res<()> {
    let (w, h) = (c.width() as i64, c.height() as i64);
    for x in 0..w {
        c.fill(x, 0, 1, h, ' ', Rgb::WHITE, Rgb::hsv(x as f64 / w as f64, 1.0, 1.0));
    }
    let band = Rgb(0x101010);
    c.fill(0, h / 2 - 1, w, 3, ' ', Rgb::WHITE, band);
    c.text_centered(h / 2, "24-bit colour, mapped to the GPU's palette", Rgb::WHITE, band);
    c.present().map(|_| ())
}

fn braille(c: &mut Canvas) -> Res<()> {
    c.clear(Rgb::BLACK);
    let mut dots = c.dots();
    dots.plot(|_| 0.0, Rgb(0x444444));
    dots.plot(|t| (t * TAU * 2.0).cos() * 0.8, Rgb(0x33CCFF));
    dots.plot(|t| (t * TAU * 2.0).sin() * 0.9, Rgb(0xFFCC00));
    let (w, h) = dots.size();
    c.text_centered(0, &format!("braille: {w}x{h} dots"), Rgb::WHITE, Rgb::BLACK);
    c.present().map(|_| ())
}

fn bars(c: &mut Canvas) -> Res<()> {
    let back = Rgb(0x0A0A1A);
    c.clear(back);
    let top = 2;
    let height = (c.height() as i64 - top).max(1);
    let count = ((c.width() as i64 - 2) / 3).clamp(1, 24);
    const PARTS: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];
    for i in 0..count {
        let value = 0.55 + 0.45 * ((i as f64 * 0.7).sin() * (i as f64 * 0.23 + 1.0).cos());
        let eighths = (value * (height * 8) as f64).round() as i64;
        let (full, part) = (eighths / 8, (eighths % 8) as usize);
        let column: String = (0..height)
            .map(|row| {
                let from_bottom = height - 1 - row;
                if from_bottom < full {
                    '█'
                } else if from_bottom == full {
                    PARTS[part]
                } else {
                    ' '
                }
            })
            .collect();
        let colour = Rgb::hsv(i as f64 / count as f64, 1.0, 1.0);
        c.text_vertical(1 + i * 3, top, &column, colour, back);
        c.text_vertical(2 + i * 3, top, &column, colour, back);
    }
    c.text_centered(0, "bars: eighth-height blocks", Rgb::WHITE, back);
    c.present().map(|_| ())
}

fn animation(c: &mut Canvas) -> Res<()> {
    /// How long the scene runs, in game time (a key stops it sooner).
    const LENGTH: Duration = Duration::from_secs(10);
    let back = Rgb(0x101828);
    let buffered = c.double_buffered(true)?;
    let (w, h) = (c.width() as i64, c.height() as i64 * 2);
    let r = (h as f64 * 0.12).clamp(2.0, 6.0) as i64;
    let (mut bx, mut by, mut vx, mut vy) = (r + 1, r + 2, 2, 1);
    let started = computer::uptime();
    let (mut frames, mut last_calls, mut caption, mut caption_at) = (0u64, 0usize, String::new(), started);
    while computer::uptime() - started < LENGTH {
        // The whole frame, every time; present sends only what changed.
        // A flat backdrop and static stars cost nothing after the first
        // frame, so each frame is just the cells at the ball's edges.
        c.clear(back);
        let mut dots = c.dots();
        let (dw, dh) = dots.size();
        for i in 0..24i64 {
            dots.put((i * 37 + 11) % dw, (i * 53 + 7) % dh, Rgb(0x8899AA));
        }
        c.pixels().fill_circle(bx, by, r, Rgb(0xFFDD33));
        // The caption changes once a second, not every frame.
        let now = computer::uptime();
        if caption.is_empty() || now - caption_at >= Duration::from_secs(1) {
            let mode = if buffered { "double-buffered" } else { "direct" };
            caption = format!(" half-block pixels, {mode}: {last_calls} GPU calls last frame, {frames} frames ");
            caption_at = now;
        }
        c.text(0, 0, &caption, Rgb::WHITE, Rgb::BLACK);
        last_calls = c.present()?;
        frames += 1;

        bx += vx;
        by += vy;
        if bx - r <= 0 || bx + r >= w - 1 {
            vx = -vx;
        }
        if by - r <= 2 || by + r >= h - 1 {
            vy = -vy;
        }
        if key(Some(Duration::from_millis(50))).is_some() {
            break;
        }
    }
    c.double_buffered(false)?;
    Ok(())
}

/// The next key pressed within `timeout` (`None` waits forever).
fn key(timeout: Option<Duration>) -> Option<u32> {
    let deadline = timeout.map(|t| computer::uptime() + t);
    loop {
        let left = deadline.map(|d| d.saturating_sub(computer::uptime()));
        match signal::pull(left) {
            Some(Signal::KeyDown { char, .. }) => return Some(char),
            Some(_) => {}
            None => return None,
        }
    }
}

fn main() {
    let gpu = Gpu::first().unwrap_or_else(|| {
        eprintln!("gfx: no GPU");
        std::process::exit(1);
    });
    let mut canvas = Canvas::new(gpu).unwrap_or_else(|e| {
        eprintln!("gfx: {e}");
        std::process::exit(1);
    });

    let only = std::env::args().nth(1).and_then(|a| a.parse::<usize>().ok());
    let result = match only {
        Some(n) if (1..=SCENES.len()).contains(&n) => (SCENES[n - 1])(&mut canvas).map(|_| {
            key(None);
        }),
        Some(n) => {
            eprintln!("gfx: scene {n}? there are {}", SCENES.len());
            Ok(())
        }
        None => (|| {
            for scene in SCENES {
                scene(&mut canvas)?;
                if key(Some(Duration::from_secs(4))) == Some('q' as u32) {
                    break;
                }
            }
            Ok(())
        })(),
    };

    // Leave a clean screen for the shell.
    canvas.clear(Rgb::BLACK);
    let _ = canvas.present();
    if let Err(e) = result {
        eprintln!("gfx: {e}");
        std::process::exit(1);
    }
}
