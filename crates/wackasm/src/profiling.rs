//! Per-opcode cycle accounting for the interpreter loop, enabled with the
//! `op-profile` feature.
//!
//! [`begin`] is called at the top of every loop iteration. It charges the
//! cycles elapsed since the previous call to the previous opcode, so every
//! exit path out of a match arm (`continue`, `return`, fallthrough) is covered
//! without instrumenting the arms themselves. Time spent outside the loop
//! (e.g. frame setup after a `call`) is charged to the last opcode executed.
//!
//! Numbers are best read relatively: each measurement adds a few dozen cycles
//! of overhead and out-of-order execution blurs boundaries between cheap ops.

#[cfg(not(target_arch = "x86_64"))]
compile_error!("the `op-profile` feature requires x86_64 (uses rdtsc)");

use std::{arch::x86_64::_rdtsc, cell::Cell};

use wasmparser::Operator;

macro_rules! define_op_kinds {
    ($( @$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
        #[allow(non_camel_case_types, dead_code)]
        enum OpKind { $($op,)* }

        const NAMES: &[&str] = &[$(stringify!($op),)*];

        #[inline(always)]
        fn op_index(op: &Operator) -> usize {
            match op {
                $(Operator::$op { .. } => OpKind::$op as usize,)*
                _ => unreachable!("operator unknown to for_each_operator"),
            }
        }
    };
}
wasmparser::for_each_operator!(define_op_kinds);

const OP_COUNT: usize = NAMES.len();
const NONE: usize = usize::MAX;

struct Profile {
    cycles: [Cell<u64>; OP_COUNT],
    counts: [Cell<u64>; OP_COUNT],
    current: Cell<usize>,
    start: Cell<u64>,
}

thread_local! {
    static PROFILE: Profile = const {
        Profile {
            cycles: [const { Cell::new(0) }; OP_COUNT],
            counts: [const { Cell::new(0) }; OP_COUNT],
            current: Cell::new(NONE),
            start: Cell::new(0),
        }
    };
}

/// Closes the measurement for the previous opcode and starts one for `op`.
#[inline(always)]
pub(crate) fn begin(op: &Operator) {
    let now = unsafe { _rdtsc() };
    PROFILE.with(|p| {
        let previous = p.current.get();
        if previous != NONE {
            let cycles = &p.cycles[previous];
            cycles.set(cycles.get() + now.wrapping_sub(p.start.get()));
            let count = &p.counts[previous];
            count.set(count.get() + 1);
        }
        p.current.set(op_index(op));
        // Read again so this bookkeeping isn't attributed to `op`.
        p.start.set(unsafe { _rdtsc() });
    });
}

/// Clears all counters and drops any in-flight measurement. Call this before
/// the region of interest, and after any pause in execution (e.g.
/// serialization) so the gap isn't charged to the last opcode.
pub fn reset() {
    PROFILE.with(|p| {
        p.cycles.iter().for_each(|c| c.set(0));
        p.counts.iter().for_each(|c| c.set(0));
        p.current.set(NONE);
    });
}

/// Prints a per-opcode breakdown for the current thread, sorted by total
/// cycles. The final in-flight opcode is not included.
pub fn report() {
    PROFILE.with(|p| {
        let mut rows: Vec<usize> = (0..OP_COUNT).filter(|&i| p.counts[i].get() > 0).collect();
        rows.sort_by_key(|&i| std::cmp::Reverse(p.cycles[i].get()));

        let total_cycles: u64 = rows.iter().map(|&i| p.cycles[i].get()).sum();
        let total_ops: u64 = rows.iter().map(|&i| p.counts[i].get()).sum();
        if total_ops == 0 {
            println!("op-profile: no operators recorded");
            return;
        }

        println!(
            "{:<24} {:>7} {:>16} {:>7} {:>10}",
            "operator", "cycles%", "count", "count%", "cyc/op"
        );
        for i in rows {
            let cycles = p.cycles[i].get();
            let count = p.counts[i].get();
            println!(
                "{:<24} {:>6.2}% {:>16} {:>6.2}% {:>10.1}",
                NAMES[i],
                100.0 * cycles as f64 / total_cycles as f64,
                count,
                100.0 * count as f64 / total_ops as f64,
                cycles as f64 / count as f64,
            );
        }
        println!(
            "{:<24} {:>7} {:>16} {:>7} {:>10.1}",
            "total",
            "",
            total_ops,
            "",
            total_cycles as f64 / total_ops as f64
        );
    });
}
