use serde::{Deserialize, Serialize};

use crate::{compiler::WackasmOperator, dispatch::WackasmOperatorHandler};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "WackasmOperator", into = "WackasmOperator")]
pub(crate) struct RuntimeWackasmOperator {
    pub(crate) handler: WackasmOperatorHandler,
    pub(crate) op: WackasmOperator,
}

impl From<WackasmOperator> for RuntimeWackasmOperator {
    fn from(op: WackasmOperator) -> Self {
        Self { handler: crate::dispatch::handler_for(&op), op }
    }
}
impl From<RuntimeWackasmOperator> for WackasmOperator {
    fn from(op: RuntimeWackasmOperator) -> Self { op.op }
}

const _: () = assert!(std::mem::size_of::<RuntimeWackasmOperator>() <= 24);