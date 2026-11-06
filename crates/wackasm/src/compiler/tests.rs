use crate::{
    WackasmModule,
    compiler::{I32Comparison, WackasmFunctionBody, WackasmOperator},
    functions::WackasmFunction,
};

pub(super) fn body(wat: &str) -> WackasmFunctionBody {
    let module = WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap();
    match module.functions.function_bodies.last().unwrap() {
        WackasmFunction::Internal(body) => body.clone(),
        _ => panic!("expected internal function"),
    }
}

pub(super) fn ops(body: &WackasmFunctionBody) -> Vec<WackasmOperator> {
    body.runtime.iter().map(|operator| operator.op()).collect()
}

#[test]
fn local_gets_disappear_and_producer_retargets_into_local() {
    let body = body(
        r#"(module (func (param i32 i32) (local i32)
        local.get 0 local.get 1 i32.add local.set 2))"#,
    );
    assert!(matches!(
        ops(&body).as_slice(),
        [
            WackasmOperator::I32Add {
                dst: 2,
                lhs: 0,
                rhs: 1
            },
            WackasmOperator::Return { len: 0, .. },
        ]
    ));
    assert_eq!(body.max_height_slots, 2);
    assert_eq!(body.theoretical_max_stack_size(), 5);
}

#[test]
fn tee_tracks_local_and_preserves_previous_snapshots() {
    let body = body(
        r#"(module (func (param i32 i32) (result i32 i32)
        local.get 0
        local.get 0 local.get 1 i32.add local.tee 0))"#,
    );
    let operators = ops(&body);
    assert!(
        operators
            .iter()
            .any(|op| matches!(op, WackasmOperator::Copy { dst: 2, src: 0 }))
    );
    // Snapshot materialisation blocks unsafe retargeting over the copy.
    assert!(operators.iter().any(|op| matches!(
        op,
        WackasmOperator::I32Add {
            dst: 3,
            lhs: 0,
            rhs: 1
        }
    )));
    assert!(
        operators
            .iter()
            .any(|op| matches!(op, WackasmOperator::Copy { dst: 0, src: 3 }))
    );
}

#[test]
fn immediate_binary_and_comparison_variants_use_signed_i32_fields() {
    let body = body(
        r#"(module (func (param i32) (result i32)
        local.get 0 i32.const -7 i32.add
        i32.const 3 i32.sub
        i32.const 255 i32.and
        i32.const 2 i32.shl
        i32.const 1 i32.shr_u
        i32.const -1 i32.lt_u))"#,
    );
    assert!(matches!(
        ops(&body).as_slice(),
        [
            WackasmOperator::I32AddImm {
                lhs: 0,
                imm: -7,
                ..
            },
            WackasmOperator::I32SubImm { imm: 3, .. },
            WackasmOperator::I32AndImm { imm: 255, .. },
            WackasmOperator::I32ShlImm { imm: 2, .. },
            WackasmOperator::I32ShrUImm { imm: 1, .. },
            WackasmOperator::I32LtUImm { imm: -1, .. },
            WackasmOperator::Return { len: 1, .. },
        ]
    ));
}

#[test]
fn i64_immediates_are_only_selected_when_sign_extension_is_exact() {
    let body = body(
        r#"(module (func (param i64) (result i64)
        local.get 0 i64.const -2147483648 i64.add
        i64.const 2147483648 i64.add))"#,
    );
    assert!(matches!(
        ops(&body).as_slice(),
        [
            WackasmOperator::I64AddImm { imm: i32::MIN, .. },
            WackasmOperator::CopyImm32 {
                value: 2147483648,
                ..
            },
            WackasmOperator::I64Add { .. },
            WackasmOperator::Return { .. },
        ]
    ));
}

#[test]
fn constants_preserve_raw_integer_and_float_bits_without_a_pool() {
    let body = body(
        r#"(module (func (result i32 f32 i64 f64)
        i32.const -1 f32.const -0 i64.const -9223372036854775808 f64.const -0))"#,
    );
    assert!(matches!(
        ops(&body).as_slice(),
        [
            WackasmOperator::CopyImm64 {
                value: [u32::MAX, u32::MAX],
                ..
            },
            WackasmOperator::CopyImm32 {
                value: 0x80000000,
                ..
            },
            WackasmOperator::CopyImm64 {
                value: [0, 0x80000000],
                ..
            },
            WackasmOperator::CopyImm64 {
                value: [0, 0x80000000],
                ..
            },
            WackasmOperator::Return { len: 4, .. },
        ]
    ));
}

#[test]
fn compares_fuse_into_both_branch_polarities() {
    let body = body(
        r#"(module (func (param i32 i32)
        (block local.get 0 local.get 1 i32.lt_s br_if 0)
        local.get 0 local.get 1 i32.eq if nop end))"#,
    );
    assert!(ops(&body).iter().any(|op| matches!(
        op,
        WackasmOperator::JumpIfI32Cmp {
            lhs: 0,
            rhs: 1,
            kind: I32Comparison::LtS,
            invert: false,
            ..
        }
    )));
    assert!(ops(&body).iter().any(|op| matches!(
        op,
        WackasmOperator::JumpIfI32Cmp {
            lhs: 0,
            rhs: 1,
            kind: I32Comparison::Eq,
            invert: true,
            ..
        }
    )));
    assert!(!ops(&body).iter().any(|op| matches!(
        op,
        WackasmOperator::I32LtS { .. } | WackasmOperator::I32Eq { .. }
    )));
}

#[test]
fn eqz_folds_into_inverted_conditional_branches() {
    let body = body(
        r#"(module (func (param i32)
        (block local.get 0 i32.eqz br_if 0)
        local.get 0 i32.eqz if nop end))"#,
    );
    assert!(
        ops(&body)
            .iter()
            .any(|op| matches!(op, WackasmOperator::JumpIfEqz { cond: 0, .. }))
    );
    assert!(
        ops(&body)
            .iter()
            .any(|op| matches!(op, WackasmOperator::JumpIfNez { cond: 0, .. }))
    );
    assert!(
        !ops(&body)
            .iter()
            .any(|op| matches!(op, WackasmOperator::I32Eqz { .. }))
    );
}

#[test]
fn labels_prevent_fusing_or_retargeting_a_shared_result() {
    let body = body(
        r#"(module (func (param i32 i32) (local i32)
        (block (result i32)
            local.get 0 local.get 1 i32.eq)
        local.set 2
        (block
            (block (result i32)
                local.get 0 local.get 1 i32.lt_s)
            br_if 0)))"#,
    );
    assert!(
        ops(&body)
            .iter()
            .any(|op| matches!(op, WackasmOperator::I32Eq { dst: 3, .. }))
    );
    assert!(
        ops(&body)
            .iter()
            .any(|op| matches!(op, WackasmOperator::Copy { dst: 2, src: 3 }))
    );
    assert!(
        ops(&body)
            .iter()
            .any(|op| matches!(op, WackasmOperator::I32LtS { .. }))
    );
    assert!(
        !ops(&body)
            .iter()
            .any(|op| matches!(op, WackasmOperator::JumpIfI32Cmp { .. }))
    );
}

#[test]
fn table_is_inline_and_copy_targets_have_relative_trampolines() {
    let body = body(
        r#"(module (func (param i32) (result i32)
        (block $outer (result i32)
          i32.const 10
          (block $inner (result i32)
            i32.const 20 i32.const 30
            local.get 0 br_table $inner $outer)
          i32.add)))"#,
    );
    let operators = ops(&body);
    let table = operators
        .iter()
        .position(|op| matches!(op, WackasmOperator::BrTable { len: 2, .. }))
        .unwrap();
    for entry in table + 1..=table + 2 {
        let WackasmOperator::Jump { offset } = &operators[entry] else {
            panic!("table entry is not an inline jump");
        };
        let trampoline = usize::try_from(entry as i64 + *offset as i64).unwrap();
        assert!(trampoline > table + 2);
        assert!(matches!(
            operators[trampoline],
            WackasmOperator::CopySpan { len: 1, .. }
        ));
        assert!(matches!(
            operators[trampoline + 1],
            WackasmOperator::Jump { .. }
        ));
    }
}

#[test]
fn table_can_mix_direct_entries_and_return_trampolines() {
    let body = body(
        r#"(module (func (param i32) (result i32)
        (block (result i32)
          i32.const 7 local.get 0 br_table 0 1)))"#,
    );
    let operators = ops(&body);
    let table = operators
        .iter()
        .position(|op| matches!(op, WackasmOperator::BrTable { len: 2, .. }))
        .unwrap();
    assert!(
        operators
            .iter()
            .skip(table + 3)
            .any(|op| matches!(op, WackasmOperator::Return { len: 1, .. }))
    );
    assert!(
        !operators
            .iter()
            .any(|op| matches!(op, WackasmOperator::CopySpan { .. }))
    );
}

#[test]
fn loops_have_backward_relative_jumps_and_are_the_only_fuel_sites() {
    let body = body(
        r#"(module (func (param i32)
        (block
          (loop local.get 0 br_if 0))))"#,
    );
    assert_eq!(
        ops(&body)
            .iter()
            .filter(|op| matches!(op, WackasmOperator::ConsumeFuel))
            .count(),
        1
    );
    let operators = ops(&body);
    let fuel = operators
        .iter()
        .position(|op| matches!(op, WackasmOperator::ConsumeFuel))
        .unwrap();
    let (pc, offset) = operators
        .iter()
        .enumerate()
        .find_map(|(pc, op)| match op {
            WackasmOperator::JumpIfNez { offset, .. } => Some((pc, *offset)),
            _ => None,
        })
        .unwrap();
    assert_eq!(pc as i32 + offset, fuel as i32);
}

#[test]
fn wide_values_use_exact_slots_and_wide_copy_paths() {
    let body = body(
        r#"(module (func (param v128 i32) (result v128) (local v128)
        local.get 0 local.set 2
        local.get 0 local.get 2 local.get 1 select))"#,
    );
    assert_eq!(body.params_slots, 3);
    assert_eq!(body.local_defaults.len(), 2);
    assert_eq!(body.max_height_slots, 5);
    assert_eq!(body.theoretical_max_stack_size(), 10);
    assert!(
        ops(&body)
            .iter()
            .any(|op| matches!(op, WackasmOperator::CopyWide { dst: 3, src: 0 }))
    );
    assert!(ops(&body).iter().any(|op| matches!(
        op,
        WackasmOperator::SelectWide {
            dst: 5,
            cond: 2,
            a: 0,
            b: 3
        }
    )));
}

#[test]
fn call_result_slots_are_included_in_frame_height() {
    let body = body(
        r#"(module
        (func $many (result v128 i64) (unreachable))
        (func (result v128 i64) call $many))"#,
    );
    assert_eq!(body.max_height_slots, 3);
    assert_eq!(body.theoretical_max_stack_size(), 3);
    assert!(matches!(
        ops(&body).as_slice(),
        [
            WackasmOperator::Call { base: 0, .. },
            WackasmOperator::Return { src: 0, len: 3 },
        ]
    ));
}

#[test]
fn unsupported_and_polymorphic_nested_control_reconcile_with_validator() {
    let compiled = body(
        r#"(module (func (param i32) (result i32)
        local.get 0 if (result i32)
          i32.const 8 i32.const 2 i32.div_s
        else
          i32.const 5
        end))"#,
    );
    assert_eq!(
        ops(&compiled)
            .iter()
            .filter(|op| matches!(op, WackasmOperator::Unsupported { .. }))
            .count(),
        1
    );
    let dead = body(
        r#"(module (func
        unreachable
        (block (param v128) (result v128)
            (loop (param v128) (result v128) br 0))
        drop))"#,
    );
    assert!(matches!(
        ops(&dead).first(),
        Some(WackasmOperator::Unreachable)
    ));
}

#[test]
fn register_and_width_bounds_are_checked() {
    use crate::compiler::register::{Provider, RegStack};
    let mut stack = RegStack::new(65533);
    stack.push(Provider::Const(0), 2).unwrap();
    assert!(stack.push(Provider::Const(0), 1).is_err());
    let mut stack = RegStack::new(65534);
    assert!(stack.push(Provider::Const(0), 2).is_err());
    let wat = format!("(module (func (local {})))", "i32 ".repeat(65536));
    assert!(WackasmModule::new(&wat::parse_str(&wat).unwrap()).is_err());
}

#[test]
fn instruction_layout_stays_small() {
    assert!(std::mem::size_of::<WackasmOperator>() <= 16);
}

#[test]
fn commutative_immediates_accept_constants_on_either_side() {
    let body = body(
        r#"(module (func (param i32 i64) (result i32 i32 i64)
        i32.const 7 local.get 0 i32.add
        i32.const 255 local.get 0 i32.and
        i64.const -9 local.get 1 i64.add))"#,
    );
    assert!(matches!(
        ops(&body).as_slice(),
        [
            WackasmOperator::I32AddImm { lhs: 0, imm: 7, .. },
            WackasmOperator::I32AndImm {
                lhs: 0,
                imm: 255,
                ..
            },
            WackasmOperator::I64AddImm {
                lhs: 1,
                imm: -9,
                ..
            },
            WackasmOperator::Return { len: 3, .. },
        ]
    ));
}

#[test]
fn all_supported_i32_comparisons_have_immediate_and_fused_forms() {
    use WackasmOperator::*;
    let cases = [
        (
            "eq",
            I32EqImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::Eq,
        ),
        (
            "ne",
            I32NeImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::Ne,
        ),
        (
            "lt_s",
            I32LtSImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::LtS,
        ),
        (
            "lt_u",
            I32LtUImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::LtU,
        ),
        (
            "gt_s",
            I32GtSImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::GtS,
        ),
        (
            "gt_u",
            I32GtUImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::GtU,
        ),
        (
            "ge_s",
            I32GeSImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::GeS,
        ),
        (
            "ge_u",
            I32GeUImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::GeU,
        ),
        (
            "le_u",
            I32LeUImm {
                dst: 0,
                lhs: 0,
                imm: 0,
            },
            I32Comparison::LeU,
        ),
    ];
    for (name, expected, kind) in cases {
        let compiled = body(&format!(
            "(module (func (param i32) (result i32)
             local.get 0 i32.const -3 i32.{name}))",
        ));
        assert_eq!(
            std::mem::discriminant(&compiled.runtime[0].op()),
            std::mem::discriminant(&expected),
            "{name}",
        );
        let compiled = body(&format!(
            "(module (func (param i32 i32)
             (block local.get 0 local.get 1 i32.{name} br_if 0)))",
        ));
        assert!(
            matches!(compiled.runtime[0].op(),
            JumpIfI32Cmp { kind: actual, .. }
            if std::mem::discriminant(&actual) == std::mem::discriminant(&kind)),
            "{name}"
        );
    }
}

#[test]
fn duplicate_table_targets_share_a_single_trampoline() {
    let body = body(
        r#"(module (func (param i32) (result i32)
        (block
          i32.const 5 local.get 0 br_table 1 1 1)
        i32.const 9))"#,
    );
    let operators = ops(&body);
    let table = operators
        .iter()
        .position(|op| matches!(op, WackasmOperator::BrTable { len: 3, .. }))
        .unwrap();
    let targets = (table + 1..=table + 3)
        .map(|pc| match &operators[pc] {
            WackasmOperator::Jump { offset } => pc as i32 + offset,
            _ => panic!("expected inline jump"),
        })
        .collect::<Vec<_>>();
    assert!(targets.iter().all(|target| *target == targets[0]));
    assert!(matches!(
        operators[targets[0] as usize],
        WackasmOperator::Return { .. }
    ));
}

#[test]
fn br_if_copy_skip_can_fuse_comparison_and_preserve_fallthrough() {
    let compiled = body(
        r#"(module (func (param i32 i32) (result i32)
        (block (result i32)
          i32.const 10 i32.const 20
          local.get 0 local.get 1 i32.lt_s br_if 0
          drop)))"#,
    );
    let operators = ops(&compiled);
    // Constants must be materialised before the conditional, so this case
    // deliberately retains the comparison rather than jumping over snapshots.
    assert!(
        operators
            .iter()
            .any(|op| matches!(op, WackasmOperator::JumpIfEqz { .. }))
    );
    assert!(
        operators
            .iter()
            .any(|op| matches!(op, WackasmOperator::CopySpan { len: 1, .. }))
    );
    let fused = body(
        r#"(module (func (param i32 i32) (result i32)
        (block (result i32)
          local.get 0 local.get 1 i32.add
          local.get 0 local.get 1 i32.add
          local.get 0 local.get 1 i32.lt_s br_if 0
          drop)))"#,
    );
    assert!(
        ops(&fused)
            .iter()
            .any(|op| matches!(op, WackasmOperator::JumpIfI32Cmp { invert: true, .. }))
    );
}

#[test]
fn factorial_ssa_loop_parameters_survive_backedges_and_multi_result_calls() {
    let wasm = wat::parse_str(
        r#"(module
            (func $pick0 (param i64) (result i64 i64)
                local.get 0 local.get 0)
            (func $pick1 (param i64 i64) (result i64 i64 i64)
                local.get 0 local.get 1 local.get 0)
            (func (export "fac-ssa") (param i64) (result i64)
                i64.const 1 local.get 0
                (loop $l (param i64 i64) (result i64)
                    call $pick1 call $pick1 i64.mul
                    call $pick1 i64.const 1 i64.sub
                    call $pick0 i64.const 0 i64.gt_u
                    br_if $l
                    drop return)))"#,
    )
    .unwrap();
    let module = WackasmModule::new(&wasm).unwrap();
    if let WackasmFunction::Internal(body) = module.functions.function_bodies.last().unwrap() {
        for (pc, operator) in body.runtime.iter().enumerate() {
            if let WackasmOperator::Jump { offset } = operator.op()
                && offset < 0
            {
                assert!(matches!(
                    body.runtime[(pc as i32 + offset) as usize].op(),
                    WackasmOperator::ConsumeFuel,
                ));
            }
        }
    }
    let mut executor = crate::WackasmExecutor::new(module).unwrap();
    executor
        .call_export("fac-ssa", vec![crate::Value::from_i64(25)])
        .unwrap();
    executor.set_fuel(1000);
    assert!(matches!(
        executor.execute().unwrap(),
        crate::HostYield::Return
    ));
    assert_eq!(
        executor.get_result().unwrap(),
        &vec![crate::Value::from_i64(7034535277573963776)],
    );
}
