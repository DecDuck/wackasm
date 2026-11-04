use std::time::{SystemTime, UNIX_EPOCH};

use wackasm_spec_tests::{Summary, run_source, run_suite_with_report};

#[test]
fn runs_integer_assert_return() {
    let summary = run_source(
        r#"
            (module
                (func (export "add") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.add))
            (assert_return
                (invoke "add" (i32.const 20) (i32.const 22))
                (i32.const 42))
        "#,
    );
    assert_eq!(
        summary,
        Summary {
            passed: 2,
            failed: 0,
            unsupported: 0
        }
    );
}

#[test]
fn runs_float_vector_and_null_reference_assert_returns() {
    let summary = run_source(
        r#"
            (module
                (func (export "echo_f32") (param f32) (result f32) local.get 0)
                (func (export "echo_f64") (param f64) (result f64) local.get 0)
                (func (export "echo_v128") (param v128) (result v128) local.get 0)
                (func (export "echo_externref") (param externref) (result externref) local.get 0))
            (assert_return (invoke "echo_f32" (f32.const -0)) (f32.const -0))
            (assert_return (invoke "echo_f64" (f64.const 1.5)) (f64.const 1.5))
            (assert_return
                (invoke "echo_v128" (v128.const i32x4 1 2 3 4))
                (v128.const i32x4 1 2 3 4))
            (assert_return
                (invoke "echo_externref" (ref.null extern))
                (ref.null extern))
        "#,
    );

    assert_eq!(
        summary,
        Summary {
            passed: 5,
            failed: 0,
            unsupported: 0
        }
    );
}

#[test]
fn runs_float_and_vector_memory_loads_and_stores() {
    let summary = run_source(
        r#"
            (module
                (memory 1)
                (data (i32.const 0) "\00\00\C0\3F")
                (func (export "load_f32") (result f32)
                    i32.const 0
                    f32.load)
                (func (export "roundtrip_f32") (param i32 f32) (result f32)
                    local.get 0
                    local.get 1
                    f32.store
                    local.get 0
                    f32.load)
                (func (export "roundtrip_f64") (param i32 f64) (result f64)
                    local.get 0
                    local.get 1
                    f64.store
                    local.get 0
                    f64.load)
                (func (export "roundtrip_v128") (param i32 v128) (result v128)
                    local.get 0
                    local.get 1
                    v128.store
                    local.get 0
                    v128.load))
            (assert_return (invoke "load_f32") (f32.const 1.5))
            (assert_return (invoke "roundtrip_f32" (i32.const 4) (f32.const -0)) (f32.const -0))
            (assert_return (invoke "roundtrip_f64" (i32.const 8) (f64.const 2.5)) (f64.const 2.5))
            (assert_return
                (invoke "roundtrip_v128" (i32.const 16) (v128.const i32x4 1 2 3 4))
                (v128.const i32x4 1 2 3 4))
        "#,
    );

    assert_eq!(
        summary,
        Summary {
            passed: 5,
            failed: 0,
            unsupported: 0
        }
    );
}

#[test]
fn reports_unsupported_directives_without_passing_them() {
    let summary = run_source("(assert_unlinkable (module) \"link failure\")");
    assert_eq!(
        summary,
        Summary {
            passed: 0,
            failed: 0,
            unsupported: 1
        }
    );
}

#[test]
fn assert_trap_requires_a_trapping_execution() {
    let missing_export = run_source("(module) (assert_trap (invoke \"missing\") \"unreachable\")");
    assert_eq!(missing_export.failed, 1);

    let actual_trap = run_source(
        "(module (func (export \"trap\") (result i32) i32.const 1 i32.const 0 i32.div_u)) (assert_trap (invoke \"trap\") \"divide by zero\")",
    );
    assert_eq!(actual_trap.failed, 0);
    assert_eq!(actual_trap.unsupported, 0);
}

#[test]
fn classifies_traps_and_unsupported_features_by_error_kind() {
    let summary = run_source(
        r#"
            (module
                (memory 1)
                (func (export "oob") (result i32) i32.const 65536 i32.load)
                (func (export "unsupported") (result f32) f32.const 1 f32.const 2 f32.add))
            (assert_trap (invoke "oob") "out of bounds memory access")
            (assert_return (invoke "oob") (i32.const 0))
            (assert_trap (invoke "unsupported") "unreachable")
            (module (global f32 (f32.const 1)))
        "#,
    );
    assert_eq!(
        summary,
        Summary {
            passed: 2,
            failed: 1,
            unsupported: 2
        }
    );
}

#[test]
fn validates_invalid_and_malformed_assertions() {
    let invalid =
        run_source(r#"(assert_invalid (module (func (result i32) f32.const 0)) "type mismatch")"#);
    assert_eq!(
        invalid,
        Summary {
            passed: 1,
            failed: 0,
            unsupported: 0
        }
    );

    let malformed = run_source(r#"(assert_malformed (module binary "\\00") "unexpected end")"#);
    assert_eq!(
        malformed,
        Summary {
            passed: 1,
            failed: 0,
            unsupported: 0
        }
    );
}

#[test]
fn unsupported_execution_is_reported_and_later_directives_continue() {
    let summary = run_source(
        r#"
            (module (func (export "unsupported") (result f32) f32.const 1 f32.const 2 f32.add))
            (invoke "unsupported")
            (module (func (export "answer") (result i32) i32.const 42))
            (assert_return (invoke "answer") (i32.const 42))
        "#,
    );
    assert_eq!(
        summary,
        Summary {
            passed: 3,
            failed: 0,
            unsupported: 1
        }
    );
}

#[test]
fn links_spectest_and_registered_function_imports() {
    let summary = run_source(
        r#"
            (module $M
                (import "spectest" "print_i32" (func $print (param i32)))
                (func (export "log_and_add") (param i32 i32) (result i32)
                    local.get 0
                    call $print
                    local.get 0
                    local.get 1
                    i32.add)
                (func (export "trap") (result i32) unreachable))
            (register "M" $M)
            (module $N
                (import "M" "log_and_add" (func $add (param i32 i32) (result i32)))
                (import "M" "trap" (func $trap (result i32)))
                (export "reexported" (func $add))
                (func (export "add_one") (param i32) (result i32)
                    local.get 0
                    i32.const 1
                    call $add)
                (func (export "call_trap") (result i32) call $trap))
            (register "N" $N)
            (module $O
                (import "N" "reexported" (func $add (param i32 i32) (result i32)))
                (func (export "add_two") (param i32) (result i32)
                    local.get 0
                    i32.const 2
                    call $add))
            (assert_return (invoke $N "add_one" (i32.const 41)) (i32.const 42))
            (assert_return (invoke $N "reexported" (i32.const 40) (i32.const 2)) (i32.const 42))
            (assert_return (invoke $O "add_two" (i32.const 40)) (i32.const 42))
            (assert_trap (invoke $N "call_trap") "unreachable")
            (assert_return (invoke $N "add_one" (i32.const 1)) (i32.const 2))
        "#,
    );
    assert_eq!(
        summary,
        Summary {
            passed: 10,
            failed: 0,
            unsupported: 0
        }
    );
}

#[test]
fn reports_unresolved_imports_when_instantiating() {
    let summary = run_source(
        r#"
            (module (import "missing" "f" (func)))
            (module (import "spectest" "missing" (func)))
        "#,
    );
    assert_eq!(summary.failed, 2);
}

#[test]
fn writes_searchable_failure_records() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("wackasm-spec-{unique}"));
    std::fs::create_dir_all(&directory).unwrap();
    let test_file = directory.join("result.wast");
    let report_file = directory.join("report.txt");
    std::fs::write(
        &test_file,
        r#"(module (func (export "answer") (result i32) i32.const 42))
           (assert_return (invoke "answer") (i32.const 41))"#,
    )
    .unwrap();

    let summary = run_suite_with_report(&directory, None, Some(&report_file)).unwrap();
    let report = std::fs::read_to_string(&report_file).unwrap();

    assert_eq!(summary.failed, 1);
    assert!(report.contains("FAIL\tresult.wast\tdirective=2\t"));
    assert!(report.contains("SUMMARY\tresult.wast\tpassed=1\tfailed=1"));
    assert!(report.contains("TOTAL\tpassed=1\tfailed=1"));
    std::fs::remove_dir_all(directory).unwrap();
}
