use std::path::PathBuf;

use wackasm_oc::{
    OcMachine, OcValue, Request, RunOutcome, WorkDir,
    abi::{STATUS_ERROR, STATUS_OK, STATUS_TIMEOUT},
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
fn boot_requires_wasm_eeprom() {
    let dir = scratch_dir("eeprom");
    let mut m = machine(&dir);
    let no_bios = m.boot(None).unwrap_err().to_string();
    assert!(no_bios.contains("no bios found"), "{no_bios}");
    // The Lua BIOS isn't wasm.
    let lua = m.boot(Some(b"local init do ... end")).unwrap_err().to_string();
    assert!(lua.contains("WebAssembly BIOS"), "{lua}");
    assert!(!m.is_booted());
    let _ = std::fs::remove_dir_all(dir);
}

/// A "BIOS" that first tries to exec garbage (which must fail and return
/// STATUS_ERROR), then execs a real payload embedded in its data segment.
fn exec_bios(payload: &[u8]) -> Vec<u8> {
    let escaped: String = payload.iter().map(|b| format!("\\{b:02x}")).collect();
    wat::parse_str(format!(
        r#"(module
          (import "oc" "exec"  (func $exec (param i32 i32) (result i32)))
          (import "oc" "crash" (func $crash (param i32 i32)))
          (memory 1)
          (data (i32.const 0) "nope")
          (data (i32.const 16) "{escaped}")
          (func (export "oc_main")
            (if (i32.ne (call $exec (i32.const 0) (i32.const 4)) (i32.const -1))
              (then (call $crash (i32.const 0) (i32.const 4))))
            (drop (call $exec (i32.const 16) (i32.const {len})))
            ;; only reached if the real exec failed
            (call $crash (i32.const 0) (i32.const 4))))"#,
        len = payload.len()
    ))
    .unwrap()
}

#[test]
fn exec_replaces_program() {
    let dir = scratch_dir("exec");
    let payload = wat::parse_str(
        r#"(module
          (import "oc" "shutdown" (func $shutdown (param i32)))
          (memory 2)
          (func (export "oc_main") (call $shutdown (i32.const 1))))"#,
    )
    .unwrap();
    let mut m = machine(&dir);
    m.set_memory_limit(3 * PAGE);
    m.boot(Some(&exec_bios(&payload))).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Shutdown { reboot: true });

    // The exec'd program is held to the same memory limit.
    let mut m = machine(&dir);
    m.set_memory_limit(PAGE);
    m.boot(Some(&exec_bios(&payload))).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("nope".into()));
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

/// A WASI child: writes "hi\n", reads stdin, echoes it, asks the game for a
/// beep (a Java request while its parent waits), then exits with 7.
const WASI_CHILD: &str = r#"
(module
  (import "wasi_snapshot_preview1" "fd_write"  (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_read"   (func $fd_read (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
  (import "oc" "beep" (func $beep (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0)   "\64\00\00\00\03\00\00\00") ;; iovec -> 100, len 3
  (data (i32.const 100) "hi\n")
  (data (i32.const 16)  "\c8\00\00\00\10\00\00\00") ;; iovec -> 200, len 16
  (func (export "_start")
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 50)))
    (drop (call $fd_read (i32.const 0) (i32.const 16) (i32.const 1) (i32.const 60)))
    (i32.store (i32.const 32) (i32.const 200))
    (i32.store (i32.const 36) (i32.load (i32.const 60)))
    (drop (call $fd_write (i32.const 1) (i32.const 32) (i32.const 1) (i32.const 50)))
    (drop (call $beep (i32.const 440) (i32.const 100)))
    (call $proc_exit (i32.const 7))))
"#;

/// A parent that runs `child` and records each event code as a digit, then
/// the child's exit code, and crashes with that log so the test can read it.
fn spawning_parent(child: &[u8], script: &str) -> Vec<u8> {
    let escaped: String = child.iter().map(|b| format!("\\{b:02x}")).collect();
    wat::parse_str(format!(
        r#"(module
          (import "oc" "spawn"         (func $spawn (param i32 i32 i32 i32) (result i32)))
          (import "oc" "resume"        (func $resume (param i32 i32) (result i32)))
          (import "oc" "response_read" (func $rread (param i32 i32) (result i32)))
          (import "oc" "crash"         (func $crash (param i32 i32)))
          (memory 1)
          (data (i32.const 0) "abc")
          (data (i32.const 16) "\07\01\00\00\00\05\05\00\00\00child") ;; ["child"]
          (data (i32.const 1024) "{escaped}")
          (global $n (mut i32) (i32.const 0))
          (func $rec (param $e i32)
            (i32.store8 (i32.add (i32.const 512) (global.get $n)) (i32.add (i32.const 48) (local.get $e)))
            (global.set $n (i32.add (global.get $n) (i32.const 1))))
          (func (export "oc_main")
            (call $rec (call $spawn (i32.const 1024) (i32.const {len}) (i32.const 16) (i32.const 15)))
            {script}
            (call $crash (i32.const 512) (global.get $n))))"#,
        len = child.len()
    ))
    .unwrap()
}

#[test]
fn spawn_runs_a_wasi_child_with_stdio() {
    let dir = scratch_dir("spawn");
    let child = wat::parse_str(WASI_CHILD).unwrap();
    let parent = spawning_parent(
        &child,
        r#"
        (call $rec (call $resume (i32.const 0) (i32.const 0)))   ;; after "hi": child wants input
        (call $rec (call $resume (i32.const 0) (i32.const 3)))   ;; feed "abc": echoed
        (call $rec (call $resume (i32.const 0) (i32.const 0)))   ;; continue: exits
        (drop (call $rread (i32.const 600) (i32.const 64)))
        (call $rec (i32.load8_u (i32.const 606)))                ;; exit code from [code, output]
        "#,
    );
    let mut m = machine(&dir);
    m.set_memory_limit(4 * PAGE);
    m.boot(Some(&parent)).unwrap();

    // The child's beep reaches Java while the parent waits. Reload right there.
    assert_eq!(
        m.run(0.0, 1_000_000).unwrap(),
        RunOutcome::Request(Request::Beep { frequency: 440, duration_ms: 100 })
    );
    assert_eq!(m.process_count(), 2);
    let mut m = reload(m, &dir);
    m.respond(STATUS_OK, Vec::new()).unwrap();

    // OUTPUT, INPUT, OUTPUT, EXITED, then exit code 7.
    assert_eq!(m.run(0.1, 1_000_000).unwrap(), RunOutcome::Error("12107".into()));
    assert_eq!(m.process_count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn crashing_child_leaves_parent_running() {
    let dir = scratch_dir("spawn-crash");
    let child = wat::parse_str(r#"(module (func (export "_start") unreachable))"#).unwrap();
    let parent = spawning_parent(&child, "");
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();
    // EVENT_CRASHED, and the parent carried on to report it.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("3".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn child_shares_the_machines_ram() {
    let dir = scratch_dir("spawn-ram");
    // Needs 2 pages; the parent already holds 1 of the 2 available.
    let child = wat::parse_str(r#"(module (memory 2) (func (export "_start")))"#).unwrap();
    let parent = spawning_parent(&child, "");
    let mut m = machine(&dir);
    m.set_memory_limit(2 * PAGE);
    m.boot(Some(&parent)).unwrap();
    // spawn returns STATUS_ERROR (-1), recorded as '0' - 1.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("/".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// A WASI child that opens /f.txt on its disk (via preopen fd 3), reads it,
/// closes it, and exits with 5 if it read "data", or 9 otherwise.
const WASI_FILE_CHILD: &str = r#"
(module
  (import "wasi_snapshot_preview1" "path_open" (func $path_open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_read"   (func $fd_read (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_close"  (func $fd_close (param i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
  (memory (export "memory") 1)
  (data (i32.const 0)  "f.txt")
  (data (i32.const 16) "\c8\00\00\00\10\00\00\00") ;; iovec -> 200, len 16
  (func (export "_start")
    ;; path_open(dirfd 3, dirflags 0, "f.txt", oflags 0, rights FD_READ, 0, fdflags 0, &fd at 100)
    (if (call $path_open (i32.const 3) (i32.const 0) (i32.const 0) (i32.const 5) (i32.const 0)
                         (i64.const 2) (i64.const 0) (i32.const 0) (i32.const 100))
      (then (call $proc_exit (i32.const 9))))
    (drop (call $fd_read (i32.load (i32.const 100)) (i32.const 16) (i32.const 1) (i32.const 60)))
    (drop (call $fd_close (i32.load (i32.const 100))))
    (call $proc_exit
      (select (i32.const 5) (i32.const 9)
        (i32.eq (i32.load (i32.const 200)) (i32.const 0x61746164)))))) ;; "data"
"#;

#[test]
fn wasi_files_map_onto_the_default_disk() {
    let dir = scratch_dir("wasi-fs");
    let child = wat::parse_str(WASI_FILE_CHILD).unwrap();
    let parent = spawning_parent(
        &child,
        r#"
        (drop (call $rread (i32.const 600) (i32.const 64)))
        (call $rec (i32.load8_u (i32.const 606)))                ;; exit code from [code, output]
        "#,
    );
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();

    let handle = OcValue::Userdata(1);
    let invoke = |method: &str, args: Vec<OcValue>| {
        RunOutcome::Request(Request::Invoke { address: "disk-a".into(), method: method.into(), args })
    };
    let ok = |values: Vec<OcValue>| OcValue::Array(values).encode();

    // Which disk? Then the component calls path_open, fd_read and fd_close
    // make: one each, as for Lua.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(Request::DefaultDisk));
    m.respond(STATUS_OK, ok(vec![OcValue::str("disk-a")])).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), invoke("open", vec![OcValue::str("/f.txt"), OcValue::str("rb")]));

    // Reload with path_open waiting on Java.
    let mut m = reload(m, &dir);
    m.respond(STATUS_OK, ok(vec![handle.clone()])).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), invoke("read", vec![handle.clone(), OcValue::Int(16)]));
    m.respond(STATUS_OK, ok(vec![OcValue::Bytes(b"data".to_vec())])).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), invoke("close", vec![handle]));
    m.respond(STATUS_OK, ok(Vec::new())).unwrap();

    // EXITED ('0'), then exit code 5.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("05".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn wasi_file_errors_become_errnos() {
    let dir = scratch_dir("wasi-fs-err");
    let child = wat::parse_str(WASI_FILE_CHILD).unwrap();
    let parent = spawning_parent(
        &child,
        r#"
        (drop (call $rread (i32.const 600) (i32.const 64)))
        (call $rec (i32.load8_u (i32.const 606)))
        "#,
    );
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();
    // No disk at all: path_open fails with an errno, and the child exits with 9.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(Request::DefaultDisk));
    m.respond(STATUS_OK, OcValue::Array(Vec::new()).encode()).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("09".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// A parent that `spawn_file`s /c.wasm from `disk` once per `(spawn)` in
/// `script`, recording each event code ('0' = exited, '+' = not found).
fn spawn_file_parent(disk: &str, spawns: usize) -> Vec<u8> {
    let call = format!(
        "(call $rec (call $spawn_file (i32.const 64) (i32.const {}) (i32.const 128) (i32.const 7) (i32.const 16) (i32.const 15)))",
        disk.len()
    );
    wat::parse_str(format!(
        r#"(module
          (import "oc" "spawn_file" (func $spawn_file (param i32 i32 i32 i32 i32 i32) (result i32)))
          (import "oc" "crash"      (func $crash (param i32 i32)))
          (memory 1)
          (data (i32.const 16) "\07\01\00\00\00\05\05\00\00\00child") ;; ["child"]
          (data (i32.const 64) "{disk}")
          (data (i32.const 128) "/c.wasm")
          (global $n (mut i32) (i32.const 0))
          (func $rec (param $e i32)
            (i32.store8 (i32.add (i32.const 512) (global.get $n)) (i32.add (i32.const 48) (local.get $e)))
            (global.set $n (i32.add (global.get $n) (i32.const 1))))
          (func (export "oc_main")
            {calls}
            (call $crash (i32.const 512) (global.get $n))))"#,
        calls = vec![call; spawns].join("\n")
    ))
    .unwrap()
}

/// Answers the component calls that loading /c.wasm makes, asserting each one.
fn serve_load(m: &mut OcMachine, disk: &str, modified: i64, image: Option<&[u8]>) {
    let ok = |values: Vec<OcValue>| OcValue::Array(values).encode();
    let expect = |m: &mut OcMachine, method: &str| match m.run(0.0, 1_000_000).unwrap() {
        RunOutcome::Request(Request::Invoke { address, method: got, .. }) => {
            assert_eq!((address.as_str(), got.as_str()), (disk, method));
        }
        other => panic!("expected {method}, got {other:?}"),
    };
    let size = image.map_or(0, |i| i.len() as i64);
    expect(m, "size");
    m.respond(STATUS_OK, ok(vec![OcValue::Int(size)])).unwrap();
    expect(m, "lastModified");
    m.respond(STATUS_OK, ok(vec![OcValue::Int(modified)])).unwrap();
}

fn serve_read(m: &mut OcMachine, disk: &str, image: &[u8]) {
    let ok = |values: Vec<OcValue>| OcValue::Array(values).encode();
    let handle = OcValue::Userdata(1);
    for (method, answer) in [
        ("open", vec![handle.clone()]),
        ("read", vec![OcValue::Bytes(image.to_vec())]),
        ("read", vec![OcValue::Nil]),
        ("close", vec![]),
    ] {
        match m.run(0.0, 1_000_000).unwrap() {
            RunOutcome::Request(Request::Invoke { address, method: got, .. }) => {
                assert_eq!((address.as_str(), got.as_str()), (disk, method));
            }
            other => panic!("expected {method}, got {other:?}"),
        }
        m.respond(STATUS_OK, ok(answer)).unwrap();
    }
}

#[test]
fn spawn_file_caches_compiled_programs() {
    let dir = scratch_dir("spawn-file");
    let disk = "disk-cache-test";
    let child = wat::parse_str(r#"(module (func (export "_start")))"#).unwrap();
    let mut m = machine(&dir);
    m.boot(Some(&spawn_file_parent(disk, 3))).unwrap();

    // First run: size and mtime, then the file is read and compiled.
    serve_load(&mut m, disk, 1000, Some(&child));
    serve_read(&mut m, disk, &child);
    // Second run, same size and mtime: straight from the cache, no reads.
    serve_load(&mut m, disk, 1000, Some(&child));
    // Third run after the file changed: read again.
    serve_load(&mut m, disk, 2000, Some(&child));
    serve_read(&mut m, disk, &child);

    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("000".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn spawn_file_reports_missing_files() {
    let dir = scratch_dir("spawn-file-missing");
    let disk = "disk-missing-test";
    let mut m = machine(&dir);
    m.boot(Some(&spawn_file_parent(disk, 1))).unwrap();
    serve_load(&mut m, disk, 0, None);
    // open fails, and with size and mtime 0 that means no such file.
    match m.run(0.0, 1_000_000).unwrap() {
        RunOutcome::Request(Request::Invoke { method, .. }) => assert_eq!(method, "open"),
        other => panic!("expected open, got {other:?}"),
    }
    m.respond(STATUS_ERROR, b"/c.wasm".to_vec()).unwrap();
    // STATUS_NOT_FOUND (-5) recorded as '0' - 5.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("+".into()));
    let _ = std::fs::remove_dir_all(dir);
}

fn escape(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

fn spec(entries: Vec<(&str, OcValue)>) -> Vec<u8> {
    OcValue::Map(entries.into_iter().map(|(k, v)| (OcValue::str(k), v)).collect()).encode()
}

/// A parent that `spawn_with`s `spec`, recording the event code and then the
/// child's exit code as characters ('0' + n), and crashes with that log.
fn spawn_with_parent(spec: &[u8]) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
          (import "oc" "spawn_with"    (func $spawn_with (param i32 i32) (result i32)))
          (import "oc" "response_read" (func $rread (param i32 i32) (result i32)))
          (import "oc" "crash"         (func $crash (param i32 i32)))
          (memory 2)
          (data (i32.const 1024) "{data}")
          (global $n (mut i32) (i32.const 0))
          (func $rec (param $e i32)
            (i32.store8 (i32.add (i32.const 512) (global.get $n)) (i32.add (i32.const 48) (local.get $e)))
            (global.set $n (i32.add (global.get $n) (i32.const 1))))
          (func (export "oc_main")
            (call $rec (call $spawn_with (i32.const 1024) (i32.const {len})))
            (drop (call $rread (i32.const 600) (i32.const 64)))
            (call $rec (i32.load8_u (i32.const 606)))
            (call $crash (i32.const 512) (global.get $n))))"#,
        data = escape(spec),
        len = spec.len()
    ))
    .unwrap()
}

/// A WASI child that exits with argc * 10 + the number of environment variables.
const ARGS_ENV_CHILD: &str = r#"
(module
  (import "wasi_snapshot_preview1" "args_sizes_get"    (func $args (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "environ_sizes_get" (func $env (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit"         (func $exit (param i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    (drop (call $args (i32.const 0) (i32.const 4)))
    (drop (call $env (i32.const 8) (i32.const 12)))
    (call $exit (i32.add (i32.mul (i32.load (i32.const 0)) (i32.const 10)) (i32.load (i32.const 8))))))
"#;

#[test]
fn spawn_with_passes_argv_and_env() {
    let dir = scratch_dir("spawn-with");
    let child = wat::parse_str(ARGS_ENV_CHILD).unwrap();
    let parent = spawn_with_parent(&spec(vec![
        ("image", OcValue::Bytes(child)),
        ("argv", OcValue::Array(vec![OcValue::str("child"), OcValue::str("x")])),
        ("env", OcValue::Array(vec![OcValue::str("A=1"), OcValue::str("B=2"), OcValue::str("C=3")])),
    ]));
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();
    // EXITED ('0'), then 2 args * 10 + 3 vars = 23 ('0' + 23 = 'G').
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("0G".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn spawn_with_children_inherit_the_environment() {
    let dir = scratch_dir("spawn-with-env");
    let grandchild = wat::parse_str(ARGS_ENV_CHILD).unwrap();
    // The middle process spawns the grandchild without an env, then exits with its code.
    let inner = spec(vec![("image", OcValue::Bytes(grandchild))]);
    let child = wat::parse_str(format!(
        r#"(module
          (import "oc" "spawn_with"    (func $spawn_with (param i32 i32) (result i32)))
          (import "oc" "response_read" (func $rread (param i32 i32) (result i32)))
          (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
          (memory (export "memory") 1)
          (data (i32.const 1024) "{data}")
          (func (export "_start")
            (drop (call $spawn_with (i32.const 1024) (i32.const {len})))
            (drop (call $rread (i32.const 600) (i32.const 64)))
            (call $exit (i32.load8_u (i32.const 606)))))"#,
        data = escape(&inner),
        len = inner.len()
    ))
    .unwrap();
    let parent = spawn_with_parent(&spec(vec![
        ("image", OcValue::Bytes(child)),
        ("env", OcValue::Map(vec![(OcValue::str("A"), OcValue::str("1")), (OcValue::str("B"), OcValue::str("2"))])),
    ]));
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();
    // The grandchild got argv ["program"] and the two inherited variables: 1 * 10 + 2 = 12 ('<').
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("0<".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn spawn_with_loads_from_and_shares_the_callers_disk() {
    let dir = scratch_dir("spawn-with-disk");
    let disk = "disk-inherit-test";
    let child = wat::parse_str(WASI_FILE_CHILD).unwrap();
    let parent = spawn_with_parent(&spec(vec![("path", OcValue::str("/c.wasm"))]));
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();

    // No `disk` given: the parent's own disk, which Java names first.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(Request::DefaultDisk));
    m.respond(STATUS_OK, OcValue::Array(vec![OcValue::str(disk)]).encode()).unwrap();
    serve_load(&mut m, disk, 1000, Some(&child));
    serve_read(&mut m, disk, &child);

    // The child opens its file on that same disk, without asking Java again.
    match m.run(0.0, 1_000_000).unwrap() {
        RunOutcome::Request(Request::Invoke { address, method, .. }) => assert_eq!((address.as_str(), method.as_str()), (disk, "open")),
        other => panic!("expected the child's open, got {other:?}"),
    }
    let ok = |values: Vec<OcValue>| OcValue::Array(values).encode();
    m.respond(STATUS_OK, ok(vec![OcValue::Userdata(7)])).unwrap();
    assert!(matches!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(Request::Invoke { method, .. }) if method == "read"));
    m.respond(STATUS_OK, ok(vec![OcValue::Bytes(b"data".to_vec())])).unwrap();
    assert!(matches!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(Request::Invoke { method, .. }) if method == "close"));
    m.respond(STATUS_OK, ok(Vec::new())).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("05".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_middle_process_can_relay_while_its_child_is_paused() {
    let dir = scratch_dir("relay");
    let grandchild = wat::parse_str(r#"
        (module
          (import "wasi_snapshot_preview1" "fd_write"  (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
          (memory (export "memory") 1)
          (data (i32.const 0)   "\64\00\00\00\03\00\00\00")
          (data (i32.const 100) "hi\n")
          (func (export "_start")
            (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 50)))
            (call $exit (i32.const 4))))"#).unwrap();
    let inner = spec(vec![("image", OcValue::Bytes(grandchild))]);
    // Gets the grandchild's output, writes its own (pausing towards the root
    // while the grandchild is still paused), then lets the grandchild exit.
    let middle = wat::parse_str(format!(
        r#"(module
          (import "oc" "spawn_with"    (func $spawn_with (param i32 i32) (result i32)))
          (import "oc" "resume"        (func $resume (param i32 i32) (result i32)))
          (import "oc" "response_read" (func $rread (param i32 i32) (result i32)))
          (import "wasi_snapshot_preview1" "fd_write"  (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
          (memory (export "memory") 1)
          (data (i32.const 0)    "\64\00\00\00\03\00\00\00")
          (data (i32.const 100)  "fwd")
          (data (i32.const 1024) "{data}")
          (func (export "_start")
            (drop (call $spawn_with (i32.const 1024) (i32.const {len})))
            ;; Two relayed writes: each hands an event to the root.
            (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 50)))
            (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 50)))
            (drop (call $resume (i32.const 0) (i32.const 0)))
            (drop (call $rread (i32.const 600) (i32.const 64)))
            (call $exit (i32.load8_u (i32.const 606)))))"#,
        data = escape(&inner),
        len = inner.len()
    ))
    .unwrap();
    let root = spawning_parent(
        &middle,
        r#"
        (call $rec (call $resume (i32.const 0) (i32.const 0)))
        (call $rec (call $resume (i32.const 0) (i32.const 0)))
        (drop (call $rread (i32.const 600) (i32.const 64)))
        (call $rec (i32.load8_u (i32.const 606)))
        "#,
    );
    let mut m = machine(&dir);
    m.boot(Some(&root)).unwrap();
    // OUTPUT, OUTPUT (the middle's relays), EXITED, then the grandchild's code 4.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("1104".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// A WASI child that exits with the size of its environment block.
const ENV_SIZE_CHILD: &str = r#"
(module
  (import "wasi_snapshot_preview1" "environ_sizes_get" (func $env (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit"         (func $exit (param i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    (drop (call $env (i32.const 0) (i32.const 4)))
    (call $exit (i32.load (i32.const 4)))))
"#;

#[test]
fn spawn_with_cwd_sets_the_childs_pwd() {
    let dir = scratch_dir("spawn-cwd");
    let child = wat::parse_str(ENV_SIZE_CHILD).unwrap();
    let parent = spawn_with_parent(&spec(vec![
        ("image", OcValue::Bytes(child)),
        ("env", OcValue::Array(vec![OcValue::str("PWD=/old"), OcValue::str("A=1")])),
        // Relative to the caller's own working directory, `/`.
        ("cwd", OcValue::str("sub/../dir")),
    ]));
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();
    // "A=1\0PWD=/dir\0" is 13 bytes ('0' + 13 = '='); the old PWD is gone.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("0=".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn spawn_with_cwd_cannot_leave_the_disk() {
    let dir = scratch_dir("spawn-cwd-escape");
    let child = wat::parse_str(ENV_SIZE_CHILD).unwrap();
    let parent = spawn_with_parent(&spec(vec![("image", OcValue::Bytes(child)), ("cwd", OcValue::str("/a/../.."))]));
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();
    // STATUS_BAD_ARGUMENT (-2) is '.'; the response then holds the reason.
    assert!(matches!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error(log) if log.starts_with('.')));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn spawn_with_resolves_relative_paths_against_the_cwd() {
    let dir = scratch_dir("spawn-relative");
    let parent = spawn_with_parent(&spec(vec![
        ("path", OcValue::str("./tools/c.wasm")),
        ("disk", OcValue::str("disk-rel")),
        ("cwd", OcValue::str("/home")),
    ]));
    let mut m = machine(&dir);
    m.boot(Some(&parent)).unwrap();
    match m.run(0.0, 1_000_000).unwrap() {
        RunOutcome::Request(Request::Invoke { args, .. }) => assert_eq!(args, vec![OcValue::str("/home/tools/c.wasm")]),
        other => panic!("expected the load's first call, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn files_disk_names_the_disk_wasi_uses() {
    let dir = scratch_dir("files-disk");
    let guest = wat::parse_str(
        r#"(module
          (import "oc" "files_disk"    (func $files_disk (result i32)))
          (import "oc" "response_read" (func $rread (param i32 i32) (result i32)))
          (import "oc" "crash"         (func $crash (param i32 i32)))
          (memory 1)
          (func (export "oc_main")
            (i32.store8 (i32.const 0) (i32.add (i32.const 48) (call $files_disk)))
            (call $crash (i32.const 0) (i32.add (i32.const 1) (call $rread (i32.const 1) (i32.const 64))))))"#,
    )
    .unwrap();
    let mut m = machine(&dir);
    m.boot(Some(&guest)).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(Request::DefaultDisk));
    m.respond(STATUS_OK, OcValue::Array(vec![OcValue::str("disk-files")]).encode()).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error("0disk-files".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn spawn_with_cwd_is_relative_to_the_callers_pwd_unless_absolute() {
    // The middle process runs in /a and starts the grandchild in `dir`; the
    // grandchild exits with the size of its environment, just "PWD=...\0".
    let run = |dir: &str| {
        let scratch = scratch_dir("spawn-cwd-nested");
        let inner = spec(vec![("image", OcValue::Bytes(wat::parse_str(ENV_SIZE_CHILD).unwrap())), ("cwd", OcValue::str(dir))]);
        let middle = wat::parse_str(format!(
            r#"(module
              (import "oc" "spawn_with"    (func $spawn_with (param i32 i32) (result i32)))
              (import "oc" "response_read" (func $rread (param i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory (export "memory") 1)
              (data (i32.const 1024) "{data}")
              (func (export "_start")
                (drop (call $spawn_with (i32.const 1024) (i32.const {len})))
                (drop (call $rread (i32.const 600) (i32.const 64)))
                (call $exit (i32.load8_u (i32.const 606)))))"#,
            data = escape(&inner),
            len = inner.len()
        ))
        .unwrap();
        let parent = spawn_with_parent(&spec(vec![
            ("image", OcValue::Bytes(middle)),
            ("env", OcValue::Array(Vec::new())),
            ("cwd", OcValue::str("/a")),
        ]));
        let mut m = machine(&scratch);
        m.boot(Some(&parent)).unwrap();
        let outcome = m.run(0.0, 1_000_000).unwrap();
        let _ = std::fs::remove_dir_all(scratch);
        outcome
    };
    // "PWD=/b\0" is 7 bytes; "PWD=/a/c\0" is 9.
    assert_eq!(run("/b"), RunOutcome::Error("07".into()));
    assert_eq!(run("c"), RunOutcome::Error("09".into()));
}

#[test]
fn methods_of_userdata_values_go_to_java() {
    let dir = scratch_dir("invoke-value");
    let args = OcValue::Array(vec![OcValue::Int(16)]).encode();
    let guest = wat::parse_str(format!(
        r#"(module
          (import "oc" "invoke_value"  (func $invoke_value (param i32 i32 i32 i32 i32) (result i32)))
          (import "oc" "release_value" (func $release (param i32) (result i32)))
          (import "oc" "response_read" (func $rread (param i32 i32) (result i32)))
          (import "oc" "crash"         (func $crash (param i32 i32)))
          (memory 1)
          (data (i32.const 0) "read")
          (data (i32.const 16) "{args}")
          (func (export "oc_main")
            (drop (call $invoke_value (i32.const 7) (i32.const 0) (i32.const 4) (i32.const 16) (i32.const {len})))
            (drop (call $release (i32.const 7)))
            (call $crash (i32.const 100) (call $rread (i32.const 100) (i32.const 64)))))"#,
        args = escape(&args),
        len = args.len()
    ))
    .unwrap();
    let mut m = machine(&dir);
    m.boot(Some(&guest)).unwrap();
    let request = Request::InvokeValue { value: 7, method: "read".into(), args: vec![OcValue::Int(16)] };
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(request.clone()));
    // The value crosses to Java as userdata, so the codec hands Java the object.
    assert_eq!(request.payload(), OcValue::Array(vec![OcValue::Userdata(7), OcValue::str("read"), OcValue::Array(vec![OcValue::Int(16)])]));
    m.respond(STATUS_OK, OcValue::Array(vec![OcValue::Bytes(b"abc".to_vec())]).encode()).unwrap();
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Request(Request::ReleaseValue { value: 7 }));
    m.respond(STATUS_OK, Vec::new()).unwrap();
    // The release left an empty response; the read's reply was replaced.
    assert_eq!(m.run(0.0, 1_000_000).unwrap(), RunOutcome::Error(String::new()));
    let _ = std::fs::remove_dir_all(dir);
}
