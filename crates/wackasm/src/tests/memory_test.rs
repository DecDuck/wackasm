use crate::{HaltError, HostYield, Value, WackasmExecutor, WackasmModule};

fn executor(wat: &str) -> WackasmExecutor {
    let bytes = wat::parse_str(wat).unwrap();
    WackasmExecutor::new(WackasmModule::new(&bytes).unwrap()).unwrap()
}

fn restore(executor: WackasmExecutor) -> WackasmExecutor {
    let bytes = bitcode::serialize(&executor).unwrap();
    drop(executor);
    bitcode::deserialize(&bytes).unwrap()
}

#[test]
fn specialized_and_indexed_handlers_preserve_unaligned_scalar_and_wide_values() {
    for mem in [0, 1] {
        let mut executor = executor(&format!(
            r#"(module (memory 1) (memory 1)
              (func (export "run") (param v128) (result i32 i64 f32 f64 v128)
                i32.const 1 i32.const -12345 i32.store {mem}
                i32.const 5 i64.const -81985529216486895 i64.store {mem}
                i32.const 13 f32.const -1.5 f32.store {mem}
                i32.const 17 f64.const -0 f64.store {mem}
                i32.const 25 local.get 0 v128.store {mem}
                i32.const 1 i32.load {mem}
                i32.const 5 i64.load {mem}
                i32.const 13 f32.load {mem}
                i32.const 17 f64.load {mem}
                i32.const 25 v128.load {mem}))"#
        ));
        executor
            .call_export("run", vec![Value(13), Value(29)])
            .unwrap();
        assert!(matches!(executor.execute().unwrap(), HostYield::Return));
        assert_eq!(
            executor.get_result().unwrap(),
            &vec![
                Value::from_i32(-12345),
                Value::from_i64(-81985529216486895),
                Value::from_f32(-1.5),
                Value::from_f64(-0.0),
                Value(13),
                Value(29),
            ],
        );
    }
}

#[test]
fn effective_addresses_do_not_wrap_and_store_traps_are_atomic() {
    for mem in [0, 1] {
        let mut executor = executor(&format!(
            r#"(module (memory 1) (memory 1)
              (func (export "store") (param i32)
                local.get 0 i64.const -1 i64.store {mem})
              (func (export "overflow") (param i32) (result i32)
                local.get 0 i32.load {mem} offset=4294967295)
              (func (export "last") (result i64)
                i32.const 65528 i64.load {mem}))"#
        ));
        executor.call_export("store", vec![Value(65528)]).unwrap();
        assert!(matches!(executor.execute().unwrap(), HostYield::Return));
        let before = executor.memory().bytes(mem).unwrap().to_vec();
        executor.call_export("store", vec![Value(65529)]).unwrap();
        assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
        assert_eq!(executor.memory().bytes(mem).unwrap(), before);
        executor.call_export("overflow", vec![Value(1)]).unwrap();
        assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
        executor
            .call_export("overflow", vec![Value::from_i32(-1)])
            .unwrap();
        assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
        executor.call_export("last", vec![]).unwrap();
        assert!(matches!(executor.execute().unwrap(), HostYield::Return));
        assert_eq!(executor.get_result().unwrap(), &vec![Value::from_i64(-1)]);
    }
}

#[test]
fn memory0_cache_is_refreshed_after_internal_growth_and_restoration() {
    let mut executor = executor(
        r#"(module (memory 0 2)
          (func $grow (result i32) i32.const 1 memory.grow)
          (func (export "run") (result i32 i32)
            call $grow
            i32.const 65535 i32.const 123 i32.store8
            (loop)
            i32.const 65535 i32.load8_u))"#,
    );
    executor.call_export("run", vec![]).unwrap();
    executor.set_fuel(1);
    assert!(matches!(executor.execute().unwrap(), HostYield::OutOfFuel));
    executor = restore(executor);
    executor.set_fuel(1);
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(executor.get_result().unwrap(), &vec![Value(0), Value(123)]);
}

#[test]
fn growing_other_memories_or_failing_growth_does_not_stale_memory0() {
    let mut executor = executor(
        r#"(module (memory 1 1) (memory 0 1)
          (func (export "run") (result i32 i32 i32 i32)
            i32.const 0 i32.const 42 i32.store
            i32.const 1 memory.grow 1
            i32.const 0 i32.load
            i32.const 65532 i32.const 99 i32.store 1
            i32.const 1 memory.grow 0
            i32.const 65532 i32.load 1))"#,
    );
    executor.call_export("run", vec![]).unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(
        executor.get_result().unwrap(),
        &vec![Value(0), Value(42), Value::from_i32(-1), Value(99)],
    );
}

#[test]
fn host_yields_rebuild_memory0_cache_on_resume() {
    let mut executor = executor(
        r#"(module
          (import "host" "pause" (func $pause))
          (memory 0 1)
          (func (export "run") (result i32)
            i32.const 1 memory.grow drop
            i32.const 65532 i32.const 42 i32.store
            call $pause
            i32.const 65532 i32.load))"#,
    );
    executor.call_export("run", vec![]).unwrap();
    assert!(matches!(
        executor.execute().unwrap(),
        HostYield::ImportCall { .. }
    ));
    executor = restore(executor);
    executor.import_return(&[]).unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(executor.get_result().unwrap(), &vec![Value(42)]);
}

#[test]
fn bulk_memory_handles_overlap_cross_memory_and_zero_length_boundaries() {
    let mut executor = executor(
        r#"(module (memory 1) (memory 1)
          (data (i32.const 0) "\00\01\02\03\04\05\06\07")
          (func (export "run")
            i32.const 2 i32.const 0 i32.const 6 memory.copy 0 0
            i32.const 0 i32.const 0 i32.const 8 memory.copy 1 0
            i32.const 8 i32.const 255 i32.const 2 memory.fill 1
            i32.const 65536 i32.const 65536 i32.const 0 memory.copy 1 0
            i32.const 65536 i32.const 0 i32.const 0 memory.fill 0)
          (func (export "bad-copy")
            i32.const 0 i32.const 65535 i32.const 2 memory.copy 1 0)
          (func (export "bad-fill")
            i32.const 65535 i32.const 0 i32.const 2 memory.fill 1))"#,
    );
    executor.call_export("run", vec![]).unwrap();
    assert!(matches!(executor.execute().unwrap(), HostYield::Return));
    assert_eq!(
        executor.memory().read_bytes(0, 0, 8).unwrap(),
        &[0, 1, 0, 1, 2, 3, 4, 5]
    );
    assert_eq!(
        executor.memory().read_bytes(1, 0, 10).unwrap(),
        &[0, 1, 0, 1, 2, 3, 4, 5, 255, 255]
    );
    let before = executor.memory().bytes(1).unwrap().to_vec();
    for name in ["bad-copy", "bad-fill"] {
        executor.call_export(name, vec![]).unwrap();
        assert!(matches!(executor.execute(), Err(HaltError::Trap(_))));
        assert_eq!(executor.memory().bytes(1).unwrap(), before);
    }
}

#[test]
fn validation_rejects_unknown_memories_before_unchecked_dispatch() {
    for body in [
        "i32.const 0 i32.load 1 drop",
        "i32.const 0 i32.const 0 i32.store 1",
    ] {
        let wasm = wat::parse_str(format!("(module (memory 1) (func {body}))")).unwrap();
        assert!(WackasmModule::new(&wasm).is_err());
    }
}

#[test]
fn restoration_revalidates_indexes_before_unchecked_dispatch() {
    use crate::{compiler::WackasmOperator, functions::WackasmFunction};

    for (instruction, corrupt_destination) in [
        ("i32.const 0 i32.load drop", false),
        ("i32.const 0 i32.const 0 i32.store", false),
        ("i32.const 0 memory.grow drop", false),
        ("i32.const 0 i32.const 0 i32.const 0 memory.fill", false),
        ("i32.const 0 i32.const 0 i32.const 0 memory.copy", false),
        ("i32.const 0 i32.const 0 i32.const 0 memory.copy", true),
    ] {
        let bytes = wat::parse_str(format!("(module (memory 1) (func {instruction}))")).unwrap();
        let mut module = WackasmModule::new(&bytes).unwrap();
        for function in &mut module.functions.function_bodies {
            let WackasmFunction::Internal(body) = function else {
                continue;
            };
            for operator in &mut body.runtime {
                let mut op = operator.op();
                match &mut op {
                    WackasmOperator::I32Load { memarg, .. }
                    | WackasmOperator::I32Store { memarg, .. }
                    | WackasmOperator::I32LoadAt { memarg, .. }
                    | WackasmOperator::I32StoreAt { memarg, .. } => memarg.memory = 1,
                    WackasmOperator::MemoryGrow { mem, .. }
                    | WackasmOperator::MemoryFill { mem, .. } => *mem = 1,
                    WackasmOperator::MemoryCopy {
                        src_mem, dst_mem, ..
                    } => {
                        if corrupt_destination {
                            *dst_mem = 1
                        } else {
                            *src_mem = 1
                        };
                    }
                    _ => {}
                }
                *operator = op.into();
            }
        }
        let serialized = bitcode::serialize(&module).unwrap();
        assert!(bitcode::deserialize::<WackasmModule>(&serialized).is_err());
    }
}

#[test]
fn memory0_selects_specialized_handlers_without_changing_the_serialized_ir() {
    use crate::compiler::{RuntimeWackasmOperator, WackasmMemArg, WackasmOperator};

    for operation in [
        WackasmOperator::I32Load {
            dst: 0,
            addr: 0,
            memarg: WackasmMemArg {
                memory: 0,
                offset: 0,
            },
        },
        WackasmOperator::F64Store {
            addr: 0,
            value: 0,
            memarg: WackasmMemArg {
                memory: 0,
                offset: 0,
            },
        },
        WackasmOperator::V128Load {
            dst: 0,
            addr: 0,
            memarg: WackasmMemArg {
                memory: 0,
                offset: 0,
            },
        },
    ] {
        let zero = RuntimeWackasmOperator::from(operation.clone());
        let mut indexed = operation;
        match &mut indexed {
            WackasmOperator::I32Load { memarg, .. }
            | WackasmOperator::F64Store { memarg, .. }
            | WackasmOperator::V128Load { memarg, .. } => memarg.memory = 1,
            _ => unreachable!(),
        }
        let indexed = RuntimeWackasmOperator::from(indexed);
        assert!(!std::ptr::fn_addr_eq(zero.handler(), indexed.handler()));
        let bytes = bitcode::serialize(&zero).unwrap();
        let restored: RuntimeWackasmOperator = bitcode::deserialize(&bytes).unwrap();
        assert!(std::ptr::fn_addr_eq(zero.handler(), restored.handler()));
    }
}
