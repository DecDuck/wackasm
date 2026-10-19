use wackasm::Value;
use wast::{
    core::{NanPattern, V128Pattern, WastArgCore, WastRetCore},
    token::{F32, F64},
};

#[derive(Debug)]
pub(super) enum ExpectedValue {
    Value(Value),
    F32(NanPattern<F32>),
    F64(NanPattern<F64>),
    V128(V128Pattern),
    NullRef,
    FuncRef,
}

pub(super) fn convert_arg(arg: &wast::WastArg<'_>) -> Result<Value, String> {
    match arg {
        wast::WastArg::Core(WastArgCore::I32(value)) => Ok(Value::I32(*value)),
        wast::WastArg::Core(WastArgCore::I64(value)) => Ok(Value::I64(*value)),
        wast::WastArg::Core(WastArgCore::F32(value)) => Ok(Value::F32(f32::from_bits(value.bits))),
        wast::WastArg::Core(WastArgCore::F64(value)) => Ok(Value::F64(f64::from_bits(value.bits))),
        wast::WastArg::Core(WastArgCore::V128(value)) => {
            Ok(Value::V128(u128::from_le_bytes(value.to_le_bytes())))
        }
        wast::WastArg::Core(WastArgCore::RefNull(_)) => Ok(Value::Unknown),
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
        WastRetCore::I32(value) => Ok(vec![ExpectedValue::Value(Value::I32(*value))]),
        WastRetCore::I64(value) => Ok(vec![ExpectedValue::Value(Value::I64(*value))]),
        WastRetCore::F32(value) => Ok(vec![ExpectedValue::F32(*value)]),
        WastRetCore::F64(value) => Ok(vec![ExpectedValue::F64(*value)]),
        WastRetCore::V128(value) => Ok(vec![ExpectedValue::V128(value.clone())]),
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

pub(super) fn value_matches(actual: &Value, expected: &ExpectedValue) -> bool {
    match (actual, expected) {
        (Value::I32(actual), ExpectedValue::Value(Value::I32(expected))) => actual == expected,
        (Value::I64(actual), ExpectedValue::Value(Value::I64(expected))) => actual == expected,
        (Value::F32(actual), ExpectedValue::F32(expected)) => {
            f32_matches(actual.to_bits(), *expected)
        }
        (Value::F64(actual), ExpectedValue::F64(expected)) => {
            f64_matches(actual.to_bits(), *expected)
        }
        (Value::V128(actual), ExpectedValue::V128(expected)) => v128_matches(*actual, expected),
        (Value::Unknown, ExpectedValue::NullRef) => true,
        (_, ExpectedValue::FuncRef) => actual.is_func_ref(),
        _ => false,
    }
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

fn v128_matches(actual: u128, expected: &V128Pattern) -> bool {
    let bytes = actual.to_le_bytes();
    match expected {
        V128Pattern::I8x16(lanes) => bytes
            .iter()
            .zip(lanes)
            .all(|(actual, expected)| *actual as i8 == *expected),
        V128Pattern::I16x8(lanes) => bytes
            .chunks_exact(2)
            .zip(lanes)
            .all(|(actual, expected)| i16::from_le_bytes([actual[0], actual[1]]) == *expected),
        V128Pattern::I32x4(lanes) => bytes.chunks_exact(4).zip(lanes).all(|(actual, expected)| {
            i32::from_le_bytes(actual.try_into().expect("four-byte lane")) == *expected
        }),
        V128Pattern::I64x2(lanes) => bytes.chunks_exact(8).zip(lanes).all(|(actual, expected)| {
            i64::from_le_bytes(actual.try_into().expect("eight-byte lane")) == *expected
        }),
        V128Pattern::F32x4(lanes) => bytes.chunks_exact(4).zip(lanes).all(|(actual, expected)| {
            f32_matches(
                u32::from_le_bytes(actual.try_into().expect("four-byte lane")),
                *expected,
            )
        }),
        V128Pattern::F64x2(lanes) => bytes.chunks_exact(8).zip(lanes).all(|(actual, expected)| {
            f64_matches(
                u64::from_le_bytes(actual.try_into().expect("eight-byte lane")),
                *expected,
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{ExpectedValue, convert_arg, convert_core_expected, value_matches};
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

        assert!(
            matches!(convert_arg(&f32), Ok(Value::F32(value)) if value.to_bits() == 0x7fc0_1234)
        );
        assert!(
            matches!(convert_arg(&f64), Ok(Value::F64(value)) if value.to_bits() == 0x7ff8_0000_0000_1234)
        );
    }

    #[test]
    fn compares_float_nan_patterns_and_exact_values() {
        let canonical = ExpectedValue::F32(NanPattern::CanonicalNan);
        let arithmetic = ExpectedValue::F32(NanPattern::ArithmeticNan);
        let exact = ExpectedValue::F32(NanPattern::Value(F32 { bits: 0x8000_0000 }));

        assert!(value_matches(
            &Value::F32(f32::from_bits(0xffc0_0000)),
            &canonical
        ));
        assert!(value_matches(
            &Value::F32(f32::from_bits(0x7fc0_1234)),
            &arithmetic
        ));
        assert!(!value_matches(
            &Value::F32(f32::from_bits(0x7f80_0001)),
            &arithmetic
        ));
        assert!(value_matches(&Value::F32(-0.0), &exact));
        assert!(!value_matches(&Value::F32(0.0), &exact));

        let f64_canonical = ExpectedValue::F64(NanPattern::CanonicalNan);
        assert!(value_matches(
            &Value::F64(f64::from_bits(0xfff8_0000_0000_0000)),
            &f64_canonical
        ));
    }

    #[test]
    fn converts_and_matches_vector_constants_by_lane_bits() {
        let vector = WastArg::Core(WastArgCore::V128(V128Const::I16x8([
            -1, 2, -3, 4, -5, 6, -7, 8,
        ])));
        let converted = convert_arg(&vector).unwrap();
        let expected = convert_core_expected(&WastRetCore::V128(V128Pattern::I16x8([
            -1, 2, -3, 4, -5, 6, -7, 8,
        ])))
        .unwrap();

        assert!(value_matches(&converted, &expected[0]));
        assert!(!value_matches(
            &converted,
            &ExpectedValue::V128(V128Pattern::I16x8([0; 8]))
        ));
    }

    #[test]
    fn matches_nan_patterns_in_vector_float_lanes() {
        let value = u128::from_le_bytes([
            0x00, 0x00, 0xc0, 0x7f, 0, 0, 0x80, 0x3f, 0, 0, 0x80, 0x3f, 0, 0, 0x80, 0x3f,
        ]);
        let expected = ExpectedValue::V128(V128Pattern::F32x4([
            NanPattern::ArithmeticNan,
            NanPattern::Value(F32 { bits: 0x3f80_0000 }),
            NanPattern::Value(F32 { bits: 0x3f80_0000 }),
            NanPattern::Value(F32 { bits: 0x3f80_0000 }),
        ]));

        assert!(value_matches(&Value::V128(value), &expected));
    }

    #[test]
    fn matches_null_reference_values() {
        assert!(value_matches(&Value::Unknown, &ExpectedValue::NullRef));
    }
}
