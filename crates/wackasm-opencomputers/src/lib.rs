//! Typed Rust bindings to OpenComputers, for programs running on the
//! WebAssembly (wackasm) architecture.
//!
//! Build an ordinary WASI program (`--target wasm32-wasip1`) and use
//! components directly:
//!
//! ```no_run
//! use wackasm_opencomputers::prelude::*;
//!
//! let gpu = Gpu::first().expect("no GPU");
//! let (w, h) = gpu.get_resolution()?;
//! gpu.fill(1, 1, w, h, " ")?;
//! gpu.set(1, 1, "hello from Rust", None)?;
//!
//! // Wait for a key.
//! let key = signal::pull_filtered(None, |s| matches!(s, Signal::KeyDown { .. }));
//! # Ok::<(), wackasm_opencomputers::Error>(())
//! ```
//!
//! * [`components`]: a generated struct per component type (`Gpu`,
//!   `Filesystem`, `Robot`, `Modem`, `Redstone`, ...), with a method for every
//!   callback and OpenComputers' own docs.
//! * [`component`]: finding components, and untyped calls ([`Proxy`]).
//! * [`signal`]: typed events.
//! * [`computer`]: this computer's address, uptime, memory and power.
//! * [`process`]: running other programs, like `std::process`.
//! * [`draw`]: a framebuffer with text, half-block pixels, braille and
//!   shapes, sent to the GPU with as few calls as possible.
//! * [`internet`]: HTTP requests and TCP connections through an internet card.
//! * [`pattern_provider`]: autocrafting patterns for Applied Energistics 2
//!   that the program fulfils (with AE2 installed).
//! * `task` (with the `async` feature): an async executor, so programs can
//!   wait on several component calls, timers and signals at once. Every
//!   component method then has an `_async` twin.
//! * [`Side`], [`Color`], [`keys`]: the numeric codes components use.
//!
//! Files are simplest through `std::fs`, which the architecture maps onto a
//! disk. The `Filesystem` wrapper is there for other disks.
//!
//! Numbers cross as [`Number`]: an integer or a float, as OpenComputers sent
//! it, so integers stay exact.

pub mod component;
pub mod components;
pub mod computer;
pub mod draw;
mod error;
pub mod internet;
pub mod pattern_provider;
pub mod process;
pub mod signal;
mod sys;
#[cfg(feature = "async")]
pub mod task;
mod types;
mod value;

pub use component::{Address, Component, MethodInfo, Proxy};
pub use error::{Error, Result};
pub use signal::Signal;
pub use types::{Color, Side, keys};
pub use value::{Handle, Number, OcString, Value};

/// Everything a typical program needs: `use wackasm_opencomputers::prelude::*;`
pub mod prelude {
    pub use crate::{
        Address, Color, Component, Error, Handle, Number, OcString, Proxy, Result, Side, Signal, Value,
        components::*, computer, keys,
        internet,
        pattern_provider::{self, PatternProvider},
        process::{self, Command},
        signal,
    };
}
