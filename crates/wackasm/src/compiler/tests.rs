use crate::{
    WackasmModule,
    compiler::{I32Comparison, WackasmFunctionBody, WackasmMemArg, WackasmOperator},
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
    // Backedges charge fuel themselves and skip the header's ConsumeFuel.
    assert_eq!(pc as i32 + offset, fuel as i32 + 1);
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
                    body.runtime[(pc as i32 + offset - 1) as usize].op(),
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

#[test]
fn exit_branch_then_backedge_inverts_into_one_metered_backedge() {
    let wat = r#"(module (memory 1)
      (func (export "run") (param i32 i32) (result i32)
        (block $exit
          (loop $loop
            local.get 0 i32.const -1 i32.store8
            local.get 0 i32.const 1 i32.add local.tee 0
            local.get 1 i32.eq br_if $exit
            br $loop))
        local.get 0))"#;
    let operators = ops(&body(wat));
    let fuel = operators
        .iter()
        .position(|op| matches!(op, WackasmOperator::ConsumeFuel))
        .unwrap();
    let backedge = operators
        .iter()
        .position(|op| matches!(op, WackasmOperator::JumpIfI32Cmp { .. }))
        .unwrap();
    assert!(matches!(
        operators[backedge],
        WackasmOperator::JumpIfI32Cmp { kind: I32Comparison::Eq, invert: true, offset, .. }
            if backedge as i32 + offset == fuel as i32 + 1
    ));
    assert!(matches!(operators[backedge + 1], WackasmOperator::Jump { offset } if offset > 0));

    let mut executor =
        crate::WackasmExecutor::new(WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap())
            .unwrap();
    executor
        .call_export("run", vec![crate::Value(0), crate::Value(4)])
        .unwrap();
    executor.set_fuel(isize::MAX);
    assert!(matches!(executor.execute().unwrap(), crate::HostYield::Return));
    assert_eq!(executor.get_result().unwrap(), &vec![crate::Value(4)]);
    // One header entry plus three backedges, as when every iteration passed the header.
    assert_eq!(isize::MAX - executor.get_fuel(), 4);
    assert_eq!(
        executor.memory().read_bytes(0, 0, 5).unwrap(),
        &[255, 255, 255, 255, 0]
    );
}

#[test]
fn constant_memory0_stores_use_exact_immediates() {
    let wat = r#"(module (memory 1) (memory 1)
      (func (export "run") (param i32)
        local.get 0 i64.const -2 i64.store
        local.get 0 i64.const 0x123456789 i64.store offset=8
        local.get 0 i64.const 0x1ff i64.store8 offset=16
        local.get 0 f32.const -1.5 f32.store offset=20
        local.get 0 i32.const 7 i32.store 1))"#;
    let operators = ops(&body(wat));
    assert!(operators.iter().any(|op| matches!(
        op,
        WackasmOperator::I64StoreImm { value: -2, offset: 0, .. }
    )));
    assert!(operators.iter().any(|op| matches!(
        op,
        WackasmOperator::I64Store { memarg: WackasmMemArg { offset: 8, .. }, .. }
    )));
    assert!(operators.iter().any(|op| matches!(
        op,
        WackasmOperator::I32Store8Imm { value: 0x1ff, offset: 16, .. }
    )));
    assert!(operators.iter().any(|op| matches!(
        op,
        WackasmOperator::I32Store { memarg: WackasmMemArg { memory: 1, .. }, .. }
    )));

    let mut executor =
        crate::WackasmExecutor::new(WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap())
            .unwrap();
    executor.call_export("run", vec![crate::Value(8)]).unwrap();
    assert!(matches!(executor.execute().unwrap(), crate::HostYield::Return));
    let memory = executor.memory();
    assert_eq!(memory.read_bytes(0, 8, 8).unwrap(), &(-2i64).to_le_bytes());
    assert_eq!(memory.read_bytes(0, 16, 8).unwrap(), &0x123456789u64.to_le_bytes());
    assert_eq!(memory.read_bytes(0, 24, 1).unwrap(), &[0xff]);
    assert_eq!(memory.read_bytes(0, 28, 4).unwrap(), &(-1.5f32).to_le_bytes());
    assert_eq!(memory.read_bytes(1, 8, 4).unwrap(), &7u32.to_le_bytes());

    executor.call_export("run", vec![crate::Value(65530)]).unwrap();
    let before = executor.memory().bytes(0).unwrap().to_vec();
    assert!(matches!(executor.execute(), Err(crate::HaltError::Trap(_))));
    assert_eq!(executor.memory().bytes(0).unwrap(), before);
}

fn run_i32(executor: &mut crate::WackasmExecutor, name: &str, args: &[i32]) -> Vec<crate::Value> {
    let args = args.iter().map(|&arg| crate::Value::from_i32(arg)).collect();
    executor.call_export(name, args).unwrap();
    assert!(matches!(executor.execute().unwrap(), crate::HostYield::Return));
    executor.get_result().unwrap().clone()
}

#[test]
fn add_immediates_fuse_into_zero_test_backedges_and_retry_after_suspension() {
    let wat = r#"(module
      (func (export "tee") (param i32) (result i32) (local i32)
        (loop $l
          local.get 1 i32.const 1 i32.add local.set 1
          local.get 0 i32.const -1 i32.add local.tee 0
          br_if $l)
        local.get 1)
      (func (export "eqz") (param i32) (result i32) (local i32)
        (block $exit
          (loop $l
            local.get 1 i32.const 1 i32.add local.set 1
            local.get 0 i32.const 1 i32.sub local.tee 0
            i32.eqz br_if $exit
            br $l))
        local.get 1))"#;
    let module = WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap();
    for index in [0, 1] {
        let WackasmFunction::Internal(body) = &module.functions.function_bodies[index] else {
            panic!("expected internal function")
        };
        assert!(
            ops(body).iter().any(|op| matches!(
                op,
                WackasmOperator::I32AddImmJumpIfNez { dst: 0, lhs: 0, imm: -1, offset }
                    if *offset < 0
            )),
            "{:?}",
            ops(body)
        );
    }
    let mut executor = crate::WackasmExecutor::new(module).unwrap();
    for name in ["tee", "eqz"] {
        executor.set_fuel(isize::MAX);
        assert_eq!(run_i32(&mut executor, name, &[5]), vec![crate::Value(5)]);
        // Exhaust fuel on every backedge; a retried backedge must not repeat its add.
        executor
            .call_export(name, vec![crate::Value(5)])
            .unwrap();
        let mut suspensions = 0;
        loop {
            executor.set_fuel(1);
            match executor.execute().unwrap() {
                crate::HostYield::Return => break,
                crate::HostYield::OutOfFuel => {
                    let bytes = bitcode::serialize(&executor).unwrap();
                    drop(executor);
                    executor = bitcode::deserialize(&bytes).unwrap();
                    suspensions += 1;
                }
                _ => panic!("unexpected import"),
            }
        }
        assert_eq!(suspensions, 4);
        assert_eq!(executor.get_result().unwrap(), &vec![crate::Value(5)]);
    }
}

#[test]
fn register_metered_fuel_is_written_back_when_traps_end_dispatch() {
    let wat = r#"(module (memory 1)
      (func (export "oob") (param i32)
        (loop $l
          local.get 0 i32.const -1 i32.add local.tee 0
          br_if $l)
        i32.const 70000 i32.load drop)
      (func (export "unreachable") (param i32)
        (loop $l
          local.get 0 i32.const -1 i32.add local.tee 0
          br_if $l)
        unreachable))"#;
    let mut executor =
        crate::WackasmExecutor::new(WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap())
            .unwrap();
    for name in ["oob", "unreachable"] {
        executor.set_fuel(100);
        executor.call_export(name, vec![crate::Value(4)]).unwrap();
        assert!(matches!(executor.execute(), Err(crate::HaltError::Trap(_))));
        // One loop entry plus three backedges.
        assert_eq!(executor.get_fuel(), 96, "{name}");
    }
}

#[test]
fn branch_targets_between_add_and_zero_test_block_fusion() {
    let wat = r#"(module
      (func (export "run") (param i32 i32) (result i32)
        (block $b
          (block $inner
            local.get 1 br_if $inner
            local.get 0 i32.const 1 i32.add local.set 0)
          local.get 0 br_if $b
          i32.const 0 return)
        i32.const 1))"#;
    assert!(!ops(&body(wat)).iter().any(|op| matches!(
        op,
        WackasmOperator::I32AddImmJumpIfNez { .. } | WackasmOperator::I32AddImmJumpIfEqz { .. }
    )));
    let mut executor =
        crate::WackasmExecutor::new(WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap())
            .unwrap();
    for (args, expected) in [([5, 1], 1), ([0, 1], 0), ([-1, 0], 0), ([0, 0], 1)] {
        assert_eq!(run_i32(&mut executor, "run", &args), vec![crate::Value(expected)]);
    }
}

#[test]
fn add_immediate_addresses_fold_into_memory0_loads_with_32_bit_wrapping() {
    let wat = r#"(module (memory 1)
      (data (i32.const 0) "\01\02\03\04\05\06\07\08\09")
      (func (export "run") (param i32) (result i32 i64 i32)
        local.get 0 i32.const 1 i32.add i32.load8_u
        local.get 0 i32.const -1 i32.sub i64.load offset=1
        local.get 0 i32.const 1 i32.add i32.load16_s offset=4))"#;
    let operators = ops(&body(wat));
    assert!(matches!(
        operators.as_slice(),
        [
            WackasmOperator::I32Load8URel { base: 0, delta: 1, offset: 0, .. },
            WackasmOperator::I64LoadRel { base: 0, delta: 1, offset: 1, .. },
            WackasmOperator::I32Load16SRel { base: 0, delta: 1, offset: 4, .. },
            WackasmOperator::Return { .. },
        ]
    ));
    let mut executor =
        crate::WackasmExecutor::new(WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        run_i32(&mut executor, "run", &[0]),
        vec![
            crate::Value(2),
            crate::Value(u64::from_le_bytes([3, 4, 5, 6, 7, 8, 9, 0])),
            crate::Value(0x0706),
        ]
    );
    // The add wraps to address 0 before the offset is applied, so this does not trap.
    assert_eq!(
        run_i32(&mut executor, "run", &[-1]),
        vec![
            crate::Value(1),
            crate::Value(u64::from_le_bytes([2, 3, 4, 5, 6, 7, 8, 9])),
            crate::Value(0x0605),
        ]
    );
    executor
        .call_export("run", vec![crate::Value(65535)])
        .unwrap();
    assert!(matches!(executor.execute(), Err(crate::HaltError::Trap(_))));
}

#[test]
fn register_sum_addresses_fold_into_memory0_base_index_accesses() {
    let wat = r#"(module (memory 1) (memory 1)
      (data (i32.const 0) "\01\02\80\04")
      (func (export "run") (param i32 i32 i64) (result i32 i64 i32)
        local.get 0 local.get 1 i32.add i32.const 0x1ab i32.store8 offset=16
        local.get 0 local.get 1 i32.add local.get 2 i64.store offset=24
        local.get 0 local.get 1 i32.add i32.load8_s offset=2
        local.get 0 local.get 1 i32.add i64.load offset=24
        local.get 0 local.get 1 i32.add i32.load16_u offset=16)
      (func (export "other") (param i32 i32)
        local.get 0 local.get 1 i32.add i32.const 7 i32.store8 1))"#;
    let module = WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap();
    let operators = |index: usize| {
        let WackasmFunction::Internal(body) = &module.functions.function_bodies[index] else {
            panic!("expected internal function")
        };
        ops(body)
    };
    assert!(matches!(
        operators(0).as_slice(),
        [
            WackasmOperator::Store8ImmIdx { base: 0, index: 1, value: 0x1ab, offset: 16 },
            WackasmOperator::Store64Idx { base: 0, index: 1, value: 2, offset: 24 },
            WackasmOperator::LoadI8Idx { base: 0, index: 1, offset: 2, .. },
            WackasmOperator::LoadI64Idx { base: 0, index: 1, offset: 24, .. },
            WackasmOperator::LoadU16Idx { base: 0, index: 1, offset: 16, .. },
            WackasmOperator::Return { .. },
        ]
    ));
    // Only memory 0 has base+index forms.
    assert!(operators(1).iter().any(|op| matches!(op, WackasmOperator::I32Add { .. })));

    let mut executor = crate::WackasmExecutor::new(module).unwrap();
    // The second case wraps `base + index` to 0 before the offset, so it must not trap.
    for (base, index) in [(0, 0), (u32::MAX, 1)] {
        executor
            .call_export(
                "run",
                vec![crate::Value::from_u32(base), crate::Value::from_u32(index), crate::Value::from_i64(-5)],
            )
            .unwrap();
        assert!(matches!(executor.execute().unwrap(), crate::HostYield::Return));
        assert_eq!(
            executor.get_result().unwrap(),
            &vec![crate::Value::from_i32(-128), crate::Value::from_i64(-5), crate::Value(0xab)]
        );
    }
    executor
        .call_export("run", vec![crate::Value(65535), crate::Value(0), crate::Value(0)])
        .unwrap();
    let before = executor.memory().bytes(0).unwrap().to_vec();
    assert!(matches!(executor.execute(), Err(crate::HaltError::Trap(_))));
    assert_eq!(executor.memory().bytes(0).unwrap(), before);
}

#[test]
fn base_index_stores_do_not_fold_when_the_value_would_clobber_the_index() {
    // The index is a temporary in the slot that the wide constant materializes into.
    let wat = r#"(module (memory 1)
      (func (export "run") (param i32) (result i64)
        local.get 0 local.get 0 i32.const 1 i32.and i32.add
        i64.const 0x123456789 i64.store
        local.get 0 i64.load))"#;
    assert!(ops(&body(wat)).iter().any(|op| matches!(op, WackasmOperator::I64Store { .. })));
    let mut executor =
        crate::WackasmExecutor::new(WackasmModule::new(&wat::parse_str(wat).unwrap()).unwrap())
            .unwrap();
    assert_eq!(run_i32(&mut executor, "run", &[8]), vec![crate::Value(0x123456789)]);
}
