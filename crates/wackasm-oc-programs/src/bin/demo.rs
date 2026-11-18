//! `demo`: the `wackasm-opencomputers` crate in action. It lists components, draws a
//! box straight onto the screen through the GPU, and waits five seconds for
//! a key.

use std::time::Duration;

use wackasm_opencomputers::{component, prelude::*};

fn main() -> Result<()> {
    println!(
        "computer {} up {:?}, {} KiB for this program",
        computer::address(),
        computer::uptime(),
        computer::memory_limit() / 1024
    );
    for (address, kind) in component::list("") {
        println!("  {kind:<11} {address}");
    }

    // Drawing directly, next to the shell's text.
    let gpu = Gpu::first().ok_or(Error::NoSuchComponent)?;
    let (w, h) = gpu.get_resolution()?;
    let (w, h) = (w.as_i64(), h.as_i64());
    let (bw, bh) = (22, 5);
    let (x, y) = (w - bw + 1, h - bh - 1);
    gpu.fill(x, y, bw, bh, "#")?;
    gpu.fill(x + 1, y + 1, bw - 2, bh - 2, " ")?;
    gpu.set(x + 2, y + 2, "drawn by Gpu::set", None)?;

    println!("press a key within 5s...");
    match signal::pull_filtered(Some(Duration::from_secs(5)), |s| matches!(s, Signal::KeyDown { .. })) {
        Some(Signal::KeyDown { char, code, player, .. }) => {
            println!("{player} pressed {:?} (code {code})", char::from_u32(char).unwrap_or('?'));
        }
        _ => println!("no key pressed"),
    }
    computer::beep(880, Duration::from_millis(100));
    Ok(())
}
