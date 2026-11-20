//! Events: key presses, screen touches, network messages, components
//! appearing, and so on. OpenComputers queues them per computer.

use std::time::Duration;

use crate::{
    component::Address,
    sys,
    value::{Number, Value},
};

/// A signal, decoded when it is one of OpenComputers' standard ones.
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    KeyDown { keyboard: Address, char: u32, code: u32, player: String },
    KeyUp { keyboard: Address, char: u32, code: u32, player: String },
    Clipboard { keyboard: Address, text: String, player: String },
    Touch { screen: Address, x: Number, y: Number, button: i64, player: String },
    Drag { screen: Address, x: Number, y: Number, button: i64, player: String },
    Drop { screen: Address, x: Number, y: Number, button: i64, player: String },
    Scroll { screen: Address, x: Number, y: Number, direction: i64, player: String },
    Walk { screen: Address, x: Number, y: Number, player: Option<String> },
    ComponentAdded { address: Address, kind: String },
    ComponentRemoved { address: Address, kind: String },
    ComponentAvailable { kind: String },
    ComponentUnavailable { kind: String },
    RedstoneChanged { address: Address, side: i64, old: i64, new: i64, color: Option<i64> },
    Motion { address: Address, x: Number, y: Number, z: Number, entity: Option<String> },
    ModemMessage { receiver: Address, sender: Address, port: i64, distance: Number, data: Vec<Value> },
    InventoryChanged { slot: i64 },
    /// A Computer Pattern Provider (AE2) was handed the inputs for a job;
    /// see [`crate::pattern_provider`]. `pattern` is the pattern's index, from 1.
    PatternRequest { provider: Address, job: i64, pattern: i64 },
    /// Anything else, as sent.
    Other { name: String, args: Vec<Value> },
}

impl Signal {
    /// Decodes `[name, args...]`.
    pub fn parse(name: String, args: Vec<Value>) -> Signal {
        Self::typed(&name, &args).unwrap_or(Signal::Other { name, args })
    }

    fn typed(name: &str, a: &[Value]) -> Option<Signal> {
        let s = |i: usize| a.get(i)?.as_str().map(str::to_owned);
        let addr = |i: usize| s(i).map(Address);
        let n = |i: usize| a.get(i)?.as_number();
        let int = |i: usize| n(i).map(Number::as_i64);
        let ch = |i: usize| n(i).map(Number::as_u32);
        Some(match name {
            "key_down" => Signal::KeyDown { keyboard: addr(0)?, char: ch(1)?, code: ch(2)?, player: s(3)? },
            "key_up" => Signal::KeyUp { keyboard: addr(0)?, char: ch(1)?, code: ch(2)?, player: s(3)? },
            "clipboard" => Signal::Clipboard { keyboard: addr(0)?, text: s(1)?, player: s(2)? },
            "touch" => Signal::Touch { screen: addr(0)?, x: n(1)?, y: n(2)?, button: int(3)?, player: s(4)? },
            "drag" => Signal::Drag { screen: addr(0)?, x: n(1)?, y: n(2)?, button: int(3)?, player: s(4)? },
            "drop" => Signal::Drop { screen: addr(0)?, x: n(1)?, y: n(2)?, button: int(3)?, player: s(4)? },
            "scroll" => Signal::Scroll { screen: addr(0)?, x: n(1)?, y: n(2)?, direction: int(3)?, player: s(4)? },
            "walk" => Signal::Walk { screen: addr(0)?, x: n(1)?, y: n(2)?, player: s(3) },
            "component_added" => Signal::ComponentAdded { address: addr(0)?, kind: s(1)? },
            "component_removed" => Signal::ComponentRemoved { address: addr(0)?, kind: s(1)? },
            "component_available" => Signal::ComponentAvailable { kind: s(0)? },
            "component_unavailable" => Signal::ComponentUnavailable { kind: s(0)? },
            "redstone_changed" => Signal::RedstoneChanged {
                address: addr(0)?,
                side: int(1)?,
                old: int(2)?,
                new: int(3)?,
                color: int(4),
            },
            "motion" => Signal::Motion { address: addr(0)?, x: n(1)?, y: n(2)?, z: n(3)?, entity: s(4) },
            "modem_message" => Signal::ModemMessage {
                receiver: addr(0)?,
                sender: addr(1)?,
                port: int(2)?,
                distance: n(3)?,
                data: a.get(4..).unwrap_or_default().to_vec(),
            },
            "inventory_changed" => Signal::InventoryChanged { slot: int(0)? },
            "pattern_request" => Signal::PatternRequest { provider: addr(0)?, job: int(1)?, pattern: int(2)? },
            _ => return None,
        })
    }

    /// The typed key press, if this is one: `(char, code)`.
    pub fn key_down(&self) -> Option<(char, u32)> {
        match self {
            Signal::KeyDown { char, code, .. } => Some((char::from_u32(*char).unwrap_or('\0'), *code)),
            _ => None,
        }
    }
}

/// Waits for the next signal. `None` waits forever; a timeout that passes
/// returns `None`.
pub fn pull(timeout: Option<Duration>) -> Option<Signal> {
    let ms = timeout.map_or(-1, |t| t.as_millis().min(i64::MAX as u128) as i64);
    if unsafe { sys::pull_signal_ms(ms) } != sys::STATUS_OK {
        return None;
    }
    match Value::decode(&sys::response()) {
        Some(Value::Array(mut items)) if !items.is_empty() => {
            let name = match items.remove(0) {
                Value::String(s) => s.to_string_lossy(),
                _ => return None,
            };
            Some(Signal::parse(name, items))
        }
        _ => None,
    }
}

/// Waits for a signal that `filter` accepts, discarding others, until the
/// timeout (if any) runs out.
pub fn pull_filtered(timeout: Option<Duration>, mut filter: impl FnMut(&Signal) -> bool) -> Option<Signal> {
    let deadline = timeout.map(|t| crate::computer::uptime() + t);
    loop {
        let left = match deadline {
            Some(d) => Some(d.checked_sub(crate::computer::uptime())?),
            None => None,
        };
        let signal = pull(left)?;
        if filter(&signal) {
            return Some(signal);
        }
    }
}

/// Queues a signal on this computer. `false` if the queue is full.
pub fn push(name: &str, args: Vec<Value>) -> bool {
    let mut signal = vec![Value::from(name)];
    signal.extend(args);
    let bytes = Value::Array(signal).encode();
    unsafe { sys::push_signal(bytes.as_ptr(), bytes.len()) == sys::STATUS_OK }
}
