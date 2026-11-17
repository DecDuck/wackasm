use std::path::PathBuf;

use wackasm_oc::{
    OcMachine, OcValue, Request, RunOutcome, WorkDir,
    abi::{STATUS_OK, STATUS_TIMEOUT},
};

const KEY: &str = "test-machine";
const ADDRESS: &str = "00000000-0000-0000-0000-000000000001";

/// Exercises a component lookup, a component call, a bad pointer, a long
/// loop that spans several fuel slices, a pull_signal timeout, and shutdown.
const GUEST: &str = r#"
(module
  (import "oc" "components"   (func $components (param i32 i32) (result i32)))
  (import "oc" "invoke"       (func $invoke (param i32 i32 i32 i32 i32 i32) (result i32)))
  (import "oc" "response_len" (func $response_len (result i32)))
  (import "oc" "pull_signal"  (func $pull_signal (param f64) (result i32)))
  (import "oc" "shutdown"     (func $shutdown (param i32)))
  (import "oc" "crash"        (func $crash (param i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0)   "gpu")
  (data (i32.const 16)  "gpu-addr")
  (data (i32.const 32)  "set")
  ;; [1, 1, "hi"] in the oc value encoding
  (data (i32.const 64)  "\07\03\00\00\00\03\01\00\00\00\00\00\00\00\03\01\00\00\00\00\00\00\00\05\02\00\00\00hi")
  (data (i32.const 128) "bad len")

  (func $fail (call $crash (i32.const 128) (i32.const 7)))

  (func (export "oc_main") (local $i i32)
    (if (i32.ne (call $components (i32.const 0) (i32.const 3)) (i32.const 0))
      (then (call $fail)))
    (if (i32.ne (call $invoke (i32.const 16) (i32.const 8) (i32.const 32) (i32.const 3)
                              (i32.const 64) (i32.const 30)) (i32.const 0))
      (then (call $fail)))
    ;; encode([true]) is 6 bytes
    (if (i32.ne (call $response_len) (i32.const 6))
      (then (call $fail)))
    ;; an out-of-bounds pointer comes back as STATUS_BAD_ARGUMENT without a trip to Java
    (if (i32.ne (call $components (i32.const 0x7fffffff) (i32.const 10)) (i32.const -2))
      (then (call $fail)))
    (block $done
      (loop $spin
        (br_if $done (i32.ge_u (local.get $i) (i32.const 100000)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $spin)))
    (if (i32.ne (call $pull_signal (f64.const 0.5)) (i32.const 1))
      (then (call $fail)))
    (call $shutdown (i32.const 1)))
)
"#;

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wackasm-oc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn machine(dir: &PathBuf) -> OcMachine {
    OcMachine::new(WorkDir::open(dir).unwrap(), KEY, ADDRESS).unwrap()
}

/// Simulates a server reload: snapshot, drop everything, restore.
fn reload(machine: OcMachine, dir: &PathBuf) -> OcMachine {
    machine.save().unwrap();
    drop(machine);
    let mut restored = self::machine(dir);
    assert!(restored.load().unwrap(), "snapshot should exist");
    restored
}

#[test]
fn full_lifecycle_with_suspension() {
    let dir = scratch_dir("lifecycle");
    let wasm = wat::parse_str(GUEST).unwrap();
    let mut m = machine(&dir);
    m.boot(Some(&wasm)).unwrap();

    // 1. Suspends on the component lookup.
    let outcome = m.run(0.0, 1_000_000).unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Request(Request::Components { filter: "gpu".into() })
    );

    // Reload while the import is pending. The request survives so that Java
    // could answer it straight from `runSynchronized`.
    let mut m = reload(m, &dir);
    assert_eq!(
        m.pending_request(),
        Some(&Request::Components { filter: "gpu".into() })
    );
    let components = OcValue::Map(vec![(OcValue::str("gpu-addr"), OcValue::str("gpu"))]);
    m.respond(STATUS_OK, components.encode()).unwrap();

    // 2. The component call, with arguments decoded from guest memory.
    let outcome = m.run(0.05, 1_000_000).unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Request(Request::Invoke {
            address: "gpu-addr".into(),
            method: "set".into(),
            args: vec![OcValue::Int(1), OcValue::Int(1), OcValue::str("hi")],
        })
    );
    // Reload again mid-invoke, as if the server stopped between runThreaded
    // returning SynchronizedCall and runSynchronized.
    let mut m = reload(m, &dir);
    m.respond(STATUS_OK, OcValue::Array(vec![OcValue::Bool(true)]).encode())
        .unwrap();

    // 3. The spin loop needs several slices at low fuel.
    let mut slices = 0;
    let outcome = loop {
        match m.run(0.1, 20_000).unwrap() {
            RunOutcome::Yield => slices += 1,
            other => break other,
        }
        if slices % 5 == 0 {
            m = reload(m, &dir);
        }
        assert!(slices < 1000, "guest never left the loop");
    };
    assert!(slices > 1, "expected the loop to span several slices, got {slices}");

    // 4. pull_signal: the deadline is fixed when the guest first asks, and it survives reloads.
    assert_eq!(
        outcome,
        RunOutcome::Request(Request::PullSignal { remaining: Some(0.5) })
    );
    let mut m = reload(m, &dir);
    assert_eq!(
        m.run(0.35, 1_000_000).unwrap(),
        RunOutcome::Request(Request::PullSignal { remaining: Some(0.25) })
    );
    assert_eq!(
        m.run(0.9, 1_000_000).unwrap(),
        RunOutcome::Request(Request::PullSignal { remaining: Some(0.0) })
    );
    m.respond(STATUS_TIMEOUT, Vec::new()).unwrap();

    // 5. Shutdown, which sticks.
    assert_eq!(m.run(1.0, 1_000_000).unwrap(), RunOutcome::Shutdown { reboot: true });
    assert_eq!(m.run(1.1, 1_000_000).unwrap(), RunOutcome::Shutdown { reboot: true });
    assert_eq!(
        m.current_payload(),
        Some(OcValue::Array(vec![OcValue::Bool(true)]))
    );

    m.discard().unwrap();
    assert!(!m.load().unwrap());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn wrong_response_crashes_guest() {
    let dir = scratch_dir("crash");
    let wasm = wat::parse_str(GUEST).unwrap();
    let mut m = machine(&dir);
    m.boot(Some(&wasm)).unwrap();
    assert!(matches!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(_)));
    m.respond(STATUS_OK, Vec::new()).unwrap();
    assert!(matches!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(Request::Invoke { .. })));

    // A response of the wrong length makes the guest call oc.crash.
    m.respond(STATUS_OK, OcValue::Nil.encode()).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("bad len".into()));
    assert!(m.respond(STATUS_OK, Vec::new()).is_err(), "nothing is pending after a crash");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn boot_falls_back_to_work_dir_image() {
    let dir = scratch_dir("fallback");
    let mut m = machine(&dir);
    // Lua BIOS in the EEPROM and no boot.wasm yet.
    assert!(m.boot(Some(b"local c = component")).is_err());

    std::fs::write(dir.join("boot.wasm"), wat::parse_str(GUEST).unwrap()).unwrap();
    m.boot(Some(b"local c = component")).unwrap();
    assert!(matches!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(_)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn memory_limit_is_enforced() {
    let dir = scratch_dir("memory");
    let mut m = machine(&dir);
    m.set_memory_limit(1024); // smaller than one 64 KiB page
    assert!(m.boot(Some(&wat::parse_str(GUEST).unwrap())).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn rejects_path_like_keys() {
    let dir = scratch_dir("keys");
    for key in ["", "../evil", "a/b", "a\\b", "c:"] {
        assert!(OcMachine::new(WorkDir::open(&dir).unwrap(), key, ADDRESS).is_err(), "{key:?}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

const PAGE: u64 = 64 * 1024;

/// Starts at 1 page. `memory.grow 1` should succeed and `memory.grow 2` should
/// return -1 under a 3-page limit. It crashes with "A" or "B" if either check
/// fails, and shuts down if both pass.
const GROW_GUEST: &str = r#"
(module
  (import "oc" "crash"    (func $crash (param i32 i32)))
  (import "oc" "shutdown" (func $shutdown (param i32)))
  (memory 1)
  (data (i32.const 0) "AB")
  (func (export "oc_main")
    (if (i32.ne (memory.grow (i32.const 1)) (i32.const 1))
      (then (call $crash (i32.const 0) (i32.const 1))))
    (if (i32.ne (memory.grow (i32.const 2)) (i32.const -1))
      (then (call $crash (i32.const 1) (i32.const 1))))
    (call $shutdown (i32.const 0)))
)
"#;

#[test]
fn memory_grow_fails_past_limit() {
    let dir = scratch_dir("grow");
    let wasm = wat::parse_str(GROW_GUEST).unwrap();
    let mut m = machine(&dir);
    m.set_memory_limit(3 * PAGE);
    m.boot(Some(&wasm)).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Shutdown { reboot: false });
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn memory_limit_can_change_at_runtime() {
    let dir = scratch_dir("grow-runtime");
    let wasm = wat::parse_str(GROW_GUEST).unwrap();
    let mut m = machine(&dir);
    m.set_memory_limit(3 * PAGE);
    m.boot(Some(&wasm)).unwrap();
    // More RAM installed after boot: the second grow now succeeds, so the
    // guest's -1 check fails.
    m.set_memory_limit(4 * PAGE);
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("B".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn memory_limit_survives_snapshot() {
    let dir = scratch_dir("grow-snapshot");
    let wasm = wat::parse_str(GROW_GUEST).unwrap();
    let mut m = machine(&dir);
    m.set_memory_limit(3 * PAGE);
    m.boot(Some(&wasm)).unwrap();
    m.save().unwrap();
    drop(m);

    // The restored machine has no limit of its own, so it takes the snapshot's.
    let mut m = machine(&dir);
    assert!(m.load().unwrap());
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Shutdown { reboot: false });
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unlimited_by_default() {
    let dir = scratch_dir("grow-unlimited");
    let mut m = machine(&dir);
    m.boot(Some(&wat::parse_str(GROW_GUEST).unwrap())).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("B".into()));
    let _ = std::fs::remove_dir_all(dir);
}
