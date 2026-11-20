//! The Computer Pattern Provider: autocrafting patterns for Applied
//! Energistics 2 that a program fulfils.
//!
//! The block exists when AE2 is installed alongside the wackasm mod. It's an
//! ME network device and an OpenComputers component (`pattern_provider`)
//! at once:
//!
//! 1. The program declares processing patterns ([`PatternProvider::set_patterns`]).
//!    AE2 sees them as ordinary processing patterns: they show up as
//!    craftable, and the crafting planner uses them.
//! 2. When a crafting job needs one, AE2 hands the block the inputs. The block
//!    holds them as a [`Job`] and sends [`Signal::PatternRequest`](crate::Signal::PatternRequest).
//! 3. The program moves the inputs out ([`PatternProvider::push_inputs`]),
//!    does the work, and gets the outputs **imported into the ME network**:
//!    through an ME Interface, an import bus, or by inserting them into this
//!    block (with a transposer, say), which forwards them. AE2's crafting
//!    CPUs claim them as they arrive, the same as for any processing pattern.
//!
//! Nothing creates items: the outputs have to come from somewhere real.
//!
//! ```no_run
//! use wackasm_opencomputers::prelude::*;
//! use wackasm_opencomputers::pattern_provider::{Pattern, PatternProvider, Stack};
//!
//! let provider = PatternProvider::first().expect("no pattern provider");
//! provider.set_patterns(&[Pattern::new(
//!     vec![Stack::item("minecraft:cobblestone", 1)],
//!     vec![Stack::item("minecraft:stone", 1)],
//! )])?;
//! loop {
//!     // Jobs from before this program started are waiting too.
//!     for job in provider.jobs()? {
//!         provider.push_inputs(job.id, Side::Top)?; // into a furnace, say
//!     }
//!     signal::pull_filtered(None, |s| matches!(s, Signal::PatternRequest { .. }));
//! }
//! # Ok::<(), wackasm_opencomputers::Error>(())
//! ```

use crate::{
    component::{Address, Component, single},
    error::{Error, Result},
    types::Side,
    value::Value,
};

/// The `pattern_provider` component.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PatternProvider(pub Address);

impl Component for PatternProvider {
    const KIND: &'static str = "pattern_provider";
    fn from_address(address: Address) -> Self {
        Self(address)
    }
    fn address(&self) -> &Address {
        &self.0
    }
}

/// An item or fluid and an amount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stack {
    /// The registry name, such as `minecraft:iron_ingot`.
    pub name: String,
    /// Items, or millibuckets for fluids.
    pub count: i64,
    pub fluid: bool,
    /// AE2's description of the exact stack (SNBT), for items with data
    /// components such as enchantments. When set, it's used and `name` and
    /// `fluid` are ignored. Stacks the component returns have it whenever
    /// it's needed, so they can be passed back as they are.
    pub key: Option<String>,
    /// The display name. Only filled in on stacks the component returns.
    pub label: Option<String>,
}

impl Stack {
    pub fn item(name: impl Into<String>, count: i64) -> Stack {
        Stack { name: name.into(), count, fluid: false, key: None, label: None }
    }

    /// `millibuckets` of a fluid.
    pub fn fluid(name: impl Into<String>, millibuckets: i64) -> Stack {
        Stack { name: name.into(), count: millibuckets, fluid: true, key: None, label: None }
    }

    fn to_value(&self) -> Value {
        let mut entries = vec![
            ("name".into(), self.name.as_str().into()),
            ("count".into(), self.count.into()),
            ("fluid".into(), self.fluid.into()),
        ];
        if let Some(key) = &self.key {
            entries.push(("key".into(), key.as_str().into()));
        }
        Value::Map(entries)
    }

    fn from_value(value: &Value) -> Option<Stack> {
        Some(Stack {
            name: value.get("name")?.as_str()?.to_owned(),
            count: value.get("count")?.as_i64()?,
            fluid: value.get("fluid").and_then(Value::as_bool).unwrap_or(false),
            key: value.get("key").and_then(Value::as_str).map(str::to_owned),
            label: value.get("label").and_then(Value::as_str).map(str::to_owned),
        })
    }
}

/// A processing pattern: AE2 hands over `inputs` and expects `outputs` back.
/// It needs at least one input (AE2 requires it) and at least one output; the
/// first output is the one AE2 lists it under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub inputs: Vec<Stack>,
    pub outputs: Vec<Stack>,
}

impl Pattern {
    pub fn new(inputs: Vec<Stack>, outputs: Vec<Stack>) -> Pattern {
        Pattern { inputs, outputs }
    }

    fn to_value(&self) -> Value {
        Value::Map(vec![
            ("inputs".into(), stacks_value(&self.inputs)),
            ("outputs".into(), stacks_value(&self.outputs)),
        ])
    }

    fn from_value(value: &Value) -> Option<Pattern> {
        Some(Pattern { inputs: stacks(value.get("inputs")?)?, outputs: stacks(value.get("outputs")?)? })
    }
}

/// One use of a pattern that AE2 handed over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: i64,
    /// The pattern's index (from 1) when the job arrived.
    pub pattern: i64,
    /// The inputs still held by the block.
    pub inputs: Vec<Stack>,
    /// What the crafting job expects to be imported into the network.
    pub outputs: Vec<Stack>,
}

impl Job {
    fn from_value(value: &Value) -> Option<Job> {
        Some(Job {
            id: value.get("id")?.as_i64()?,
            pattern: value.get("pattern")?.as_i64()?,
            inputs: stacks(value.get("inputs")?)?,
            outputs: stacks(value.get("outputs")?)?,
        })
    }
}

fn stacks_value(stacks: &[Stack]) -> Value {
    Value::Array(stacks.iter().map(Stack::to_value).collect())
}

fn stacks(value: &Value) -> Option<Vec<Stack>> {
    // An empty Lua table has no keys to tell a sequence from a map.
    if matches!(value, Value::Map(m) if m.is_empty()) {
        return Some(Vec::new());
    }
    value.as_array()?.into_iter().map(Stack::from_value).collect()
}

fn list<T>(reply: Vec<Value>, what: &str, parse: impl Fn(&Value) -> Option<T>) -> Result<Vec<T>> {
    let value: Value = single(reply)?;
    if matches!(&value, Value::Map(m) if m.is_empty()) {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .and_then(|items| items.into_iter().map(parse).collect())
        .ok_or_else(|| Error::UnexpectedReply(format!("expected a list of {what}, got {value:?}")))
}

fn job(reply: Vec<Value>) -> Result<Option<Job>> {
    match single::<Value>(reply)? {
        Value::Nil => Ok(None),
        value => Job::from_value(&value)
            .map(Some)
            .ok_or_else(|| Error::UnexpectedReply(format!("expected a job, got {value:?}"))),
    }
}

macro_rules! methods {
    ($(
        $(#[$doc:meta])*
        fn $name:ident / $name_async:ident ($($arg:ident: $ty:ty),*) -> $ret:ty =
            $lua:literal [$($value:expr),*] => $decode:expr;
    )*) => {
        impl PatternProvider {$(
            $(#[$doc])*
            pub fn $name(&self, $($arg: $ty),*) -> Result<$ret> {
                let reply = self.invoke($lua, vec![$($value),*])?;
                ($decode)(reply)
            }

            #[doc = concat!("[`Self::", stringify!($name), "`] without blocking; see [`crate::task`].")]
            #[cfg(feature = "async")]
            pub async fn $name_async(&self, $($arg: $ty),*) -> Result<$ret> {
                let reply = self.invoke_async($lua, vec![$($value),*]).await?;
                ($decode)(reply)
            }
        )*}
    };
}

methods! {
    /// Replaces every pattern. Returns how many there are.
    fn set_patterns / set_patterns_async(patterns: &[Pattern]) -> usize =
        "setPatterns" [Value::Array(patterns.iter().map(Pattern::to_value).collect())]
        => |r| single::<crate::Number>(r).map(|n| n.as_i64() as usize);

    /// Adds a pattern. Returns its index (from 1).
    fn add_pattern / add_pattern_async(pattern: &Pattern) -> i64 =
        "addPattern" [stacks_value(&pattern.inputs), stacks_value(&pattern.outputs)]
        => |r| single::<crate::Number>(r).map(|n| n.as_i64());

    /// Removes the pattern at `index` (from 1); later ones move down. Jobs
    /// already handed over are kept. `false` if there was none.
    fn remove_pattern / remove_pattern_async(index: i64) -> bool =
        "removePattern" [index.into()] => single::<bool>;

    /// Every pattern, in order.
    fn patterns / patterns_async() -> Vec<Pattern> =
        "getPatterns" [] => |r| list(r, "patterns", Pattern::from_value);

    /// The jobs waiting, oldest first.
    fn jobs / jobs_async() -> Vec<Job> =
        "getJobs" [] => |r| list(r, "jobs", Job::from_value);

    /// One job; `None` once it's done or cancelled.
    fn job / job_async(id: i64) -> Option<Job> =
        "getJob" [id.into()] => job;

    /// Moves a job's inputs into the inventory or tank on `side` of the
    /// block. `true` once everything is out, which ends the job; `false` if
    /// some didn't fit (it stays held, for another try).
    fn push_inputs / push_inputs_async(id: i64, side: Side) -> bool =
        "pushInputs" [id.into(), side.into()] => single::<bool>;

    /// Gives up on a job, returning its inputs to the ME network. `true` if
    /// they all went back, ending the job. AE2's crafting job keeps waiting
    /// for its outputs until it's cancelled in a terminal.
    fn cancel_job / cancel_job_async(id: i64) -> bool =
        "cancelJob" [id.into()] => single::<bool>;

    /// Whether the block is on a powered ME network with a channel.
    fn is_online / is_online_async() -> bool =
        "isOnline" [] => single::<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacks_round_trip() {
        let stack = Stack { key: Some("{id:\"x\"}".into()), ..Stack::fluid("minecraft:water", 1000) };
        assert_eq!(Stack::from_value(&stack.to_value()), Some(stack));
    }

    #[test]
    fn jobs_decode_from_lua_style_tables() {
        let stack = |n: &str| Value::Map(vec![("name".into(), n.into()), ("count".into(), 2.into())]);
        let job = Value::Map(vec![
            ("id".into(), 7.into()),
            ("pattern".into(), 1.into()),
            ("inputs".into(), Value::Map(vec![(1.into(), stack("minecraft:cobblestone"))])),
            ("outputs".into(), Value::Array(vec![stack("minecraft:stone")])),
        ]);
        let job = Job::from_value(&job).unwrap();
        assert_eq!((job.id, job.inputs[0].name.as_str(), job.outputs[0].count), (7, "minecraft:cobblestone", 2));
        assert_eq!(list(vec![Value::Map(vec![])], "jobs", Job::from_value), Ok(vec![]));
    }
}
