use crate::{HaltError, HostYield, Value, WackasmExecutor, WackasmModule};

fn executor(wat: &str) -> WackasmExecutor {
    let wasm = wat::parse_str(wat).expect("valid wat");
    WackasmExecutor::new(WackasmModule::new(&wasm).expect("compile module"))
        .expect("instantiate module")
}

fn run(wat: &str, args: Vec<Value>) -> Vec<Value> {
    let mut executor = executor(wat);
    executor.call_export("run", args).unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    executor.get_result().unwrap().clone()
}

fn restore(executor: WackasmExecutor) -> WackasmExecutor {
    let bytes = bitcode::serialize(&executor).unwrap();
    drop(executor);
    bitcode::deserialize(&bytes).unwrap()
}

#[test]
fn local_snapshots_survive_set_tee_and_retargeting() {
    let result = run(
        r#"(module
          (func (export "run") (param i32) (result i32 i32 i32)
            local.get 0
            i32.const 7 local.set 0
            local.get 0
            i32.const 2 i32.add local.tee 0
            local.get 0))"#,
        vec![Value::from_i32(3)],
    );
    assert_eq!(result, vec![Value(3), Value(9), Value(9)]);

    let result = run(
        r#"(module
          (func (export "run") (param i32 i32) (result i32)
            local.get 0 local.get 1 i32.add local.set 0
            local.get 0))"#,
        vec![Value(11), Value(17)],
    );
    assert_eq!(result, vec![Value(28)]);
}

#[test]
fn providers_preserve_float_and_integer_bits() {
    assert_eq!(
        run(
            r#"(module (func (export "run") (result i32 i64 f32 f64)
              i32.const -1 i64.const -9223372036854775808
              f32.const -1.5 f64.const -0))"#,
            vec![],
        ),
        vec![
            Value::from_i32(-1),
            Value::from_i64(i64::MIN),
            Value::from_f32(-1.5),
            Value::from_f64(-0.0),
        ],
    );
}

#[test]
fn conditional_branches_preserve_fallthrough_and_move_results() {
    let wat = r#"(module
      (func (export "run") (param i32) (result i32)
        (block (result i32)
          i32.const 10
          i32.const 99
          local.get 0
          br_if 0
          drop
          i32.const 1 i32.add)))"#;
    assert_eq!(run(wat, vec![Value(0)]), vec![Value(11)]);
    assert_eq!(run(wat, vec![Value(1)]), vec![Value(99)]);
}

#[test]
fn branch_table_uses_target_specific_trampolines_and_default() {
    let wat = r#"(module
      (func (export "run") (param i32) (result i32)
        (block $outer (result i32)
          i32.const 10
          (block $inner (result i32)
            i32.const 20
            i32.const 30
            local.get 0 br_table $inner $outer
          )
          i32.add)))"#;
    assert_eq!(run(wat, vec![Value(0)]), vec![Value(40)]);
    assert_eq!(run(wat, vec![Value(1)]), vec![Value(30)]);
    assert_eq!(run(wat, vec![Value::from_i32(-1)]), vec![Value(30)]);
}

#[test]
fn wide_locals_branches_and_select_use_two_slots() {
    let wat = r#"(module (memory 1)
      (func (export "run") (param v128 i32) (result v128)
        (local v128)
        local.get 0
        i32.const 0 v128.load local.tee 2
        drop
        local.get 2
        local.get 1 select
        (block (param v128) (result v128)
          br 0)))"#;
    assert_eq!(
        run(
            wat,
            vec![
                Value(0x0123456789abcdef),
                Value(0xfedcba9876543210),
                Value(1)
            ]
        ),
        vec![Value(0x0123456789abcdef), Value(0xfedcba9876543210)],
    );
    assert_eq!(
        run(wat, vec![Value(1), Value(2), Value(0)]),
        vec![Value(0), Value(0)],
    );
}

#[test]
fn wide_local_snapshot_survives_overwrite() {
    assert_eq!(
        run(
            r#"(module (memory 1)
              (func (export "run") (param v128) (result v128)
                local.get 0
                i32.const 0 v128.load local.set 0))"#,
            vec![Value(13), Value(29)],
        ),
        vec![Value(13), Value(29)],
    );
}

#[test]
fn fuel_is_charged_at_calls_and_loop_headers_not_blocks() {
    let mut executor = executor(
        r#"(module
          (func $leaf (result i32) (block) i32.const 42)
          (func (export "run") (result i32)
            (block) call $leaf))"#,
    );
    executor.call_export("run", vec![]).unwrap();
    executor.set_fuel(1);
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(executor.get_fuel(), 0);
    assert_eq!(executor.get_result().unwrap(), &vec![Value(42)]);
}

#[test]
fn nested_call_suspension_resumes_full_callstack() {
    let mut executor = executor(
        r#"(module
          (func $count (param i32) (result i32)
            (loop $again
              local.get 0 i32.const 1 i32.sub local.tee 0
              br_if $again)
            local.get 0)
          (func $middle (param i32) (result i32)
            local.get 0 call $count i32.const 10 i32.add)
          (func (export "run") (param i32) (result i32)
            local.get 0 call $middle i32.const 20 i32.add))"#,
    );
    executor.call_export("run", vec![Value(5)]).unwrap();
    for _ in 0..20 {
        executor.set_fuel(1);
        match executor.execute().unwrap() {
            HostYield::Return => {
                assert_eq!(executor.get_result().unwrap(), &vec![Value(30)]);
                return;
            }
            HostYield::OutOfFuel => executor = restore(executor),
            _ => panic!("unexpected import"),
        }
    }
    panic!("did not finish with bounded fuel");
}

#[test]
fn import_suspension_preserves_registers_and_result_destination() {
    let mut executor = executor(
        r#"(module
          (import "host" "pair" (func $pair (param i32 i64) (result i64 i32)))
          (func $middle (param i32) (result i64 i32)
            local.get 0 i64.const 17 call $pair)
          (func (export "run") (result i32 i64 i32)
            i32.const 13 i32.const 9 call $middle))"#,
    );
    executor.call_export("run", vec![]).unwrap();
    assert!(matches!(
        executor.execute().unwrap(),
        HostYield::ImportCall { args, should_return: 2, .. }
        if args == vec![Value(9), Value(17)]
    ));
    executor = restore(executor);
    assert!(executor.import_return(&[Value(1)]).is_err());
    assert!(matches!(
        executor.execute().unwrap(),
        HostYield::ImportCall { .. }
    ));
    executor.import_return(&[Value(123), Value(45)]).unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(
        executor.get_result().unwrap(),
        &vec![Value(13), Value(123), Value(45)]
    );
}

#[test]
fn indirect_calls_run_and_imports_resume_in_dispatch() {
    let mut executor = executor(
        r#"(module
          (type $t (func (param i32) (result i32)))
          (import "host" "add" (func $add (type $t)))
          (table 2 funcref)
          (elem (i32.const 0) $double $add)
          (func $double (type $t) local.get 0 i32.const 2 i32.mul)
          (func (export "run") (param i32) (result i32)
            i32.const 7 local.get 0 call_indirect (type $t)))"#,
    );
    executor.call_export("run", vec![Value(0)]).unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(executor.get_result().unwrap(), &vec![Value(14)]);
    executor.call_export("run", vec![Value(1)]).unwrap();
    assert!(matches!(
        executor.execute().unwrap(),
        HostYield::ImportCall { .. }
    ));
    executor = restore(executor);
    executor.import_return(&[Value(19)]).unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(executor.get_result().unwrap(), &vec![Value(19)]);
}

#[test]
fn recursive_calls_trap_at_buffer_limit_without_overwriting_caller() {
    let mut executor = executor(
        r#"(module
          (func $recurse (export "run") (param i32) (result i32)
            local.get 0 call $recurse))"#,
    );
    executor.call_export("run", vec![Value(1)]).unwrap();
    assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
}

#[test]
fn calls_with_more_results_than_params_preserve_live_caller_values() {
    assert_eq!(
        run(
            r#"(module
              (func $pick (param i64 i64) (result i64 i64 i64)
                local.get 0 local.get 1 local.get 0)
              (func (export "run") (result i64 i64 i64 i64)
                i64.const 7 i64.const 2 i64.const 3
                call $pick i64.mul call $pick))"#,
            vec![],
        ),
        vec![Value(7), Value(2), Value(6), Value(2)],
    );
}

#[test]
fn exhausted_indirect_call_yields_before_resolving_the_table() {
    let mut executor = executor(
        r#"(module
          (type $t (func))
          (table 1 funcref)
          (func $leaf)
          (func (export "run")
            call $leaf
            i32.const 0 call_indirect (type $t)))"#,
    );
    executor.call_export("run", vec![]).unwrap();
    executor.set_fuel(1);
    assert!(matches!(executor.execute().unwrap(), HostYield::OutOfFuel));
    executor = restore(executor);
    executor.set_fuel(1);
    assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
    assert_eq!(executor.get_fuel(), 0);
}

#[test]
fn exported_import_suspends_without_an_internal_frame() {
    let mut executor = executor(
        r#"(module
          (import "host" "pair" (func $pair (param v128) (result v128)))
          (export "run" (func $pair)))"#,
    );
    executor
        .call_export("run", vec![Value(3), Value(5)])
        .unwrap();
    assert!(matches!(
        executor.execute().unwrap(),
        HostYield::ImportCall { args, should_return: 2, .. }
        if args == vec![Value(3), Value(5)]
    ));
    executor = restore(executor);
    executor.import_return(&[Value(7), Value(11)]).unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(executor.get_result().unwrap(), &vec![Value(7), Value(11)]);
}

#[test]
fn start_imports_remain_pending_until_initialization_completes() {
    for start in ["(start $host)", "(func $start call $host) (start $start)"] {
        let wat = format!(
            r#"(module
              (import "host" "start" (func $host))
              {start}
              (func (export "run") (result i32) i32.const 42))"#
        );
        let mut executor = executor(&wat);
        assert!(executor.call_export("run", vec![]).is_err());
        assert!(matches!(
            executor.execute().unwrap(),
            HostYield::ImportCall { .. }
        ));
        executor = restore(executor);
        executor.import_return(&[]).unwrap();
        assert!(matches!(executor.execute().unwrap(), HostYield::Return));
        executor.call_export("run", vec![]).unwrap();
        assert!(matches!(executor.execute().unwrap(), HostYield::Return));
        assert_eq!(executor.get_result().unwrap(), &vec![Value(42)]);
    }
}

#[test]
fn unsupported_unreachable_code_does_not_trap() {
    assert_eq!(
        run(
            r#"(module (func (export "run") (result i32)
              i32.const 42 return
              f32.const 1 f32.sqrt drop
              i32.const 0))"#,
            vec![],
        ),
        vec![Value(42)],
    );
    let mut executor = executor(
        r#"(module (func (export "run") (result f32)
          f32.const 1 f32.sqrt))"#,
    );
    executor.call_export("run", vec![]).unwrap();
    assert!(matches!(executor.execute(), Err(HaltError::Unsupported(_))));
}
