use wasmparser::ValType;

use crate::{
    HaltError, Value,
    module::{BlockIndex, FunctionIndex},
    stack::{GlobalStack, MAX_OPERAND_STACK_SIZE, StackSection, flat::FlatBlockAwareStack},
};

const TEST_BLOCKS: usize = 8;

/// Lets the generic tests construct any `GlobalStack` implementation.
trait TestStack: GlobalStack {
    /// `blocks` is the number of blocks in the function, as passed by `WackasmModule`.
    fn with_blocks(blocks: usize) -> Self;

    fn create() -> Self
    where
        Self: Sized,
    {
        Self::with_blocks(TEST_BLOCKS)
    }
}

impl TestStack for FlatBlockAwareStack {
    fn with_blocks(blocks: usize) -> Self {
        FlatBlockAwareStack::new(blocks)
    }
}

/// Values are untyped slots, so equality is exact bit equality.
fn value_eq(a: &Value, b: &Value) -> bool {
    a.0 == b.0
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
fn return_ok<S: TestStack>(stack: &mut S, preserve: &[ValType]) -> StackSection {
    stack
        .return_section(preserve, None)
        .expect("return_section should succeed")
        .expect("a section should be open")
}

fn block(index: u32) -> StackSection {
    StackSection::Block(BlockIndex(index))
}

fn function(index: u32) -> StackSection {
    StackSection::Function(FunctionIndex(index))
}

// ---------------------------------------------------------------------------
// Basic operand stack behaviour
// ---------------------------------------------------------------------------

fn pop_empty_errors<S: TestStack>() {
    let mut stack = S::create();
    assert_trap(stack.pop());
}

fn push_pop_single<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(42)).unwrap();
    assert_pop(&mut stack, Value::from_i32(42));
    assert_empty(&mut stack);
}

fn push_pop_is_lifo<S: TestStack>() {
    let mut stack = S::create();
    for v in 0..10 {
        stack.push(Value::from_i32(v)).unwrap();
    }
    for v in (0..10).rev() {
        assert_pop(&mut stack, Value::from_i32(v));
    }
    assert_empty(&mut stack);
}

fn push_pop_all_value_types<S: TestStack>() {
    let mut stack = S::create();
    let values = [
        Value::from_i32(-1),
        Value::from_i64(i64::MIN),
        Value::from_f32(1.5),
        Value::from_f64(-2.25),
        Value::NULL_REF,
        Value(7),
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
    stack.push(Value::from_i32(1)).unwrap();
    stack.push(Value::from_i32(2)).unwrap();

    assert!(value_eq(stack.peek().unwrap(), &Value::from_i32(2)));
    assert!(value_eq(stack.peek().unwrap(), &Value::from_i32(2)));

    assert_pop(&mut stack, Value::from_i32(2));
    assert!(value_eq(stack.peek().unwrap(), &Value::from_i32(1)));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

const V128_TEST_VALUE: u128 = 0x0011_2233_4455_6677_8899_aabb_ccdd_eeff;

fn typed_pops_succeed<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(-1)).unwrap();
    stack.push(Value::from_i64(i64::MIN)).unwrap();
    stack.push(Value::from_f32(1.5)).unwrap();
    stack.push(Value::from_f64(-2.25)).unwrap();
    stack.push_v128(V128_TEST_VALUE).unwrap();

    assert_eq!(stack.pop_v128().unwrap(), V128_TEST_VALUE);
    assert_eq!(stack.pop_f64().unwrap(), -2.25);
    assert_eq!(stack.pop_f32().unwrap(), 1.5);
    assert_eq!(stack.pop_i64().unwrap(), i64::MIN);
    assert_eq!(stack.pop_i32().unwrap(), -1);
    assert_empty(&mut stack);
}

fn typed_pops_preserve_float_bits<S: TestStack>() {
    let mut stack = S::create();
    let f32_nan = f32::from_bits(0x7fc0_1234);
    let f64_nan = f64::from_bits(0x7ff8_0000_0000_1234);

    stack.push(Value::from_f32(-0.0)).unwrap();
    stack.push(Value::from_f32(f32_nan)).unwrap();
    stack.push(Value::from_f64(f64_nan)).unwrap();

    assert_eq!(stack.pop_f64().unwrap().to_bits(), f64_nan.to_bits());
    assert_eq!(stack.pop_f32().unwrap().to_bits(), f32_nan.to_bits());
    assert_eq!(stack.pop_f32().unwrap().to_bits(), (-0.0f32).to_bits());
    assert_empty(&mut stack);
}

fn i32_pops_ignore_upper_bits<S: TestStack>() {
    // Unsigned ops push zero-extended results, signed ones sign-extended; both must read back the same.
    let mut stack = S::create();
    stack.push(Value(u32::MAX as u64)).unwrap();
    stack.push(Value::from_i32(-1)).unwrap();

    assert_eq!(stack.pop_i32().unwrap(), -1);
    assert_eq!(stack.pop_i32().unwrap(), -1);
    assert_empty(&mut stack);
}

fn v128_occupies_two_slots<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(9)).unwrap();
    stack.push_v128(V128_TEST_VALUE).unwrap();

    assert_pop(&mut stack, Value(V128_TEST_VALUE as u64));
    assert_pop(&mut stack, Value((V128_TEST_VALUE >> 64) as u64));
    assert_pop(&mut stack, Value::from_i32(9));
    assert_empty(&mut stack);
}

fn pop_v128_with_one_slot_errors<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i64(1)).unwrap();
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
    stack.push(Value::from_i32(1)).unwrap();
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);

    stack.push(Value::from_i32(2)).unwrap();
    assert_pop(&mut stack, Value::from_i32(2));
    assert_empty(&mut stack);
}

// ---------------------------------------------------------------------------
// Block behaviour
// ---------------------------------------------------------------------------

fn peek_block_empty_is_none<S: TestStack>() {
    let stack = S::create();
    assert!(stack.peek_section(None).is_none());
}

fn return_block_without_block_is_none<S: TestStack>() {
    let mut stack = S::create();
    assert_eq!(stack.return_section(&[], None).unwrap(), None);
}

fn return_block_without_block_leaves_base_untouched<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    assert_eq!(stack.return_section(&[ValType::I32], None).unwrap(), None);
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

fn push_block_sets_peek_block<S: TestStack>() {
    let mut stack = S::create();
    stack.push_section(block(3), None).unwrap();
    assert_eq!(stack.peek_section(None), Some(&block(3)));
}

fn peek_block_returns_innermost<S: TestStack>() {
    let mut stack = S::create();
    stack.push_section(block(1), None).unwrap();
    stack.push_section(block(2), None).unwrap();
    stack.push_section(block(3), None).unwrap();
    assert_eq!(stack.peek_section(None), Some(&block(3)));

    assert_eq!(return_ok(&mut stack, &[]), block(3));
    assert_eq!(stack.peek_section(None), Some(&block(2)));

    assert_eq!(return_ok(&mut stack, &[]), block(2));
    assert_eq!(stack.peek_section(None), Some(&block(1)));

    assert_eq!(return_ok(&mut stack, &[]), block(1));
    assert!(stack.peek_section(None).is_none());
    assert_eq!(stack.return_section(&[], None).unwrap(), None);
}

fn new_block_starts_empty<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    assert_empty(&mut stack);
}

fn block_pops_down_to_floor_only<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    stack.push(Value::from_i32(3)).unwrap();

    assert_pop(&mut stack, Value::from_i32(3));
    assert_pop(&mut stack, Value::from_i32(2));
    assert_empty(&mut stack);
}

fn push_block_with_parameters<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(100)).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push(Value::from_i64(2)).unwrap();
    stack
        .push_section(block(0), Some(&[ValType::I32, ValType::I64]))
        .unwrap();

    assert_pop(&mut stack, Value::from_i64(2));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

fn push_block_with_empty_parameters<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(100)).unwrap();
    stack.push_section(block(0), Some(&[])).unwrap();
    assert_empty(&mut stack);
    assert_eq!(stack.peek_section(None), Some(&block(0)));
}

fn block_parameters_preserve_order<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push(Value::from_i64(2)).unwrap();

    stack
        .push_section(block(0), Some(&[ValType::I32, ValType::I64]))
        .unwrap();

    assert_eq!(stack.pop_i64().unwrap(), 2);
    assert_eq!(stack.pop_i32().unwrap(), 1);
    assert_empty(&mut stack);
}

fn return_block_without_preserve_discards_values<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(5), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    stack.push(Value::from_i32(3)).unwrap();

    assert_eq!(return_ok(&mut stack, &[]), block(5));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

fn return_block_preserves_single_value<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i64(42)).unwrap();

    assert_eq!(
        return_ok(&mut stack, &[ValType::I64]),
        block(0)
    );
    assert_pop(&mut stack, Value::from_i64(42));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

fn return_block_preserves_multiple_values_in_order<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(10)).unwrap();
    stack.push(Value::from_f32(20.0)).unwrap();
    stack.push(Value::from_i64(30)).unwrap();

    return_ok(&mut stack, &[ValType::I32, ValType::F32, ValType::I64]);

    assert_pop(&mut stack, Value::from_i64(30));
    assert_pop(&mut stack, Value::from_f32(20.0));
    assert_pop(&mut stack, Value::from_i32(10));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

fn return_block_discards_values_below_preserved<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(97)).unwrap();
    stack.push(Value::from_i32(98)).unwrap();
    stack.push(Value::from_i32(99)).unwrap();

    return_ok(&mut stack, &[ValType::I32]);

    assert_pop(&mut stack, Value::from_i32(99));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

fn return_block_preserve_underflow_errors<S: TestStack>() {
    // The block has fewer values than requested; it must not reach into the parent.
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    assert_trap(stack.return_section(&[ValType::I32, ValType::I32], None));
}

fn nested_blocks_are_isolated<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(0)).unwrap();
    stack.push_section(block(1), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(2), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();

    assert_pop(&mut stack, Value::from_i32(2));
    assert_empty(&mut stack);

    assert_eq!(return_ok(&mut stack, &[]), block(2));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);

    assert_eq!(return_ok(&mut stack, &[]), block(1));
    assert_pop(&mut stack, Value::from_i32(0));
    assert_empty(&mut stack);
}

fn nested_blocks_propagate_preserved_values<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(0)).unwrap();
    stack.push_section(block(1), None).unwrap();
    stack.push(Value::from_i32(10)).unwrap();
    stack.push_section(block(2), None).unwrap();
    stack.push(Value::from_i32(20)).unwrap();
    stack.push(Value::from_i32(21)).unwrap();

    // Inner block yields two values into the middle block.
    return_ok(&mut stack, &[ValType::I32, ValType::I32]);
    assert_eq!(stack.peek_section(None), Some(&block(1)));

    // Middle block now holds [10, 20, 21]; yield the top two into the base.
    return_ok(&mut stack, &[ValType::I32, ValType::I32]);
    assert!(stack.peek_section(None).is_none());

    assert_pop(&mut stack, Value::from_i32(21));
    assert_pop(&mut stack, Value::from_i32(20));
    assert_pop(&mut stack, Value::from_i32(0));
    assert_empty(&mut stack);
}

fn block_with_parameters_and_results<S: TestStack>() {
    // (i32 i32) -> (i32) block that adds its params.
    let mut stack = S::create();
    stack.push(Value::from_i32(7)).unwrap();
    stack.push(Value::from_i32(3)).unwrap();
    stack.push(Value::from_i32(4)).unwrap();

    stack
        .push_section(block(9), Some(&[ValType::I32, ValType::I32]))
        .unwrap();

    let b = stack.pop_i32().unwrap();
    let a = stack.pop_i32().unwrap();
    assert_eq!((a, b), (3, 4));
    stack.push(Value::from_i32(a + b)).unwrap();

    assert_eq!(
        return_ok(&mut stack, &[ValType::I32]),
        block(9)
    );
    assert_pop(&mut stack, Value::from_i32(7));
    assert_pop(&mut stack, Value::from_i32(7));
    assert_empty(&mut stack);
}

fn reentered_block_starts_empty<S: TestStack>() {
    let mut stack = S::create();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    return_ok(&mut stack, &[]);

    stack.push_section(block(0), None).unwrap();
    assert_empty(&mut stack);
}

fn many_sequential_blocks<S: TestStack>() {
    let mut stack = S::create();
    for i in 0..(TEST_BLOCKS as u32 * 4) {
        stack.push_section(block(i), None).unwrap();
        stack.push(Value::from_i32(i as i32)).unwrap();
        assert_eq!(
            return_ok(&mut stack, &[ValType::I32]),
            block(i)
        );
    }
    for i in (0..(TEST_BLOCKS as i32 * 4)).rev() {
        assert_pop(&mut stack, Value::from_i32(i));
    }
    assert_empty(&mut stack);
}

fn deeply_nested_blocks<S: TestStack>() {
    let mut stack = S::create();
    let depth = TEST_BLOCKS as u32 - 1;
    for i in 0..depth {
        stack.push(Value::from_i32(i as i32)).unwrap();
        stack.push_section(block(i), None).unwrap();
    }
    for i in (0..depth).rev() {
        assert_empty(&mut stack);
        assert_eq!(return_ok(&mut stack, &[]), block(i));
        assert_pop(&mut stack, Value::from_i32(i as i32));
    }
    assert!(stack.peek_section(None).is_none());
    assert_empty(&mut stack);
}

fn block_can_use_full_stack_size<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(-1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    for i in 0..(MAX_OPERAND_STACK_SIZE - 1) {
        stack.push(Value::from_i32(i as i32)).unwrap();
    }
    return_ok(&mut stack, &[]);
    assert_pop(&mut stack, Value::from_i32(-1));
    assert_empty(&mut stack);
}

// ---------------------------------------------------------------------------
// Function sections
// ---------------------------------------------------------------------------

fn push_function_sets_peek_section<S: TestStack>() {
    let mut stack = S::create();
    stack.push_section(function(4), None).unwrap();
    assert_eq!(stack.peek_section(None), Some(&function(4)));
    assert_eq!(return_ok(&mut stack, &[]), function(4));
    assert!(stack.peek_section(None).is_none());
}

fn function_section_takes_arguments_and_returns_results<S: TestStack>() {
    // Caller pushes (i32 i64) arguments, callee returns a single i32.
    let mut stack = S::create();
    stack.push(Value::from_i32(100)).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push(Value::from_i64(2)).unwrap();
    stack
        .push_section(function(1), Some(&[ValType::I32, ValType::I64]))
        .unwrap();

    assert_eq!(stack.pop_i64().unwrap(), 2);
    assert_eq!(stack.pop_i32().unwrap(), 1);
    assert_empty(&mut stack);
    stack.push(Value::from_i32(3)).unwrap();

    assert_eq!(return_ok(&mut stack, &[ValType::I32]), function(1));
    assert_pop(&mut stack, Value::from_i32(3));
    assert_pop(&mut stack, Value::from_i32(100));
    assert_empty(&mut stack);
}

fn blocks_nest_inside_function_sections<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(0)).unwrap();
    stack.push_section(function(0), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    stack.push_section(function(1), None).unwrap();
    stack.push(Value::from_i32(3)).unwrap();
    assert_eq!(stack.peek_section(None), Some(&function(1)));

    assert_eq!(return_ok(&mut stack, &[ValType::I32]), function(1));
    assert_eq!(stack.peek_section(None), Some(&block(0)));
    assert_pop(&mut stack, Value::from_i32(3));
    assert_pop(&mut stack, Value::from_i32(2));
    assert_empty(&mut stack);

    assert_eq!(return_ok(&mut stack, &[]), block(0));
    assert_eq!(stack.peek_section(None), Some(&function(0)));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);

    assert_eq!(return_ok(&mut stack, &[]), function(0));
    assert!(stack.peek_section(None).is_none());
    assert_pop(&mut stack, Value::from_i32(0));
    assert_empty(&mut stack);
}

fn sections_with_same_index_are_distinct<S: TestStack>() {
    assert_ne!(block(0), function(0));

    let mut stack = S::create();
    stack.push_section(block(0), None).unwrap();
    stack.push_section(function(0), None).unwrap();
    assert_eq!(return_ok(&mut stack, &[]), function(0));
    assert_eq!(return_ok(&mut stack, &[]), block(0));
}

// ---------------------------------------------------------------------------
// Unwinding multiple sections at once
// ---------------------------------------------------------------------------

fn return_section_at_position_unwinds_inner_sections<S: TestStack>() {
    // Models `br 2` out of three nested blocks carrying one value.
    let mut stack = S::create();
    stack.push(Value::from_i32(0)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(1), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    stack.push_section(block(2), None).unwrap();
    stack.push(Value::from_i32(3)).unwrap();
    stack.push(Value::from_i32(4)).unwrap();

    assert_eq!(
        stack.return_section(&[ValType::I32], Some(0)).unwrap(),
        Some(block(0))
    );
    assert!(stack.peek_section(None).is_none());
    assert_pop(&mut stack, Value::from_i32(4));
    assert_pop(&mut stack, Value::from_i32(0));
    assert_empty(&mut stack);
}

fn return_section_at_position_keeps_outer_sections<S: TestStack>() {
    let mut stack = S::create();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(1), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    stack.push_section(block(2), None).unwrap();
    stack.push(Value::from_i32(3)).unwrap();

    assert_eq!(
        stack.return_section(&[ValType::I32], Some(1)).unwrap(),
        Some(block(1))
    );
    assert_eq!(stack.peek_section(None), Some(&block(0)));
    assert_pop(&mut stack, Value::from_i32(3));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

fn return_section_at_position_then_repush_resets_loop<S: TestStack>() {
    // Models `br 1` back to a loop with one i32 param, discarding leftovers.
    let mut stack = S::create();
    stack.push(Value::from_i32(100)).unwrap();
    stack.push(Value::from_i32(5)).unwrap();
    stack
        .push_section(block(0), Some(&[ValType::I32]))
        .unwrap();
    stack.push(Value::from_i32(6)).unwrap();
    stack.push_section(block(1), None).unwrap();
    stack.push(Value::from_i32(7)).unwrap();
    stack.push(Value::from_i32(8)).unwrap();

    assert_eq!(
        stack.return_section(&[ValType::I32], Some(0)).unwrap(),
        Some(block(0))
    );
    stack
        .push_section(block(0), Some(&[ValType::I32]))
        .unwrap();

    assert_eq!(stack.peek_section(None), Some(&block(0)));
    assert_pop(&mut stack, Value::from_i32(8));
    assert_empty(&mut stack);

    assert_eq!(return_ok(&mut stack, &[]), block(0));
    assert_pop(&mut stack, Value::from_i32(100));
    assert_empty(&mut stack);
}

fn return_section_invalid_position_errors<S: TestStack>() {
    let mut stack = S::create();
    assert_trap(stack.return_section(&[], Some(0)));

    stack.push_section(block(0), None).unwrap();
    assert_trap(stack.return_section(&[], Some(1)));
    assert_eq!(stack.peek_section(None), Some(&block(0)));
}

fn return_section_at_position_underflow_errors<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push_section(block(1), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    assert_trap(stack.return_section(&[ValType::I32, ValType::I32], Some(0)));
}

fn return_function_without_function_is_none<S: TestStack>() {
    let mut stack = S::create();
    assert_eq!(stack.return_function(&[]).unwrap(), None);

    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    assert_eq!(stack.return_function(&[ValType::I32]).unwrap(), None);
    assert_eq!(stack.peek_section(None), Some(&block(0)));
    assert_pop(&mut stack, Value::from_i32(1));
}

fn return_function_from_top_level<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(100)).unwrap();
    stack.push_section(function(0), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push(Value::from_i32(2)).unwrap();

    assert_eq!(
        stack.return_function(&[ValType::I32]).unwrap(),
        Some(function(0))
    );
    assert!(stack.peek_section(None).is_none());
    assert_pop(&mut stack, Value::from_i32(2));
    assert_pop(&mut stack, Value::from_i32(100));
    assert_empty(&mut stack);
}

fn return_function_unwinds_open_blocks<S: TestStack>() {
    // `return` from inside nested blocks closes every block in the function.
    let mut stack = S::create();
    stack.push(Value::from_i32(100)).unwrap();
    stack.push_section(function(0), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    stack.push_section(block(1), None).unwrap();
    stack.push(Value::from_i32(3)).unwrap();
    stack.push(Value::from_i64(4)).unwrap();

    assert_eq!(
        stack.return_function(&[ValType::I32, ValType::I64]).unwrap(),
        Some(function(0))
    );
    assert!(stack.peek_section(None).is_none());
    assert_pop(&mut stack, Value::from_i64(4));
    assert_pop(&mut stack, Value::from_i32(3));
    assert_pop(&mut stack, Value::from_i32(100));
    assert_empty(&mut stack);
}

fn return_function_only_unwinds_innermost_function<S: TestStack>() {
    let mut stack = S::create();
    stack.push_section(function(0), None).unwrap();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(block(0), None).unwrap();
    stack.push(Value::from_i32(2)).unwrap();
    stack.push_section(function(1), None).unwrap();
    stack.push_section(block(1), None).unwrap();
    stack.push(Value::from_i32(3)).unwrap();

    assert_eq!(
        stack.return_function(&[ValType::I32]).unwrap(),
        Some(function(1))
    );
    assert_eq!(stack.peek_section(None), Some(&block(0)));
    assert_pop(&mut stack, Value::from_i32(3));
    assert_pop(&mut stack, Value::from_i32(2));
    assert_empty(&mut stack);

    assert_eq!(stack.return_function(&[]).unwrap(), Some(function(0)));
    assert!(stack.peek_section(None).is_none());
    assert_empty(&mut stack);
}

fn return_function_underflow_errors<S: TestStack>() {
    let mut stack = S::create();
    stack.push(Value::from_i32(1)).unwrap();
    stack.push_section(function(0), None).unwrap();
    stack.push_section(block(0), None).unwrap();
    assert_trap(stack.return_function(&[ValType::I32]));
}

// ---------------------------------------------------------------------------
// Sizing: stacks built for the exact number of blocks a function has
// ---------------------------------------------------------------------------

fn zero_blocks_push_pop<S: TestStack>() {
    // Functions without blocks (and the placeholder frame) are created with 0 blocks.
    let mut stack = S::with_blocks(0);
    stack.push(Value::from_i32(1)).unwrap();
    stack.push(Value::from_i64(2)).unwrap();
    assert!(value_eq(stack.peek().unwrap(), &Value::from_i64(2)));
    assert_pop(&mut stack, Value::from_i64(2));
    assert_pop(&mut stack, Value::from_i32(1));
    assert_empty(&mut stack);
}

fn zero_blocks_push_until_overflow<S: TestStack>() {
    let mut stack = S::with_blocks(0);
    for i in 0..MAX_OPERAND_STACK_SIZE {
        stack.push(Value::from_i32(i as i32)).unwrap();
    }
    assert_trap(stack.push(Value::from_i32(-1)));
    assert_pop(&mut stack, Value::from_i32(MAX_OPERAND_STACK_SIZE as i32 - 1));
}

fn exact_block_count_nesting<S: TestStack>() {
    for blocks in 1..=4 {
        let mut stack = S::with_blocks(blocks);
        stack.push(Value::from_i32(100)).unwrap();

        for depth in 0..blocks {
            stack.push_section(block(depth as u32), None).unwrap();
            stack.push(Value::from_i32(depth as i32)).unwrap();
        }

        // Each block yields its top value into its parent, discarding the parent's own.
        for depth in (0..blocks).rev() {
            assert_eq!(
                return_ok(&mut stack, &[ValType::I32]),
                block(depth as u32)
            );
        }

        assert!(stack.peek_section(None).is_none());
        assert_pop(&mut stack, Value::from_i32(blocks as i32 - 1));
        assert_pop(&mut stack, Value::from_i32(100));
        assert_empty(&mut stack);
    }
}

fn exact_block_count_with_values_in_every_frame<S: TestStack>() {
    let per_frame = MAX_OPERAND_STACK_SIZE / 4;
    let mut stack = S::with_blocks(1);

    for i in 0..per_frame {
        stack.push(Value::from_i32(i as i32)).unwrap();
    }
    stack.push_section(block(0), None).unwrap();
    for i in 0..per_frame {
        stack.push(Value::from_i64(i as i64)).unwrap();
    }

    return_ok(&mut stack, &[ValType::I64]);
    assert_pop(&mut stack, Value::from_i64(per_frame as i64 - 1));
    for i in (0..per_frame).rev() {
        assert_pop(&mut stack, Value::from_i32(i as i32));
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
                push_pop_single,
                push_pop_is_lifo,
                push_pop_all_value_types,
                peek_does_not_remove,
                typed_pops_succeed,
                typed_pops_preserve_float_bits,
                i32_pops_ignore_upper_bits,
                v128_occupies_two_slots,
                pop_v128_with_one_slot_errors,
                typed_pops_on_empty_error,
                stack_is_reusable_after_draining,
                peek_block_empty_is_none,
                return_block_without_block_is_none,
                return_block_without_block_leaves_base_untouched,
                push_block_sets_peek_block,
                peek_block_returns_innermost,
                new_block_starts_empty,
                block_pops_down_to_floor_only,
                push_block_with_parameters,
                push_block_with_empty_parameters,
                block_parameters_preserve_order,
                return_block_without_preserve_discards_values,
                return_block_preserves_single_value,
                return_block_preserves_multiple_values_in_order,
                return_block_discards_values_below_preserved,
                return_block_preserve_underflow_errors,
                nested_blocks_are_isolated,
                nested_blocks_propagate_preserved_values,
                block_with_parameters_and_results,
                reentered_block_starts_empty,
                many_sequential_blocks,
                deeply_nested_blocks,
                block_can_use_full_stack_size,
                push_function_sets_peek_section,
                function_section_takes_arguments_and_returns_results,
                blocks_nest_inside_function_sections,
                sections_with_same_index_are_distinct,
                return_section_at_position_unwinds_inner_sections,
                return_section_at_position_keeps_outer_sections,
                return_section_at_position_then_repush_resets_loop,
                return_section_invalid_position_errors,
                return_section_at_position_underflow_errors,
                return_function_without_function_is_none,
                return_function_from_top_level,
                return_function_unwinds_open_blocks,
                return_function_only_unwinds_innermost_function,
                return_function_underflow_errors,
                zero_blocks_push_pop,
                zero_blocks_push_until_overflow,
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

stack_tests!(flat, FlatBlockAwareStack);
