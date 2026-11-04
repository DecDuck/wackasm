use anyhow::anyhow;
use wasmparser::{ConstExpr, Operator};

use crate::{Value, module::FunctionIndex};


pub(crate) fn resolve_const_expr(expr: ConstExpr) -> Result<Value, anyhow::Error> {
    let operands = expr.get_operators_reader();
    for operand in operands {
        let operand = operand?;
        match operand {
            Operator::I32Const { value } => return Ok(Value::I32(value)),
            _ => return Err(anyhow!("operand {:?} not supported for const exprs", operand)),
        }
    }
    Err(anyhow!("finished evaluating const expr but no return value"))
}