use crate::{HaltError, HostYield, Value, WackasmExecutor, WackasmModule};

fn module(wat: &str) -> WackasmModule {
    WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap()
}

fn restore(executor: WackasmExecutor) -> WackasmExecutor {
    let bytes = bitcode::serialize(&executor).unwrap();
    drop(executor);
    bitcode::deserialize(&bytes).unwrap()
}

#[test]
fn canonical_signature_ids_match_structural_types_and_rebuild_after_restoration() {
    let module = module(
        r#"(module
          (type (func (param i32) (result i64)))
          (type (func (param i32) (result i64)))
          (type (func (param f32) (result i64)))
          (type (func (param i32) (result f64)))
          (type (func (param v128) (result v128)))
          (type (func (param i64 i64) (result i64 i64)))
          (type (func (param i64 i32)))
          (type (func (param i32 i64)))
          (import "host" "run" (func (type 1)))
          (func (type 0) local.get 0 i64.extend_i32_u))"#,
    );
    let check = |module: &WackasmModule| {
        let functions = &module.functions;
        for (lhs, left) in functions.function_type_indexes.iter().enumerate() {
            for (rhs, right) in functions.function_type_indexes.iter().enumerate() {
                assert_eq!(
                    functions.signature_ids[lhs] == functions.signature_ids[rhs],
                    left == right,
                );
            }
        }
        assert_eq!(
            functions.info[0].signature_id,
            functions.info[1].signature_id
        );
    };
    check(&module);
    let bytes = bitcode::serialize(&module).unwrap();
    let restored: WackasmModule = bitcode::deserialize(&bytes).unwrap();
    check(&restored);
    assert_eq!(bitcode::serialize(&restored).unwrap(), bytes);
}

#[test]
fn indirect_calls_accept_duplicate_types_but_reject_equal_width_mismatches() {
    let mut executor = WackasmExecutor::new(module(
        r#"(module
          (type $expected (func (param i32) (result i32)))
          (type $duplicate (func (param i32) (result i32)))
          (type $wrong_arg (func (param f32) (result i32)))
          (type $wrong_result (func (param i32) (result f32)))
          (table 5 funcref)
          (elem (i32.const 0) $good $bad_arg $bad_result)
          (func $good (type $duplicate) local.get 0 i32.const 3 i32.add)
          (func $bad_arg (type $wrong_arg) i32.const 0)
          (func $bad_result (type $wrong_result) f32.const 0)
          (func (export "run") (param i32) (result i32)
            i32.const 39 local.get 0 call_indirect (type $expected)))"#,
    ))
    .unwrap();
    for index in 0..=5 {
        executor.call_export("run", vec![Value(index)]).unwrap();
        executor = restore(executor);
        if index == 0 {
            assert!(matches!(executor.execute().unwrap(), HostYield::Return));
            assert_eq!(executor.get_result().unwrap(), &vec![Value(42)]);
        } else {
            let Err(HaltError::Trap(error)) = executor.execute() else {
                panic!("expected indirect call trap for table index {index}")
            };
            let expected = match index {
                1 | 2 => "indirect call type mismatch",
                3 | 4 => "uninitialized element",
                _ => "undefined element",
            };
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}

#[test]
fn duplicate_signature_imports_preserve_wide_results_across_suspension() {
    let mut executor = WackasmExecutor::new(module(
        r#"(module
          (type $expected (func (param v128) (result v128)))
          (type $duplicate (func (param v128) (result v128)))
          (import "host" "wide" (func $wide (type $duplicate)))
          (table 1 funcref)
          (elem (i32.const 0) $wide)
          (func (export "run") (param v128) (result i64 v128)
            i64.const 73
            local.get 0 i32.const 0 call_indirect (type $expected)))"#,
    ))
    .unwrap();
    executor
        .call_export("run", vec![Value(11), Value(13)])
        .unwrap();
    assert!(matches!(
        executor.execute().unwrap(),
        HostYield::ImportCall { args, should_return: 2, .. } if args == vec![Value(11), Value(13)]
    ));
    executor = restore(executor);
    executor
        .import_return(&[Value(u64::MAX), Value(17)])
        .unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(
        executor.get_result().unwrap(),
        &vec![Value(73), Value(u64::MAX), Value(17)]
    );
}

#[test]
fn calls_initialize_zero_one_two_and_many_local_slots_on_every_entry() {
    for locals in 0..=4 {
        for indirect in [false, true] {
            let declarations = "(local i64) ".repeat(locals);
            let mut work = String::new();
            for index in 1..=locals {
                work.push_str(&format!(
                    "local.get {index} i64.add i64.const -1 local.set {index} "
                ));
            }
            let call = if indirect {
                "i32.const 0 call_indirect (type $t)"
            } else {
                "call $leaf"
            };
            let mut executor = WackasmExecutor::new(module(&format!(
                r#"(module
                  (type $t (func (param i64) (result i64)))
                  (table 1 funcref) (elem (i32.const 0) $leaf)
                  (func $leaf (type $t) {declarations} local.get 0 {work})
                  (func (export "run") (result i64)
                    i64.const 43 {call} {call}))"#,
            )))
            .unwrap();
            executor.call_export("run", vec![]).unwrap();
            executor.set_fuel(1);
            assert!(matches!(executor.execute().unwrap(), HostYield::OutOfFuel));
            executor = restore(executor);
            executor.set_fuel(1);
            assert!(matches!(executor.execute().unwrap(), HostYield::Return));
            assert_eq!(executor.get_result().unwrap(), &vec![Value(43)]);
        }
    }
}

#[test]
fn small_and_large_result_copies_preserve_live_caller_values() {
    for count in 0..=4 {
        for indirect in [false, true] {
            let results = if count == 0 {
                String::new()
            } else {
                format!("(result {})", "i64 ".repeat(count))
            };
            let values = (0..count)
                .map(|i| format!("i64.const {} ", i + 101))
                .collect::<String>();
            let call = if indirect {
                "i32.const 0 call_indirect (type $t)"
            } else {
                "call $leaf"
            };
            let mut executor = WackasmExecutor::new(module(&format!(
                r#"(module
                  (type $t (func {results}))
                  (table 1 funcref) (elem (i32.const 0) $leaf)
                  (func $leaf (type $t) {values})
                  (func $middle (result i64) {call} {} i64.const 47)
                  (func (export "run") (result i64 i64 {types})
                    i64.const 43 call $middle {call}))"#,
                "drop ".repeat(count),
                types = "i64 ".repeat(count),
            )))
            .unwrap();
            executor.call_export("run", vec![]).unwrap();
            for _ in 0..8 {
                executor.set_fuel(1);
                match executor.execute().unwrap() {
                    HostYield::Return => break,
                    HostYield::OutOfFuel => executor = restore(executor),
                    _ => panic!("unexpected import"),
                }
            }
            let expected = [43, 47]
                .into_iter()
                .chain(101..101 + count as u64)
                .map(Value)
                .collect::<Vec<_>>();
            assert_eq!(executor.get_result().unwrap(), &expected);
        }
    }
}

#[test]
fn loops_and_direct_and_indirect_recursion_always_exhaust_finite_fuel() {
    for body in [
        "(func $recurse (export \"run\") (loop $again br $again))",
        "(func $recurse (export \"run\") call $recurse)",
        "(func $recurse (export \"run\") i32.const 0 call_indirect (type $t))",
        "(func $recurse (export \"run\") call $other) (func $other call $recurse)",
    ] {
        let mut executor = WackasmExecutor::new(module(&format!(
            "(module (type $t (func)) (table 1 funcref)
              (elem (i32.const 0) $recurse) {body})",
        )))
        .unwrap();
        executor.call_export("run", vec![]).unwrap();
        for _ in 0..3 {
            executor.set_fuel(7);
            assert!(matches!(executor.execute().unwrap(), HostYield::OutOfFuel));
            assert_eq!(executor.get_fuel(), 0);
            executor = restore(executor);
        }
    }
}

#[test]
fn restoration_rejects_function_types_without_valid_signature_metadata() {
    for type_index in [false, true] {
        let mut module = module("(module (func))");
        if type_index {
            module.functions.function_types[0].0 = 1;
        } else {
            module.functions.function_types.clear();
        }
        let bytes = bitcode::serialize(&module).unwrap();
        assert!(bitcode::deserialize::<WackasmModule>(&bytes).is_err());
    }
}
