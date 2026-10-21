use wackasm::Value;
use wast::{
    core::{NanPattern, WastArgCore, WastRetCore},
    token::{F32, F64},
};

/// A v128 occupies two stack slots, but call arguments and results are one `Value` per type.
const V128_UNSUPPORTED: &str = "v128 arguments and results are not representable by the runtime";

#[derive(Debug)]
pub(super) enum ExpectedValue {
    I32(i32),
    I64(i64),
    F32(NanPattern<F32>),
    F64(NanPattern<F64>),
    NullRef,
    FuncRef,
}

pub(super) fn convert_arg(arg: &wast::WastArg<'_>) -> Result<Value, String> {
    match arg {
        wast::WastArg::Core(WastArgCore::I32(value)) => Ok(Value::from_i32(*value)),
        wast::WastArg::Core(WastArgCore::I64(value)) => Ok(Value::from_i64(*value)),
        wast::WastArg::Core(WastArgCore::F32(value)) => Ok(Value(value.bits as u64)),
        wast::WastArg::Core(WastArgCore::F64(value)) => Ok(Value(value.bits)),
        wast::WastArg::Core(WastArgCore::V128(_)) => Err(V128_UNSUPPORTED.to_owned()),
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
        WastRetCore::V128(_) => Err(V128_UNSUPPORTED.to_owned()),
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

/// Values are untyped slots, so the expected type decides how the bits are read.
pub(super) fn value_matches(actual: &Value, expected: &ExpectedValue) -> bool {
    match expected {
        ExpectedValue::I32(expected) => actual.as_i32() == *expected,
        ExpectedValue::I64(expected) => actual.as_i64() == *expected,
        ExpectedValue::F32(expected) => f32_matches(actual.0 as u32, *expected),
        ExpectedValue::F64(expected) => f64_matches(actual.0, *expected),
        ExpectedValue::NullRef => actual.is_null_ref(),
        ExpectedValue::FuncRef => !actual.is_null_ref(),
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
    fn reports_vector_values_as_unsupported() {
        let vector = WastArg::Core(WastArgCore::V128(V128Const::I16x8([
            -1, 2, -3, 4, -5, 6, -7, 8,
        ])));
        assert!(convert_arg(&vector).is_err());
        assert!(convert_core_expected(&WastRetCore::V128(V128Pattern::I16x8([0; 8]))).is_err());
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
