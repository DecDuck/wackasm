use super::{
    WackasmFunctionBody, WackasmOperator,
    tests::{body, ops},
};
use crate::{
    HaltError, HostYield, Value, WackasmExecutor, WackasmModule, functions::WackasmFunction,
};

const LOW: u64 = 0x8877_6655_8833_ff80;
const HIGH: u64 = 0xffee_ddcc_bbaa_9900;

fn first_body(module: &WackasmModule) -> &WackasmFunctionBody {
    let WackasmFunction::Internal(body) = &module.functions.function_bodies[0] else {
        panic!("expected internal function")
    };
    body
}

fn check_restoration(module: &WackasmModule, corrupt: impl FnOnce(&mut WackasmOperator)) {
    let bytes = bitcode::serialize(module).unwrap();
    let mut restored: WackasmModule = bitcode::deserialize(&bytes).unwrap();
    assert!(std::ptr::fn_addr_eq(
        first_body(module).runtime[0].handler(),
        first_body(&restored).runtime[0].handler(),
    ));
    let WackasmFunction::Internal(body) = &mut restored.functions.function_bodies[0] else {
        panic!("expected internal function")
    };
    let mut op = body.runtime[0].op();
    corrupt(&mut op);
    body.runtime[0] = op.into();
    let bytes = bitcode::serialize(&restored).unwrap();
    assert!(bitcode::deserialize::<WackasmModule>(&bytes).is_err());
}

#[test]
fn all_constant_loads_fold_offsets_retarget_and_preserve_values_and_bounds() {
    macro_rules! check {
        ($instruction:literal, $ty:literal, $variant:ident, $width:literal, $expected:expr) => {{
            let mut memory0_handler = None;
            for memory in [0, 1] {
                let bytes = wat::parse_str(format!(
                    r#"(module (memory 1) (memory 1)
                      (data (memory {memory}) (i32.const 22)
                        "\80\ff\33\88\55\66\77\88\00\99\aa\bb\cc\dd\ee\ff")
                      (func (export "run") (result {ty})
                        i32.const 17 {instruction} {memory} offset=5)
                      (func (export "last") (result {ty})
                        i32.const {last} {instruction} {memory})
                      (func (export "bad") (result {ty})
                        i32.const {bad} {instruction} {memory}))"#,
                    ty = $ty,
                    instruction = $instruction,
                    last = 65536 - $width,
                    bad = 65537 - $width,
                )).unwrap();
                let module = WackasmModule::new(&bytes).unwrap();
                let operators = ops(first_body(&module));
                assert_eq!(operators.len(), 2, "{operators:?}");
                assert!(matches!(operators[0],
                    WackasmOperator::$variant { dst: 0, memarg }
                    if memarg.offset == 22 && memarg.memory == memory
                ));
                let handler = first_body(&module).runtime[0].handler();
                if let Some(zero) = memory0_handler {
                    assert!(!std::ptr::fn_addr_eq(zero, handler));
                } else {
                    memory0_handler = Some(handler);
                }
                check_restoration(&module, |op| {
                    let WackasmOperator::$variant { memarg, .. } = op else { panic!("{op:?}") };
                    memarg.memory = 2;
                });
                let mut executor = WackasmExecutor::new(module).unwrap();
                executor.call_export("run", vec![]).unwrap();
                assert!(matches!(executor.execute().unwrap(), HostYield::Return));
                let expected: Vec<Value> = $expected;
                assert_eq!(executor.get_result().unwrap(), &expected, "{}", $instruction);
                executor.call_export("last", vec![]).unwrap();
                assert!(matches!(executor.execute().unwrap(), HostYield::Return));
                assert_eq!(executor.get_result().unwrap(), &vec![Value(0); expected.len()]);
                executor.call_export("bad", vec![]).unwrap();
                assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));

                let retargeted = body(&format!(
                    "(module (memory 1) (memory 1) (func (local {})
                      i32.const 17 {} {memory} offset=5 local.set 0))",
                    $ty, $instruction,
                ));
                let operators = ops(&retargeted);
                assert_eq!(operators.len(), 2, "{operators:?}");
                assert!(matches!(operators[0],
                    WackasmOperator::$variant { dst: 0, memarg }
                    if memarg.offset == 22 && memarg.memory == memory
                ));
            }
        }};
    }

    check!(
        "i32.load",
        "i32",
        I32LoadAt,
        4,
        vec![Value::from_i32(LOW as i32)]
    );
    check!("i32.load8_u", "i32", I32Load8UAt, 1, vec![Value(128)]);
    check!(
        "i32.load8_s",
        "i32",
        I32Load8SAt,
        1,
        vec![Value::from_i32(-128)]
    );
    check!("i32.load16_u", "i32", I32Load16UAt, 2, vec![Value(0xff80)]);
    check!(
        "i32.load16_s",
        "i32",
        I32Load16SAt,
        2,
        vec![Value::from_i32(-128)]
    );
    check!("i64.load", "i64", I64LoadAt, 8, vec![Value(LOW)]);
    check!("i64.load8_u", "i64", I64Load8UAt, 1, vec![Value(128)]);
    check!(
        "i64.load8_s",
        "i64",
        I64Load8SAt,
        1,
        vec![Value::from_i64(-128)]
    );
    check!("i64.load16_u", "i64", I64Load16UAt, 2, vec![Value(0xff80)]);
    check!(
        "i64.load16_s",
        "i64",
        I64Load16SAt,
        2,
        vec![Value::from_i64(-128)]
    );
    check!(
        "i64.load32_u",
        "i64",
        I64Load32UAt,
        4,
        vec![Value(LOW as u32 as u64)]
    );
    check!(
        "i64.load32_s",
        "i64",
        I64Load32SAt,
        4,
        vec![Value::from_i32(LOW as i32)]
    );
    check!(
        "f32.load",
        "f32",
        F32LoadAt,
        4,
        vec![Value::from_f32(f32::from_bits(LOW as u32))]
    );
    check!(
        "f64.load",
        "f64",
        F64LoadAt,
        8,
        vec![Value::from_f64(f64::from_bits(LOW))]
    );
    check!(
        "v128.load",
        "v128",
        V128LoadAt,
        16,
        vec![Value(LOW), Value(HIGH)]
    );
}

#[test]
fn all_constant_stores_fold_offsets_and_preserve_values_and_atomic_traps() {
    macro_rules! check {
        ($instruction:literal, $ty:literal, $variant:ident, $width:literal, $args:expr) => {{
            let mut memory0_handler = None;
            for memory in [0, 1] {
                let bytes = wat::parse_str(format!(
                    r#"(module (memory 1) (memory 1)
                      (func (export "run") (param {ty})
                        i32.const 17 local.get 0 {instruction} {memory} offset=5)
                      (func (export "last") (param {ty})
                        i32.const {last} local.get 0 {instruction} {memory})
                      (func (export "bad") (param {ty})
                        i32.const {bad} local.get 0 {instruction} {memory}))"#,
                    ty = $ty,
                    instruction = $instruction,
                    last = 65536 - $width,
                    bad = 65537 - $width,
                )).unwrap();
                let module = WackasmModule::new(&bytes).unwrap();
                let operators = ops(first_body(&module));
                assert_eq!(operators.len(), 2, "{operators:?}");
                assert!(matches!(operators[0],
                    WackasmOperator::$variant { value: 0, memarg }
                    if memarg.offset == 22 && memarg.memory == memory
                ));
                let handler = first_body(&module).runtime[0].handler();
                if let Some(zero) = memory0_handler {
                    assert!(!std::ptr::fn_addr_eq(zero, handler));
                } else {
                    memory0_handler = Some(handler);
                }
                check_restoration(&module, |op| {
                    let WackasmOperator::$variant { memarg, .. } = op else { panic!("{op:?}") };
                    memarg.memory = 2;
                });
                let mut executor = WackasmExecutor::new(module).unwrap();
                let args: Vec<Value> = $args;
                let expected = ((LOW as u128) | ((HIGH as u128) << 64)).to_le_bytes();
                executor.call_export("run", args.clone()).unwrap();
                assert!(matches!(executor.execute().unwrap(), HostYield::Return));
                let bytes = executor.memory().bytes(memory).unwrap();
                assert_eq!(&bytes[22..22 + $width], &expected[..$width]);
                assert_eq!(bytes[21], 0);
                assert_eq!(bytes[22 + $width], 0);
                executor.call_export("last", args.clone()).unwrap();
                assert!(matches!(executor.execute().unwrap(), HostYield::Return));
                assert_eq!(
                    &executor.memory().bytes(memory).unwrap()[65536 - $width..],
                    &expected[..$width],
                );
                let before = executor.memory().bytes(memory).unwrap().to_vec();
                executor.call_export("bad", args).unwrap();
                assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
                assert_eq!(executor.memory().bytes(memory).unwrap(), before);
            }
        }};
    }

    check!(
        "i32.store",
        "i32",
        I32StoreAt,
        4,
        vec![Value::from_i32(LOW as i32)]
    );
    check!(
        "i32.store8",
        "i32",
        I32Store8At,
        1,
        vec![Value::from_i32(LOW as i32)]
    );
    check!(
        "i32.store16",
        "i32",
        I32Store16At,
        2,
        vec![Value::from_i32(LOW as i32)]
    );
    check!("i64.store", "i64", I64StoreAt, 8, vec![Value(LOW)]);
    check!("i64.store8", "i64", I64Store8At, 1, vec![Value(LOW)]);
    check!("i64.store16", "i64", I64Store16At, 2, vec![Value(LOW)]);
    check!("i64.store32", "i64", I64Store32At, 4, vec![Value(LOW)]);
    check!(
        "f32.store",
        "f32",
        F32StoreAt,
        4,
        vec![Value::from_f32(f32::from_bits(LOW as u32))]
    );
    check!(
        "f64.store",
        "f64",
        F64StoreAt,
        8,
        vec![Value::from_f64(f64::from_bits(LOW))]
    );
    check!(
        "v128.store",
        "v128",
        V128StoreAt,
        16,
        vec![Value(LOW), Value(HIGH)]
    );
}

#[test]
fn unsigned_constant_addresses_never_wrap_when_combined_with_offsets() {
    for (address, offset) in [
        (-1, 0),
        (0, u32::MAX),
        (1, u32::MAX),
        (-1, 1),
        (-1, u32::MAX),
        (i32::MIN, 0),
        (i32::MIN, 0x8000_0000),
    ] {
        for memory in [0, 1] {
            let bytes = wat::parse_str(format!(
                r#"(module (memory 1) (memory 1)
                  (func (export "read") (result i32)
                    i32.const {address} i32.load {memory} offset={offset})
                  (func (export "write") (param i32)
                    i32.const {address} local.get 0 i32.store {memory} offset={offset}))"#,
            ))
            .unwrap();
            let module = WackasmModule::new(&bytes).unwrap();
            let operators = ops(first_body(&module));
            if let Some(effective) = (address as u32).checked_add(offset) {
                assert_eq!(operators.len(), 2);
                assert!(
                    matches!(operators[0], WackasmOperator::I32LoadAt { memarg, .. }
                    if memarg.offset == effective && memarg.memory == memory)
                );
            } else {
                assert_eq!(operators.len(), 3);
                assert!(
                    matches!(operators[1], WackasmOperator::I32Load { memarg, .. }
                    if memarg.offset == offset && memarg.memory == memory)
                );
            }
            let WackasmFunction::Internal(store) = &module.functions.function_bodies[1] else {
                panic!("expected store function")
            };
            let stores = ops(store);
            if let Some(effective) = (address as u32).checked_add(offset) {
                assert_eq!(stores.len(), 2);
                assert!(
                    matches!(stores[0], WackasmOperator::I32StoreAt { memarg, .. }
                    if memarg.offset == effective && memarg.memory == memory)
                );
            } else {
                assert_eq!(stores.len(), 3);
                assert!(matches!(stores[1], WackasmOperator::I32Store { memarg, .. }
                    if memarg.offset == offset && memarg.memory == memory));
            }
            let mut executor = WackasmExecutor::new(module).unwrap();
            executor.call_export("read", vec![]).unwrap();
            assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
            let before = executor.memory().bytes(memory).unwrap().to_vec();
            executor.call_export("write", vec![Value(123)]).unwrap();
            assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
            assert_eq!(executor.memory().bytes(memory).unwrap(), before);
        }
    }
}

#[test]
fn dynamic_addresses_keep_register_based_operations() {
    let load = body(
        "(module (memory 1) (func (param i32) (result i64)
        local.get 0 i64.load offset=5))",
    );
    assert!(matches!(ops(&load).as_slice(), [
        WackasmOperator::I64Load { dst: 1, addr: 0, memarg }, WackasmOperator::Return { .. }
    ] if memarg.offset == 5));
    let store = body(
        "(module (memory 1) (func (param i32 i64)
        local.get 0 local.get 1 i64.store offset=5))",
    );
    assert!(matches!(ops(&store).as_slice(), [
        WackasmOperator::I64Store { addr: 0, value: 1, memarg }, WackasmOperator::Return { .. }
    ] if memarg.offset == 5));
}

#[test]
fn constant_addresses_are_checked_again_after_memory_growth() {
    for memory in [0, 1] {
        let bytes = wat::parse_str(format!(
            r#"(module (memory 0 1) (memory 0 1)
              (func (export "read") (result i32) i32.const 0 i32.load {memory})
              (func (export "grow") (result i32)
                i32.const 1 memory.grow {memory} drop
                i32.const 0 i32.const 123 i32.store {memory}
                i32.const 0 i32.load {memory}))"#,
        ))
        .unwrap();
        let mut executor = WackasmExecutor::new(WackasmModule::new(&bytes).unwrap()).unwrap();
        executor.call_export("read", vec![]).unwrap();
        assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
        executor.call_export("grow", vec![]).unwrap();
        assert!(matches!(executor.execute().unwrap(), HostYield::Return));
        assert_eq!(executor.get_result().unwrap(), &vec![Value(123)]);
    }
}
