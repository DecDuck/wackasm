use wackasm::Value;
use wast::{
    core::{NanPattern, V128Pattern, WastArgCore, WastRetCore},
    token::{F32, F64},
};

#[derive(Debug)]
pub(super) enum ExpectedValue {
    I32(i32),
    I64(i64),
    F32(NanPattern<F32>),
    F64(NanPattern<F64>),
    V128(V128Pattern),
    NullRef,
    FuncRef,
}

impl ExpectedValue {
    /// Number of runtime slots the value occupies; a v128 spans two.
    fn slots(&self) -> usize {
        match self {
            Self::V128(_) => 2,
            _ => 1,
        }
    }
}

/// Splits a v128 into runtime slots, matching the stack layout (high half first).
fn v128_to_slots(value: u128) -> [Value; 2] {
    [Value((value >> 64) as u64), Value(value as u64)]
}

pub(super) fn convert_args(args: &[wast::WastArg<'_>]) -> Result<Vec<Value>, String> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            wast::WastArg::Core(WastArgCore::V128(value)) => {
                values.extend(v128_to_slots(u128::from_le_bytes(value.to_le_bytes())));
            }
            _ => values.push(convert_arg(arg)?),
        }
    }
    Ok(values)
}

fn convert_arg(arg: &wast::WastArg<'_>) -> Result<Value, String> {
    match arg {
        wast::WastArg::Core(WastArgCore::I32(value)) => Ok(Value::from_i32(*value)),
        wast::WastArg::Core(WastArgCore::I64(value)) => Ok(Value::from_i64(*value)),
        wast::WastArg::Core(WastArgCore::F32(value)) => Ok(Value(value.bits as u64)),
        wast::WastArg::Core(WastArgCore::F64(value)) => Ok(Value(value.bits)),
        wast::WastArg::Core(WastArgCore::V128(_)) => {
            unreachable!("v128 arguments span two slots and are handled by convert_args")
        }
        wast::WastArg::Core(WastArgCore::RefNull(_)) => Ok(Value::NULL_REF),
        wast::WastArg::Core(WastArgCore::RefExtern(_) | WastArgCore::RefHost(_)) => {
            Err("extern and host references are not representable by the runtime".to_owned())
        }
        _ => Err("component-model arguments are not implemented by this runner".to_owned()),
    }
}

pub(super) fn convert_expected(expected: &wast::WastRet<'_>) -> Result<Vec<ExpectedValue>, String> {
    let wast::WastRet::Core(expected) = expected else {
        return Err("component-model return values are not implemented".to_owned());
    };
    convert_core_expected(expected)
}

fn convert_core_expected(expected: &WastRetCore<'_>) -> Result<Vec<ExpectedValue>, String> {
    match expected {
        WastRetCore::I32(value) => Ok(vec![ExpectedValue::I32(*value)]),
        WastRetCore::I64(value) => Ok(vec![ExpectedValue::I64(*value)]),
        WastRetCore::F32(value) => Ok(vec![ExpectedValue::F32(*value)]),
        WastRetCore::F64(value) => Ok(vec![ExpectedValue::F64(*value)]),
        WastRetCore::V128(pattern) => Ok(vec![ExpectedValue::V128(pattern.clone())]),
        WastRetCore::RefNull(_) => Ok(vec![ExpectedValue::NullRef]),
        WastRetCore::RefFunc(None) => Ok(vec![ExpectedValue::FuncRef]),
        WastRetCore::Either(cases) => cases
            .iter()
            .map(convert_core_expected)
            .collect::<Result<Vec<_>, _>>()
            .map(|alternatives| alternatives.into_iter().flatten().collect()),
        WastRetCore::RefExtern(_) | WastRetCore::RefHost(_) => {
            Err("extern and host references are not representable by the runtime".to_owned())
        }
        WastRetCore::RefFunc(Some(_)) => {
            Err("function reference identity is not exposed by the runtime".to_owned())
        }
        WastRetCore::RefAny
        | WastRetCore::RefEq
        | WastRetCore::RefArray
        | WastRetCore::RefStruct
        | WastRetCore::RefI31
        | WastRetCore::RefI31Shared => {
            Err("GC references are not representable by the runtime".to_owned())
        }
    }
}

/// Matches result slots against the expected results, where each expected result is a set
/// of alternatives. A v128 result consumes two slots.
pub(super) fn results_match(actual: &[Value], expected: &[Vec<ExpectedValue>]) -> bool {
    let mut remaining = actual;
    for alternatives in expected {
        let Some(slots) = alternatives.first().map(ExpectedValue::slots) else {
            return false;
        };
        let Some((current, rest)) = remaining.split_at_checked(slots) else {
            return false;
        };
        if !alternatives
            .iter()
            .any(|expected| slots_match(current, expected))
        {
            return false;
        }
        remaining = rest;
    }
    remaining.is_empty()
}

fn slots_match(slots: &[Value], expected: &ExpectedValue) -> bool {
    match (expected, slots) {
        (ExpectedValue::V128(pattern), [high, low]) => {
            v128_matches(((high.0 as u128) << 64) | low.0 as u128, pattern)
        }
        (ExpectedValue::V128(_), _) => false,
        (_, [value]) => value_matches(value, expected),
        _ => false,
    }
}

/// Values are untyped slots, so the expected type decides how the bits are read.
fn value_matches(actual: &Value, expected: &ExpectedValue) -> bool {
    match expected {
        ExpectedValue::I32(expected) => actual.as_i32() == *expected,
        ExpectedValue::I64(expected) => actual.as_i64() == *expected,
        ExpectedValue::F32(expected) => f32_matches(actual.0 as u32, *expected),
        ExpectedValue::F64(expected) => f64_matches(actual.0, *expected),
        ExpectedValue::V128(_) => false,
        ExpectedValue::NullRef => actual.is_null_ref(),
        ExpectedValue::FuncRef => !actual.is_null_ref(),
    }
}

/// Lane 0 is the least significant lane, as in the little-endian `v128` encoding.
fn v128_matches(actual: u128, expected: &V128Pattern) -> bool {
    let bytes = actual.to_le_bytes();
    match expected {
        V128Pattern::I8x16(lanes) => lanes
            .iter()
            .zip(lane_bytes::<1>(&bytes))
            .all(|(lane, actual)| lane.to_le_bytes() == actual),
        V128Pattern::I16x8(lanes) => lanes
            .iter()
            .zip(lane_bytes::<2>(&bytes))
            .all(|(lane, actual)| lane.to_le_bytes() == actual),
        V128Pattern::I32x4(lanes) => lanes
            .iter()
            .zip(lane_bytes::<4>(&bytes))
            .all(|(lane, actual)| lane.to_le_bytes() == actual),
        V128Pattern::I64x2(lanes) => lanes
            .iter()
            .zip(lane_bytes::<8>(&bytes))
            .all(|(lane, actual)| lane.to_le_bytes() == actual),
        V128Pattern::F32x4(lanes) => lanes
            .iter()
            .zip(lane_bytes::<4>(&bytes))
            .all(|(lane, actual)| f32_matches(u32::from_le_bytes(actual), *lane)),
        V128Pattern::F64x2(lanes) => lanes
            .iter()
            .zip(lane_bytes::<8>(&bytes))
            .all(|(lane, actual)| f64_matches(u64::from_le_bytes(actual), *lane)),
    }
}

fn lane_bytes<const N: usize>(bytes: &[u8; 16]) -> impl Iterator<Item = [u8; N]> + '_ {
    bytes
        .chunks_exact(N)
        .map(|lane| lane.try_into().expect("chunks_exact yields N-byte lanes"))
}

fn f32_matches(actual: u32, expected: NanPattern<F32>) -> bool {
    match expected {
        NanPattern::Value(expected) => actual == expected.bits,
        NanPattern::CanonicalNan => actual & 0x7fff_ffff == 0x7fc0_0000,
        NanPattern::ArithmeticNan => {
            actual & 0x7f80_0000 == 0x7f80_0000 && actual & 0x0040_0000 != 0
        }
    }
}

fn f64_matches(actual: u64, expected: NanPattern<F64>) -> bool {
    match expected {
        NanPattern::Value(expected) => actual == expected.bits,
        NanPattern::CanonicalNan => actual & 0x7fff_ffff_ffff_ffff == 0x7ff8_0000_0000_0000,
        NanPattern::ArithmeticNan => {
            actual & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000
                && actual & 0x0008_0000_0000_0000 != 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExpectedValue, convert_arg, convert_args, convert_core_expected, results_match,
        value_matches,
    };
    use wackasm::Value;
    use wast::{
        WastArg,
        core::{NanPattern, V128Const, V128Pattern, WastArgCore, WastRetCore},
        token::{F32, F64},
    };

    #[test]
    fn converts_float_arguments_without_changing_bits() {
        let f32 = WastArg::Core(WastArgCore::F32(F32 { bits: 0x7fc0_1234 }));
        let f64 = WastArg::Core(WastArgCore::F64(F64 {
            bits: 0x7ff8_0000_0000_1234,
        }));

        assert_eq!(convert_arg(&f32).unwrap().as_f32().to_bits(), 0x7fc0_1234);
        assert_eq!(
            convert_arg(&f64).unwrap().as_f64().to_bits(),
            0x7ff8_0000_0000_1234
        );
    }

    #[test]
    fn matches_integers_by_expected_width() {
        let minus_one = convert_arg(&WastArg::Core(WastArgCore::I32(-1))).unwrap();
        assert!(value_matches(&minus_one, &ExpectedValue::I32(-1)));
        // Zero-extended i32 results must still compare equal.
        assert!(value_matches(&Value(u32::MAX as u64), &ExpectedValue::I32(-1)));
        assert!(!value_matches(&Value::from_i32(1), &ExpectedValue::I32(-1)));

        assert!(value_matches(
            &Value::from_i64(i64::MIN),
            &ExpectedValue::I64(i64::MIN)
        ));
        assert!(!value_matches(
            &Value(u32::MAX as u64),
            &ExpectedValue::I64(-1)
        ));
    }

    #[test]
    fn compares_float_nan_patterns_and_exact_values() {
        let canonical = ExpectedValue::F32(NanPattern::CanonicalNan);
        let arithmetic = ExpectedValue::F32(NanPattern::ArithmeticNan);
        let exact = ExpectedValue::F32(NanPattern::Value(F32 { bits: 0x8000_0000 }));

        assert!(value_matches(
            &Value::from_f32(f32::from_bits(0xffc0_0000)),
            &canonical
        ));
        assert!(value_matches(
            &Value::from_f32(f32::from_bits(0x7fc0_1234)),
            &arithmetic
        ));
        assert!(!value_matches(
            &Value::from_f32(f32::from_bits(0x7f80_0001)),
            &arithmetic
        ));
        assert!(value_matches(&Value::from_f32(-0.0), &exact));
        assert!(!value_matches(&Value::from_f32(0.0), &exact));

        let f64_canonical = ExpectedValue::F64(NanPattern::CanonicalNan);
        assert!(value_matches(
            &Value::from_f64(f64::from_bits(0xfff8_0000_0000_0000)),
            &f64_canonical
        ));
    }

    #[test]
    fn converts_vector_arguments_to_high_then_low_slots() {
        let vector = WastArg::Core(WastArgCore::V128(V128Const::I32x4([1, 2, 3, 4])));
        let slots = convert_args(&[WastArg::Core(WastArgCore::I32(7)), vector]).unwrap();

        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0].as_i32(), 7);
        // Lanes 2 and 3 form the high half; lanes 0 and 1 the low half.
        assert_eq!(slots[1].0, 0x0000_0004_0000_0003);
        assert_eq!(slots[2].0, 0x0000_0002_0000_0001);
    }

    #[test]
    fn matches_vector_results_by_lane() {
        let args = convert_args(&[WastArg::Core(WastArgCore::V128(V128Const::I16x8([
            -1, 2, -3, 4, -5, 6, -7, 8,
        ])))])
        .unwrap();
        let expected = |pattern| vec![convert_core_expected(&WastRetCore::V128(pattern)).unwrap()];

        assert!(results_match(
            &args,
            &expected(V128Pattern::I16x8([-1, 2, -3, 4, -5, 6, -7, 8]))
        ));
        assert!(!results_match(
            &args,
            &expected(V128Pattern::I16x8([-1, 2, -3, 4, -5, 6, -7, 9]))
        ));
        // A single slot can't satisfy a v128 result.
        assert!(!results_match(
            &args[..1],
            &expected(V128Pattern::I16x8([0; 8]))
        ));
    }

    #[test]
    fn matches_vector_float_lanes_with_nan_patterns() {
        let bits = [0x7fc0_0000_u32, 0x3fc0_0000, 0x7fc0_1234, 0x8000_0000];
        let args = convert_args(&[WastArg::Core(WastArgCore::V128(V128Const::F32x4(
            bits.map(|bits| F32 { bits }),
        )))])
        .unwrap();
        let expected = vec![
            convert_core_expected(&WastRetCore::V128(V128Pattern::F32x4([
                NanPattern::CanonicalNan,
                NanPattern::Value(F32 { bits: 0x3fc0_0000 }),
                NanPattern::ArithmeticNan,
                NanPattern::Value(F32 { bits: 0x8000_0000 }),
            ])))
            .unwrap(),
        ];

        assert!(results_match(&args, &expected));
    }

    #[test]
    fn matches_mixed_scalar_and_vector_results_in_order() {
        let mut actual = convert_args(&[
            WastArg::Core(WastArgCore::I32(5)),
            WastArg::Core(WastArgCore::V128(V128Const::I64x2([-1, 9]))),
        ])
        .unwrap();
        let expected = vec![
            convert_core_expected(&WastRetCore::I32(5)).unwrap(),
            convert_core_expected(&WastRetCore::V128(V128Pattern::I64x2([-1, 9]))).unwrap(),
        ];

        assert!(results_match(&actual, &expected));
        actual.push(Value(0));
        assert!(!results_match(&actual, &expected));
    }

    #[test]
    fn matches_reference_values() {
        let null = convert_arg(&WastArg::Core(WastArgCore::RefNull(
            wast::core::HeapType::Abstract {
                shared: false,
                ty: wast::core::AbstractHeapType::Func,
            },
        )))
        .unwrap();
        assert!(value_matches(&null, &ExpectedValue::NullRef));
        assert!(!value_matches(&null, &ExpectedValue::FuncRef));
        assert!(value_matches(&Value(3), &ExpectedValue::FuncRef));
        assert!(!value_matches(&Value(3), &ExpectedValue::NullRef));
    }
}
