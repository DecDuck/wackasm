//! An OpenComputers architecture that runs WebAssembly on wackasm.
//!
//! The crate builds as a `cdylib` (`wackasm_oc`) that the Java classes in
//! `mc-mod/` load. The guest-facing ABI is in [`abi`], the value encoding in
//! [`value`], and the state machine that suspends and resumes guests in
//! [`machine`]. See `README.md` for the full protocol.

pub mod abi;
pub mod cache;
mod jni;
pub mod machine;
mod process;
pub mod storage;
pub mod value;
mod wasi;

pub use machine::{AsyncTarget, OcMachine, Request, RunOutcome};
pub use storage::WorkDir;
pub use value::OcValue;
