use anyhow::anyhow;
use wasmparser::{ConstExpr, Operator};

use crate::{HaltError, InlineWideValue, Value, macros::unsupported};


pub(crate) fn resolve_const_expr(expr: ConstExpr) -> Result<InlineWideValue, HaltError> {
    let operands = expr.get_operators_reader();
    for operand in operands {
        let operand = operand?;
        match operand {
            Operator::I32Const { value } => return Ok(InlineWideValue::new(Value(value as u64))),
            Operator::I64Const { value } => return Ok(InlineWideValue::new(Value(value as u64))),
            _ => {
                return Err(unsupported!(
                    "operand {:?} not supported for const exprs",
                    operand
                ));
            }
        }
    }
    Err(anyhow!("finished evaluating const expr but no return value").into())
}