use wasmparser::{RefType, ValType};

use crate::{
    HaltError, Value,
    module::{BlockIndex, FunctionIndex},
    stack::{
        BlockAwareStack, MAX_OPERAND_STACK_SIZE, block::BlockAwareOperandStack,
        flat::FlatBlockAwareStack,
    },
};

const TEST_BLOCKS: usize = 8;

/// Lets the generic tests construct any `BlockAwareStack` implementation.
trait TestStack: BlockAwareStack {
    /// `blocks` is the number of blocks in the function, as passed by `WackasmModule`.
    fn with_blocks(blocks: usize) -> Self;

    fn create() -> Self
    where
        Self: Sized,
    {
        Self::with_blocks(TEST_BLOCKS)
    }
}

impl TestStack for BlockAwareOperandStack {
    fn with_blocks(blocks: usize) -> Self {
        BlockAwareOperandStack::new(blocks)
    }
}

impl TestStack for FlatBlockAwareStack {
    fn with_blocks(blocks: usize) -> Self {
        FlatBlockAwareStack::new(blocks)
    }
}

fn value_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::I32(a), Value::I32(b)) => a == b,
        (Value::I64(a), Value::I64(b)) => a == b,
        (Value::F32(a), Value::F32(b)) => a.to_bits() == b.to_bits(),
        (Value::F64(a), Value::F64(b)) => a.to_bits() == b.to_bits(),
        (Value::V128(a), Value::V128(b)) => a == b,
        (Value::FuncRef(a), Value::FuncRef(b)) => a == b,
        (Value::Unknown, Value::Unknown) => true,
        _ => false,
    }
}

#[track_caller]
fn assert_values(actual: &[Value], expected: &[Value]) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "length mismatch: {actual:?} vs {expected:?}"
    );
    for (a, e) in actual.iter().zip(expected) {
        assert!(value_eq(a, e), "value mismatch: {actual:?} vs {expected:?}");
    }
}

#[track_caller]
fn assert_pop<S: TestStack>(stack: &mut S, expected: Value) {
    let actual = stack.pop().expect("pop should succeed");
    assert!(
        value_eq(&actual, &expected),
        "expected {expected:?}, got {actual:?}"
    );
}

#[track_caller]
fn assert_trap<T: std::fmt::Debug>(result: Result<T, HaltError>) {
    assert!(result.is_err(), "expected an error, got {result:?}");
}

#[track_caller]
fn assert_empty<S: TestStack>(stack: &mut S) {
    assert_trap(stack.pop());
}

#[track_caller]
fn return_ok<S: TestStack>(stack: &mut S, preserve: &[ValType]) -> BlockIndex {
    stack
        .return_block(preserve)
        .expect("return_block should succeed")
        .expect("a block should be open")
}

fn i32s(values: &[i32]) -> Vec<Value> {
    values.iter().copied().map(Value::I32).collect()
}

// ---------------------------------------------------------------------------
// Basic operand stack behaviour
// ---------------------------------------------------------------------------

fn pop_empty_errors<S: TestStack>() {
    let mut stack = S::create();
    assert_trap(stack.pop());
}

fn peek_empty_errors<S: TestStack>() {
    let mut stack = S::create();
    assert_trap(stack.peek().map(|v| *v));
}

fn push_pop_single<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(42)).unwrap();
    assert_pop(&mut stack, Value::I32(42));
    assert_empty(&mut stack);
}

fn push_pop_is_lifo<S: TestStack>() {
    let mut stack = S::create();
    for v in 0..10 {
        stack.push(Value::I32(v)).unwrap();
    }
    for v in (0..10).rev() {
        assert_pop(&mut stack, Value::I32(v));
    }
    assert_empty(&mut stack);
}

fn push_pop_all_value_types<S: TestStack>() {
    let mut stack = S::create();
    let values = [
        Value::I32(-1),
        Value::I64(i64::MIN),
        Value::F32(1.5),
        Value::F64(-2.25),
        Value::V128(u128::MAX),
        Value::FuncRef(FunctionIndex(7)),
    ];
    for v in values {
        stack.push(v).unwrap();
    }
    for v in values.iter().rev() {
        assert_pop(&mut stack, *v);
    }
    assert_empty(&mut stack);
}

fn peek_does_not_remove<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push(Value::I32(2)).unwrap();

    assert!(value_eq(stack.peek().unwrap(), &Value::I32(2)));
    assert!(value_eq(stack.peek().unwrap(), &Value::I32(2)));

    assert_pop(&mut stack, Value::I32(2));
    assert!(value_eq(stack.peek().unwrap(), &Value::I32(1)));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn typed_pops_succeed<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push(Value::I64(2)).unwrap();
    stack.push(Value::F32(3.0)).unwrap();
    stack.push(Value::F64(4.0)).unwrap();
    stack.push(Value::V128(5)).unwrap();

    assert_eq!(stack.pop_v128().unwrap(), 5);
    assert_eq!(stack.pop_f64().unwrap(), 4.0);
    assert_eq!(stack.pop_f32().unwrap(), 3.0);
    assert_eq!(stack.pop_i64().unwrap(), 2);
    assert_eq!(stack.pop_i32().unwrap(), 1);
    assert_empty(&mut stack);
}

fn typed_pops_reject_wrong_type<S: TestStack>() {
    let mut stack = S::create();

    stack.push(Value::I64(1)).unwrap();
    assert_trap(stack.pop_i32());

    stack.push(Value::I32(1)).unwrap();
    assert_trap(stack.pop_i64());

    stack.push(Value::F64(1.0)).unwrap();
    assert_trap(stack.pop_f32());

    stack.push(Value::F32(1.0)).unwrap();
    assert_trap(stack.pop_f64());

    stack.push(Value::I32(1)).unwrap();
    assert_trap(stack.pop_v128());
}

fn typed_pops_on_empty_error<S: TestStack>() {
    let mut stack = S::create();
    assert_trap(stack.pop_i32());
    assert_trap(stack.pop_i64());
    assert_trap(stack.pop_f32());
    assert_trap(stack.pop_f64());
    assert_trap(stack.pop_v128());
}

fn stack_is_reusable_after_draining<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);

    stack.push(Value::I32(2)).unwrap();
    assert_pop(&mut stack, Value::I32(2));
    assert_empty(&mut stack);
}

// ---------------------------------------------------------------------------
// push_reversed_all / pull_reversed_from_stack
// ---------------------------------------------------------------------------

fn push_reversed_all_matches_individual_pushes<S: TestStack>() {
    // [r1, r2, r3] goes onto the stack as [..., r3, r2, r1].
    let mut stack = S::create();
    stack.push_reversed_all(i32s(&[1, 2, 3])).unwrap();

    assert_pop(&mut stack, Value::I32(1));
    assert_pop(&mut stack, Value::I32(2));
    assert_pop(&mut stack, Value::I32(3));
    assert_empty(&mut stack);
}

fn push_reversed_all_on_top_of_existing<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(100)).unwrap();
    stack.push_reversed_all(i32s(&[1, 2])).unwrap();

    assert_pop(&mut stack, Value::I32(1));
    assert_pop(&mut stack, Value::I32(2));
    assert_pop(&mut stack, Value::I32(100));
    assert_empty(&mut stack);
}

fn push_reversed_all_empty_is_noop<S: TestStack>() {
    let mut stack = S::create();
    stack.push_reversed_all(Vec::new()).unwrap();
    assert_empty(&mut stack);

    stack.push(Value::I32(5)).unwrap();
    stack.push_reversed_all(Vec::new()).unwrap();
    assert_pop(&mut stack, Value::I32(5));
    assert_empty(&mut stack);
}

fn pull_reversed_from_stack_returns_top_first<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push(Value::I64(2)).unwrap();
    stack.push(Value::F32(3.0)).unwrap();

    let values = stack
        .pull_reversed_from_stack(&[ValType::I32, ValType::I64, ValType::F32])
        .unwrap();
    assert_values(&values, &[Value::F32(3.0), Value::I64(2), Value::I32(1)]);
    assert_empty(&mut stack);
}

fn pull_reversed_from_stack_leaves_remaining<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push(Value::I32(2)).unwrap();
    stack.push(Value::I32(3)).unwrap();

    let values = stack.pull_reversed_from_stack(&[ValType::I32]).unwrap();
    assert_values(&values, &[Value::I32(3)]);

    assert_pop(&mut stack, Value::I32(2));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn pull_reversed_from_stack_empty_signature<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();

    let values = stack.pull_reversed_from_stack(&[]).unwrap();
    assert!(values.is_empty());
    assert_pop(&mut stack, Value::I32(1));
}

fn pull_reversed_from_stack_all_types<S: TestStack>() {
    let mut stack = S::create();
    let values = [
        Value::I32(1),
        Value::I64(2),
        Value::F32(3.0),
        Value::F64(4.0),
        Value::V128(5),
        Value::FuncRef(FunctionIndex(6)),
    ];
    for v in values {
        stack.push(v).unwrap();
    }

    let pulled = stack
        .pull_reversed_from_stack(&[
            ValType::I32,
            ValType::I64,
            ValType::F32,
            ValType::F64,
            ValType::V128,
            ValType::Ref(RefType::FUNCREF),
        ])
        .unwrap();

    let expected: Vec<Value> = values.iter().rev().copied().collect();
    assert_values(&pulled, &expected);
    assert_empty(&mut stack);
}

fn pull_reversed_from_stack_type_mismatch_errors<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I64(1)).unwrap();
    assert_trap(stack.pull_reversed_from_stack(&[ValType::I32]));
}

fn pull_reversed_from_stack_underflow_errors<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    assert_trap(stack.pull_reversed_from_stack(&[ValType::I32, ValType::I32]));
}

fn pull_then_push_reversed_round_trips<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push(Value::F64(2.0)).unwrap();
    stack.push(Value::I64(3)).unwrap();

    let pulled = stack
        .pull_reversed_from_stack(&[ValType::I32, ValType::F64, ValType::I64])
        .unwrap();
    assert_empty(&mut stack);

    stack.push_reversed_all(pulled).unwrap();
    assert_pop(&mut stack, Value::I64(3));
    assert_pop(&mut stack, Value::F64(2.0));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

// ---------------------------------------------------------------------------
// Block behaviour
// ---------------------------------------------------------------------------

fn peek_block_empty_is_none<S: TestStack>() {
    let stack = S::create();
    assert!(stack.peek_block().is_none());
}

fn return_block_without_block_is_none<S: TestStack>() {
    let mut stack = S::create();
    assert_eq!(stack.return_block(&[]).unwrap(), None);
}

fn return_block_without_block_leaves_base_untouched<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    assert_eq!(stack.return_block(&[ValType::I32]).unwrap(), None);
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn push_block_sets_peek_block<S: TestStack>() {
    let mut stack = S::create();
    stack.push_block(BlockIndex(3), None).unwrap();
    assert_eq!(stack.peek_block(), Some(&BlockIndex(3)));
}

fn peek_block_returns_innermost<S: TestStack>() {
    let mut stack = S::create();
    stack.push_block(BlockIndex(1), None).unwrap();
    stack.push_block(BlockIndex(2), None).unwrap();
    stack.push_block(BlockIndex(3), None).unwrap();
    assert_eq!(stack.peek_block(), Some(&BlockIndex(3)));

    assert_eq!(return_ok(&mut stack, &[]), BlockIndex(3));
    assert_eq!(stack.peek_block(), Some(&BlockIndex(2)));

    assert_eq!(return_ok(&mut stack, &[]), BlockIndex(2));
    assert_eq!(stack.peek_block(), Some(&BlockIndex(1)));

    assert_eq!(return_ok(&mut stack, &[]), BlockIndex(1));
    assert!(stack.peek_block().is_none());
    assert_eq!(stack.return_block(&[]).unwrap(), None);
}

fn new_block_starts_empty<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    assert_empty(&mut stack);
}

fn block_cannot_peek_outer_values<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    assert_trap(stack.peek().map(|v| *v));
}

fn block_cannot_pull_outer_values<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    stack.push(Value::I32(2)).unwrap();
    assert_trap(stack.pull_reversed_from_stack(&[ValType::I32, ValType::I32]));
}

fn block_pops_down_to_floor_only<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    stack.push(Value::I32(2)).unwrap();
    stack.push(Value::I32(3)).unwrap();

    assert_pop(&mut stack, Value::I32(3));
    assert_pop(&mut stack, Value::I32(2));
    assert_empty(&mut stack);
}

fn push_block_with_parameters<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(100)).unwrap();
    // Parameters are given in reversed order, as from `pull_reversed_from_stack`.
    stack
        .push_block(BlockIndex(0), Some(vec![Value::I64(2), Value::I32(1)]))
        .unwrap();

    assert_pop(&mut stack, Value::I64(2));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn push_block_with_empty_parameters<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(100)).unwrap();
    stack.push_block(BlockIndex(0), Some(Vec::new())).unwrap();
    assert_empty(&mut stack);
    assert_eq!(stack.peek_block(), Some(&BlockIndex(0)));
}

fn block_parameters_from_pull_preserve_order<S: TestStack>() {
    // Simulates the executor moving block params from the parent into a block.
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push(Value::I64(2)).unwrap();

    let params = stack
        .pull_reversed_from_stack(&[ValType::I32, ValType::I64])
        .unwrap();
    stack.push_block(BlockIndex(0), Some(params)).unwrap();

    assert_eq!(stack.pop_i64().unwrap(), 2);
    assert_eq!(stack.pop_i32().unwrap(), 1);
    assert_empty(&mut stack);
}

fn return_block_without_preserve_discards_values<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(5), None).unwrap();
    stack.push(Value::I32(2)).unwrap();
    stack.push(Value::I32(3)).unwrap();

    assert_eq!(return_ok(&mut stack, &[]), BlockIndex(5));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn return_block_preserves_single_value<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    stack.push(Value::I64(42)).unwrap();

    assert_eq!(
        return_ok(&mut stack, &[ValType::I64]),
        BlockIndex(0)
    );
    assert_pop(&mut stack, Value::I64(42));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn return_block_preserves_multiple_values_in_order<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    stack.push(Value::I32(10)).unwrap();
    stack.push(Value::F32(20.0)).unwrap();
    stack.push(Value::I64(30)).unwrap();

    return_ok(&mut stack, &[ValType::I32, ValType::F32, ValType::I64]);

    assert_pop(&mut stack, Value::I64(30));
    assert_pop(&mut stack, Value::F32(20.0));
    assert_pop(&mut stack, Value::I32(10));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn return_block_discards_values_below_preserved<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    stack.push(Value::I32(97)).unwrap();
    stack.push(Value::I32(98)).unwrap();
    stack.push(Value::I32(99)).unwrap();

    return_ok(&mut stack, &[ValType::I32]);

    assert_pop(&mut stack, Value::I32(99));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn return_block_preserve_type_mismatch_errors<S: TestStack>() {
    let mut stack = S::create();
    stack.push_block(BlockIndex(0), None).unwrap();
    stack.push(Value::I32(1)).unwrap();
    assert_trap(stack.return_block(&[ValType::I64]));
}

fn return_block_preserve_underflow_errors<S: TestStack>() {
    // The block has fewer values than requested; it must not reach into the parent.
    let mut stack = S::create();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    stack.push(Value::I32(2)).unwrap();
    assert_trap(stack.return_block(&[ValType::I32, ValType::I32]));
}

fn nested_blocks_are_isolated<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(0)).unwrap();
    stack.push_block(BlockIndex(1), None).unwrap();
    stack.push(Value::I32(1)).unwrap();
    stack.push_block(BlockIndex(2), None).unwrap();
    stack.push(Value::I32(2)).unwrap();

    assert_pop(&mut stack, Value::I32(2));
    assert_empty(&mut stack);

    assert_eq!(return_ok(&mut stack, &[]), BlockIndex(2));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);

    assert_eq!(return_ok(&mut stack, &[]), BlockIndex(1));
    assert_pop(&mut stack, Value::I32(0));
    assert_empty(&mut stack);
}

fn nested_blocks_propagate_preserved_values<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(0)).unwrap();
    stack.push_block(BlockIndex(1), None).unwrap();
    stack.push(Value::I32(10)).unwrap();
    stack.push_block(BlockIndex(2), None).unwrap();
    stack.push(Value::I32(20)).unwrap();
    stack.push(Value::I32(21)).unwrap();

    // Inner block yields two values into the middle block.
    return_ok(&mut stack, &[ValType::I32, ValType::I32]);
    assert_eq!(stack.peek_block(), Some(&BlockIndex(1)));

    // Middle block now holds [10, 20, 21]; yield the top two into the base.
    return_ok(&mut stack, &[ValType::I32, ValType::I32]);
    assert!(stack.peek_block().is_none());

    assert_pop(&mut stack, Value::I32(21));
    assert_pop(&mut stack, Value::I32(20));
    assert_pop(&mut stack, Value::I32(0));
    assert_empty(&mut stack);
}

fn block_with_parameters_and_results<S: TestStack>() {
    // (i32 i32) -> (i32) block that adds its params.
    let mut stack = S::create();
    stack.push(Value::I32(7)).unwrap();
    stack.push(Value::I32(3)).unwrap();
    stack.push(Value::I32(4)).unwrap();

    let params = stack
        .pull_reversed_from_stack(&[ValType::I32, ValType::I32])
        .unwrap();
    stack.push_block(BlockIndex(9), Some(params)).unwrap();

    let b = stack.pop_i32().unwrap();
    let a = stack.pop_i32().unwrap();
    assert_eq!((a, b), (3, 4));
    stack.push(Value::I32(a + b)).unwrap();

    assert_eq!(
        return_ok(&mut stack, &[ValType::I32]),
        BlockIndex(9)
    );
    assert_pop(&mut stack, Value::I32(7));
    assert_pop(&mut stack, Value::I32(7));
    assert_empty(&mut stack);
}

fn reentered_block_starts_empty<S: TestStack>() {
    let mut stack = S::create();
    stack.push_block(BlockIndex(0), None).unwrap();
    stack.push(Value::I32(1)).unwrap();
    stack.push(Value::I32(2)).unwrap();
    return_ok(&mut stack, &[]);

    stack.push_block(BlockIndex(0), None).unwrap();
    assert_empty(&mut stack);
}

fn many_sequential_blocks<S: TestStack>() {
    let mut stack = S::create();
    for i in 0..(TEST_BLOCKS as u32 * 4) {
        stack.push_block(BlockIndex(i), None).unwrap();
        stack.push(Value::I32(i as i32)).unwrap();
        assert_eq!(
            return_ok(&mut stack, &[ValType::I32]),
            BlockIndex(i)
        );
    }
    for i in (0..(TEST_BLOCKS as i32 * 4)).rev() {
        assert_pop(&mut stack, Value::I32(i));
    }
    assert_empty(&mut stack);
}

fn deeply_nested_blocks<S: TestStack>() {
    let mut stack = S::create();
    let depth = TEST_BLOCKS as u32 - 1;
    for i in 0..depth {
        stack.push(Value::I32(i as i32)).unwrap();
        stack.push_block(BlockIndex(i), None).unwrap();
    }
    for i in (0..depth).rev() {
        assert_empty(&mut stack);
        assert_eq!(return_ok(&mut stack, &[]), BlockIndex(i));
        assert_pop(&mut stack, Value::I32(i as i32));
    }
    assert!(stack.peek_block().is_none());
    assert_empty(&mut stack);
}

fn block_can_use_full_stack_size<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::I32(-1)).unwrap();
    stack.push_block(BlockIndex(0), None).unwrap();
    for i in 0..(MAX_OPERAND_STACK_SIZE - 1) {
        stack.push(Value::I32(i as i32)).unwrap();
    }
    return_ok(&mut stack, &[]);
    assert_pop(&mut stack, Value::I32(-1));
    assert_empty(&mut stack);
}

// ---------------------------------------------------------------------------
// Sizing: stacks built for the exact number of blocks a function has
// ---------------------------------------------------------------------------

fn zero_blocks_push_pop<S: TestStack>() {
    // Functions without blocks (and the placeholder frame) are created with 0 blocks.
    let mut stack = S::with_blocks(0);
    stack.push(Value::I32(1)).unwrap();
    stack.push(Value::I64(2)).unwrap();
    assert!(value_eq(stack.peek().unwrap(), &Value::I64(2)));
    assert_pop(&mut stack, Value::I64(2));
    assert_pop(&mut stack, Value::I32(1));
    assert_empty(&mut stack);
}

fn zero_blocks_bulk_operations<S: TestStack>() {
    let mut stack = S::with_blocks(0);
    stack.push_reversed_all(i32s(&[1, 2, 3])).unwrap();
    let pulled = stack
        .pull_reversed_from_stack(&[ValType::I32, ValType::I32, ValType::I32])
        .unwrap();
    assert_values(&pulled, &i32s(&[1, 2, 3]));
    assert_empty(&mut stack);
}

fn zero_blocks_push_until_overflow<S: TestStack>() {
    let mut stack = S::with_blocks(0);
    for i in 0..MAX_OPERAND_STACK_SIZE {
        stack.push(Value::I32(i as i32)).unwrap();
    }
    assert_trap(stack.push(Value::I32(-1)));
    assert_pop(&mut stack, Value::I32(MAX_OPERAND_STACK_SIZE as i32 - 1));
}

fn zero_blocks_push_reversed_all_until_overflow<S: TestStack>() {
    let mut stack = S::with_blocks(0);
    stack
        .push_reversed_all(vec![Value::I32(0); MAX_OPERAND_STACK_SIZE])
        .unwrap();
    assert_trap(stack.push(Value::I32(-1)));
    assert_trap(stack.push_reversed_all(vec![Value::I32(-1)]));
}

fn exact_block_count_nesting<S: TestStack>() {
    for blocks in 1..=4 {
        let mut stack = S::with_blocks(blocks);
        stack.push(Value::I32(100)).unwrap();

        for depth in 0..blocks {
            stack.push_block(BlockIndex(depth as u32), None).unwrap();
            stack.push(Value::I32(depth as i32)).unwrap();
        }

        // Each block yields its top value into its parent, discarding the parent's own.
        for depth in (0..blocks).rev() {
            assert_eq!(
                return_ok(&mut stack, &[ValType::I32]),
                BlockIndex(depth as u32)
            );
        }

        assert!(stack.peek_block().is_none());
        assert_pop(&mut stack, Value::I32(blocks as i32 - 1));
        assert_pop(&mut stack, Value::I32(100));
        assert_empty(&mut stack);
    }
}

fn exact_block_count_with_values_in_every_frame<S: TestStack>() {
    let per_frame = MAX_OPERAND_STACK_SIZE / 4;
    let mut stack = S::with_blocks(1);

    for i in 0..per_frame {
        stack.push(Value::I32(i as i32)).unwrap();
    }
    stack.push_block(BlockIndex(0), None).unwrap();
    for i in 0..per_frame {
        stack.push(Value::I64(i as i64)).unwrap();
    }

    return_ok(&mut stack, &[ValType::I64]);
    assert_pop(&mut stack, Value::I64(per_frame as i64 - 1));
    for i in (0..per_frame).rev() {
        assert_pop(&mut stack, Value::I32(i as i32));
    }
    assert_empty(&mut stack);
}

// ---------------------------------------------------------------------------
// Test instantiation
// ---------------------------------------------------------------------------

/// Instantiates every generic test above as a `#[test]` for a concrete stack type.
macro_rules! stack_tests {
    ($module:ident, $ty:ty) => {
        mod $module {
            use super::*;

            stack_tests!(@tests $ty;
                pop_empty_errors,
                peek_empty_errors,
                push_pop_single,
                push_pop_is_lifo,
                push_pop_all_value_types,
                peek_does_not_remove,
                typed_pops_succeed,
                typed_pops_reject_wrong_type,
                typed_pops_on_empty_error,
                stack_is_reusable_after_draining,
                push_reversed_all_matches_individual_pushes,
                push_reversed_all_on_top_of_existing,
                push_reversed_all_empty_is_noop,
                pull_reversed_from_stack_returns_top_first,
                pull_reversed_from_stack_leaves_remaining,
                pull_reversed_from_stack_empty_signature,
                pull_reversed_from_stack_all_types,
                pull_reversed_from_stack_type_mismatch_errors,
                pull_reversed_from_stack_underflow_errors,
                pull_then_push_reversed_round_trips,
                peek_block_empty_is_none,
                return_block_without_block_is_none,
                return_block_without_block_leaves_base_untouched,
                push_block_sets_peek_block,
                peek_block_returns_innermost,
                new_block_starts_empty,
                block_cannot_peek_outer_values,
                block_cannot_pull_outer_values,
                block_pops_down_to_floor_only,
                push_block_with_parameters,
                push_block_with_empty_parameters,
                block_parameters_from_pull_preserve_order,
                return_block_without_preserve_discards_values,
                return_block_preserves_single_value,
                return_block_preserves_multiple_values_in_order,
                return_block_discards_values_below_preserved,
                return_block_preserve_type_mismatch_errors,
                return_block_preserve_underflow_errors,
                nested_blocks_are_isolated,
                nested_blocks_propagate_preserved_values,
                block_with_parameters_and_results,
                reentered_block_starts_empty,
                many_sequential_blocks,
                deeply_nested_blocks,
                block_can_use_full_stack_size,
                zero_blocks_push_pop,
                zero_blocks_bulk_operations,
                zero_blocks_push_until_overflow,
                zero_blocks_push_reversed_all_until_overflow,
                exact_block_count_nesting,
                exact_block_count_with_values_in_every_frame,
            );
        }
    };
    (@tests $ty:ty; $($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                super::$name::<$ty>();
            }
        )*
    };
}

stack_tests!(block, BlockAwareOperandStack);
stack_tests!(flat, FlatBlockAwareStack);
