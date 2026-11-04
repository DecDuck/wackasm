use anyhow::anyhow;
use wasmparser::{ConstExpr, Operator};

use crate::module::FunctionIndex;

#[derive(Debug)]
pub enum ConstValue {
    FuncRef(FunctionIndex),
    ConstI32(i32),
}

pub(crate) fn resolve_const_expr(expr: ConstExpr) -> Result<ConstValue, anyhow::Error> {
    let operands = expr.get_operators_reader();
    for operand in operands {
        let operand = operand?;
        match operand {
            Operator::I32Const { value } => return Ok(ConstValue::ConstI32(value)),
            _ => return Err(anyhow!("operand {:?} not supported for const exprs", operand)),
        }
    }
    Err(anyhow!("finished evaluating const expr but no return value"))
}