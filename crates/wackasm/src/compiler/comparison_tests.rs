use super::{
    I32Comparison, I64Comparison, WackasmOperator,
    tests::{body, ops},
};
use crate::{HostYield, Value, WackasmExecutor, WackasmModule};

const I32_CASES: [(&str, I32Comparison); 9] = [
    ("eq", I32Comparison::Eq),
    ("ne", I32Comparison::Ne),
    ("lt_s", I32Comparison::LtS),
    ("lt_u", I32Comparison::LtU),
    ("gt_s", I32Comparison::GtS),
    ("gt_u", I32Comparison::GtU),
    ("ge_s", I32Comparison::GeS),
    ("ge_u", I32Comparison::GeU),
    ("le_u", I32Comparison::LeU),
];

const I64_CASES: [(&str, I64Comparison); 7] = [
    ("eq", I64Comparison::Eq),
    ("ne", I64Comparison::Ne),
    ("gt_s", I64Comparison::GtS),
    ("lt_u", I64Comparison::LtU),
    ("gt_u", I64Comparison::GtU),
    ("le_u", I64Comparison::LeU),
    ("ge_u", I64Comparison::GeU),
];

fn conditional(predicate: &str, invert: bool) -> String {
    if invert {
        format!("{predicate} if nop end")
    } else {
        format!("(block {predicate} br_if 0)")
    }
}

#[test]
fn all_i32_immediate_comparisons_fuse_to_one_conditional_dispatch() {
    for (name, expected_kind) in I32_CASES {
        for invert in [false, true] {
            let control = conditional(&format!("local.get 0 i32.const -1 i32.{name}"), invert);
            let body = body(&format!("(module (func (param i32) {control}))"));
            let operators = ops(&body);
            assert_eq!(operators.len(), 2, "{name}, invert={invert}: {operators:?}");
            assert!(matches!(
                &operators[0],
                WackasmOperator::JumpIfI32CmpImm { lhs: 0, imm: -1, kind, invert: actual, offset: 1 }
                    if *kind == expected_kind && *actual == invert
            ));
        }
    }
}

#[test]
fn all_supported_i64_comparisons_fuse_registers_and_immediates() {
    for (name, expected_kind) in I64_CASES {
        for invert in [false, true] {
            for immediate in [false, true] {
                let rhs = if immediate {
                    "i64.const -1"
                } else {
                    "local.get 1"
                };
                let control = conditional(&format!("local.get 0 {rhs} i64.{name}"), invert);
                let body = body(&format!("(module (func (param i64 i64) {control}))"));
                let operators = ops(&body);
                assert_eq!(operators.len(), 2, "{name}: {operators:?}");
                match &operators[0] {
                    WackasmOperator::JumpIfI64CmpImm {
                        lhs: 0,
                        imm: -1,
                        kind,
                        invert: actual,
                        offset: 1,
                    } if immediate => {
                        assert_eq!(*kind, expected_kind);
                        assert_eq!(*actual, invert);
                    }
                    WackasmOperator::JumpIfI64Cmp {
                        lhs: 0,
                        rhs: 1,
                        kind,
                        invert: actual,
                        offset: 1,
                    } if !immediate => {
                        assert_eq!(*kind, expected_kind);
                        assert_eq!(*actual, invert);
                    }
                    _ => panic!("unexpected comparison: {:?}", operators[0]),
                }
            }
        }
    }
}

#[test]
fn i64_immediate_fusion_requires_exact_sign_extension() {
    for value in [
        i32::MIN as i64,
        i32::MAX as i64,
        -1,
        0,
        2147483648,
        4294967295,
        i64::MIN,
        i64::MAX,
    ] {
        let body = body(&format!(
            "(module (func (param i64) (block local.get 0 i64.const {value} i64.lt_u br_if 0)))"
        ));
        let operators = ops(&body);
        if let Ok(imm) = i32::try_from(value) {
            assert_eq!(operators.len(), 2);
            assert!(matches!(
                &operators[0], WackasmOperator::JumpIfI64CmpImm { imm: actual, .. } if *actual == imm
            ));
        } else {
            assert_eq!(operators.len(), 3);
            assert!(matches!(operators[1], WackasmOperator::JumpIfI64Cmp { .. }));
            let bits = match &operators[0] {
                WackasmOperator::CopyImm32 { value, .. } => *value as u64,
                WackasmOperator::CopyImm64 { value, .. } => {
                    value[0] as u64 | ((value[1] as u64) << 32)
                }
                other => panic!("unexpected constant: {other:?}"),
            };
            assert_eq!(bits, value as u64);
        }
    }
}

#[test]
fn i64_eqz_fuses_without_truncating_the_operand() {
    for invert in [false, true] {
        let control = conditional("local.get 0 i64.eqz", invert);
        let body = body(&format!("(module (func (param i64) {control}))"));
        assert!(matches!(
            ops(&body).as_slice(),
            [
                WackasmOperator::JumpIfI64CmpImm {
                    lhs: 0, imm: 0, kind: I64Comparison::Eq, invert: actual, offset: 1,
                },
                WackasmOperator::Return { .. },
            ] if *actual == invert
        ));
    }
    let bytes = wat::parse_str(
        r#"(module (func (export "run") (param i64) (result i32)
          local.get 0 i64.eqz if (result i32) i32.const 1 else i32.const 0 end))"#,
    )
    .unwrap();
    let mut executor = WackasmExecutor::new(WackasmModule::new(&bytes).unwrap()).unwrap();
    for value in [0, 1, 1i64 << 32, i64::MIN] {
        executor
            .call_export("run", vec![Value::from_i64(value)])
            .unwrap();
        assert!(matches!(executor.execute().unwrap(), HostYield::Return));
        assert_eq!(
            executor.get_result().unwrap(),
            &vec![Value((value == 0) as u64)]
        );
    }
}

#[test]
fn labels_and_local_writes_block_immediate_fusion() {
    for (ty, comparison) in [("i32", "I32"), ("i64", "I64")] {
        let body = body(&format!(
            "(module (func (param {ty}) (result i32) (local i32)
              (block local.get 0 {ty}.const 1 {ty}.eq local.tee 1 br_if 0)
              (block (block (result i32) local.get 0 {ty}.const 1 {ty}.eq) br_if 0)
              local.get 1))"
        ));
        let operators = ops(&body);
        assert!(
            !operators.iter().any(|op| matches!(
                op,
                WackasmOperator::JumpIfI32CmpImm { .. } | WackasmOperator::JumpIfI64CmpImm { .. }
            )),
            "{comparison}: {operators:?}"
        );
        assert!(operators.iter().any(|op| matches!(
            op,
            WackasmOperator::I32EqImm { dst: 1, .. } | WackasmOperator::I64CmpImm { dst: 1, .. }
        )));
    }
}

#[test]
fn fused_copy_guards_preserve_branch_results_and_fallthrough() {
    for ty in ["i32", "i64"] {
        let wat = format!(
            "(module (func (export \"run\") (param {ty}) (result i32)
              (block (result i32)
                i32.const 10 i32.const 0 i32.add
                i32.const 99 i32.const 0 i32.add
                local.get 0 {ty}.const 1 {ty}.eq br_if 0
                drop i32.const 1 i32.add)))"
        );
        let body = body(&wat);
        assert!(ops(&body).iter().any(|op| matches!(
            op,
            WackasmOperator::JumpIfI32CmpImm {
                invert: true,
                offset: 3,
                ..
            } | WackasmOperator::JumpIfI64CmpImm {
                invert: true,
                offset: 3,
                ..
            }
        )));
        let bytes = wat::parse_str(wat).unwrap();
        let mut executor = WackasmExecutor::new(WackasmModule::new(&bytes).unwrap()).unwrap();
        for (value, expected) in [(0, 11), (1, 99)] {
            executor.call_export("run", vec![Value(value)]).unwrap();
            assert!(matches!(executor.execute().unwrap(), HostYield::Return));
            assert_eq!(executor.get_result().unwrap(), &vec![Value(expected)]);
        }
    }
}

fn compare_against_wasmi(ty: &str, comparison: &str, constant: Option<i64>, values: &[i64]) {
    let rhs = constant.map_or_else(
        || "local.get 1".to_owned(),
        |value| format!("{ty}.const {value}"),
    );
    let predicate = format!("local.get 0 {rhs} {ty}.{comparison}");
    let bytes = wat::parse_str(format!(
        "(module
          (func (export \"if\") (param {ty} {ty}) (result i32)
            {predicate} if (result i32) i32.const 11 else i32.const 22 end)
          (func (export \"br\") (param {ty} {ty}) (result i32)
            (block {predicate} br_if 0 i32.const 22 return)
            i32.const 11))"
    ))
    .unwrap();
    let mut executor = WackasmExecutor::new(WackasmModule::new(&bytes).unwrap()).unwrap();
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes[..]).unwrap();
    let mut store = wasmi::Store::new(&engine, ());
    let instance = wasmi::Linker::new(&engine)
        .instantiate_and_start(&mut store, &module)
        .unwrap();
    for &lhs in values {
        for &rhs in values {
            for name in ["if", "br"] {
                let (args, expected) = if ty == "i32" {
                    let expected = instance
                        .get_typed_func::<(i32, i32), i32>(&store, name)
                        .unwrap()
                        .call(&mut store, (lhs as i32, rhs as i32))
                        .unwrap();
                    (
                        vec![Value::from_i32(lhs as i32), Value::from_i32(rhs as i32)],
                        expected,
                    )
                } else {
                    let expected = instance
                        .get_typed_func::<(i64, i64), i32>(&store, name)
                        .unwrap()
                        .call(&mut store, (lhs, rhs))
                        .unwrap();
                    (vec![Value::from_i64(lhs), Value::from_i64(rhs)], expected)
                };
                executor.call_export(name, args).unwrap();
                assert!(matches!(executor.execute().unwrap(), HostYield::Return));
                assert_eq!(
                    executor.get_result().unwrap(),
                    &vec![Value::from_i32(expected)],
                    "{ty}.{comparison}, constant={constant:?}, lhs={lhs}, rhs={rhs}, {name}"
                );
            }
        }
    }
}

#[test]
fn fused_comparisons_match_wasmi_at_signed_and_unsigned_boundaries() {
    let i32_values = [i32::MIN as i64, -1, 0, 1, i32::MAX as i64];
    for (comparison, _) in I32_CASES {
        for constant in [
            Some(i32::MIN as i64),
            Some(-1),
            Some(0),
            Some(i32::MAX as i64),
        ] {
            compare_against_wasmi("i32", comparison, constant, &i32_values);
        }
    }
    let i64_values = [
        i64::MIN,
        i32::MIN as i64,
        -1,
        0,
        1,
        i32::MAX as i64,
        4294967295,
        1i64 << 32,
        i64::MAX,
    ];
    for (comparison, _) in I64_CASES {
        for constant in [
            None,
            Some(i32::MIN as i64),
            Some(-1),
            Some(0),
            Some(i32::MAX as i64),
            Some(4294967295),
            Some(i64::MAX),
        ] {
            compare_against_wasmi("i64", comparison, constant, &i64_values);
        }
    }
}

#[test]
fn fused_backward_branches_resume_after_bitcode_suspension() {
    for ty in ["i32", "i64"] {
        for immediate in [false, true] {
            let rhs = if immediate {
                format!("{ty}.const 0")
            } else {
                "local.get 1".to_owned()
            };
            let bytes = wat::parse_str(format!(
                "(module (func (export \"run\") (param {ty} {ty}) (result {ty})
                  (loop $again
                    local.get 0 {ty}.const 1 {ty}.sub local.tee 0
                    {rhs} {ty}.gt_u br_if $again)
                  local.get 0))"
            ))
            .unwrap();
            let mut executor = WackasmExecutor::new(WackasmModule::new(&bytes).unwrap()).unwrap();
            executor
                .call_export("run", vec![Value(5), Value(0)])
                .unwrap();
            let mut suspensions = 0;
            loop {
                assert!(suspensions < 10);
                executor.set_fuel(1);
                match executor.execute().unwrap() {
                    HostYield::Return => break,
                    HostYield::OutOfFuel => {
                        let bytes = bitcode::serialize(&executor).unwrap();
                        drop(executor);
                        executor = bitcode::deserialize(&bytes).unwrap();
                        suspensions += 1;
                    }
                    _ => panic!("unexpected import"),
                }
            }
            assert!(suspensions > 0);
            assert_eq!(executor.get_result().unwrap(), &vec![Value(0)]);
        }
    }
}

#[test]
fn unfused_i64_immediates_retarget_and_preserve_local_aliases_after_restoration() {
    let wat = r#"(module (func (export "run") (param i64) (result i32) (local i32)
      local.get 0 i64.const -1 i64.lt_u local.tee 1
      local.get 1 i32.add))"#;
    let compiled = body(wat);
    assert!(matches!(
        ops(&compiled).as_slice(),
        [
            WackasmOperator::I64CmpImm {
                dst: 1,
                lhs: 0,
                imm: -1,
                kind: I64Comparison::LtU,
            },
            WackasmOperator::I32Add { lhs: 1, rhs: 1, .. },
            WackasmOperator::Return { .. },
        ]
    ));
    let bytes = wat::parse_str(wat).unwrap();
    let mut executor = WackasmExecutor::new(WackasmModule::new(&bytes).unwrap()).unwrap();
    for (value, expected) in [(0, 2), (-1, 0), (i64::MIN, 2)] {
        executor
            .call_export("run", vec![Value::from_i64(value)])
            .unwrap();
        executor.set_fuel(0);
        assert!(matches!(executor.execute().unwrap(), HostYield::OutOfFuel));
        let snapshot = bitcode::serialize(&executor).unwrap();
        drop(executor);
        executor = bitcode::deserialize(&snapshot).unwrap();
        executor.set_fuel(1);
        assert!(matches!(executor.execute().unwrap(), HostYield::Return));
        assert_eq!(executor.get_result().unwrap(), &vec![Value(expected)]);
    }
}
