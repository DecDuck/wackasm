use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{
    HaltError, Value,
    macros::trap,
    stack::{MAX_OPERAND_STACK_SIZE, OperandStackFns},
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OperandStack {
    #[serde(with = "boxed_array")]
    inner: Box<[Value; MAX_OPERAND_STACK_SIZE]>,
    len: usize,
}

mod boxed_array {
    use serde::{Deserializer, Serializer};

    use crate::{Value, stack::MAX_OPERAND_STACK_SIZE};

    pub fn serialize<S>(
        value: &Box<[Value; MAX_OPERAND_STACK_SIZE]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_arrays::serialize(&**value, serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Box<[Value; MAX_OPERAND_STACK_SIZE]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_arrays::deserialize(deserializer).map(Box::new)
    }
}

impl OperandStackFns for OperandStack {
    fn push(&mut self, value: Value) -> Result<(), HaltError> {
        if self.len == MAX_OPERAND_STACK_SIZE {
            return Err(trap!("stack overflow"));
        }

        self.len += 1;
        self.inner[self.len - 1] = value;
        Ok(())
    }

    fn push_reversed_all(&mut self, mut values: Vec<Value>) -> Result<(), HaltError> {
        if values.is_empty() {
            return Ok(());
        }

        values.reverse();
        let start = self.len;
        let end = start
            .checked_add(values.len())
            .ok_or_else(|| trap!("stack overflow"))?;

        if end > MAX_OPERAND_STACK_SIZE {
            return Err(trap!("stack overflow"));
        }

        let destination = self
            .inner
            .get_mut(start..end)
            .ok_or_else(|| trap!("stack overflow"))?;
        destination.copy_from_slice(&values);

        self.len = end;

        Ok(())
    }

    fn pull_reversed_from_stack(
        &mut self,
        returns: &[ValType],
    ) -> Result<Vec<Value>, HaltError> {
        let mut results = Vec::with_capacity(returns.len());

        for rtn in returns.iter().rev() {
            results.push(match rtn {
                ValType::I32 => Value::I32(self.pop_i32()?),
                ValType::I64 => Value::I64(self.pop_i64()?),
                ValType::F32 => Value::F32(self.pop_f32()?),
                ValType::F64 => Value::F64(self.pop_f64()?),
                ValType::V128 => Value::V128(self.pop_v128()?),
                ValType::Ref(_) => self.pop()?, // todo proper validation
            });
        }

        Ok(results)
    }

    #[inline]
    fn pop(&mut self) -> Result<Value, HaltError> {
        if self.len == 0 {
            return Err(anyhow!("stack underflow").into());
        }

        self.len -= 1;
        Ok(self.inner[self.len]) // - 1 not required to access by index here because we -1 above
    }

    #[inline]
    fn peek(&mut self) -> Result<&Value, HaltError> {
        if self.len == 0 {
            return Err(anyhow!("stack underflow").into());
        }
        Ok(&self.inner[self.len - 1])
    }

    #[inline]
    fn pop_i32(&mut self) -> Result<i32, HaltError> {
        let value = self.pop()?;
        match value {
            Value::I32(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected i32").into()),
        }
    }

    #[inline]
    fn pop_i64(&mut self) -> Result<i64, HaltError> {
        let value = self.pop()?;
        match value {
            Value::I64(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected i64").into()),
        }
    }

    fn pop_f32(&mut self) -> Result<f32, HaltError> {
        let value = self.pop()?;
        match value {
            Value::F32(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected f32").into()),
        }
    }

    fn pop_f64(&mut self) -> Result<f64, HaltError> {
        let value = self.pop()?;
        match value {
            Value::F64(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected f64").into()),
        }
    }

    fn pop_v128(&mut self) -> Result<u128, HaltError> {
        let value = self.pop()?;
        match value {
            Value::V128(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected v128").into()),
        }
    }
}

impl OperandStack {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Box::new([Value::Unknown; MAX_OPERAND_STACK_SIZE]),
            len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::OperandStack;
    use crate::{Value, stack::OperandStackFns};

    #[test]
    fn pop_returns_the_only_pushed_value() {
        let mut stack = OperandStack::new(1);
        stack.push(Value::I32(7)).unwrap();

        assert!(matches!(stack.pop(), Ok(Value::I32(7))));
        assert!(stack.pop().is_err());
    }

    #[test]
    fn pop_returns_values_in_lifo_order() {
        let mut stack = OperandStack::new(2);
        stack.push(Value::I32(1)).unwrap();
        stack.push(Value::I32(2)).unwrap();

        assert!(matches!(stack.pop(), Ok(Value::I32(2))));
        assert!(matches!(stack.pop(), Ok(Value::I32(1))));
        assert!(stack.pop().is_err());
    }

    #[test]
    fn bulk_push_updates_the_stack_top() {
        let mut stack = OperandStack::new(2);
        stack
            .push_reversed_all(vec![Value::I32(2), Value::I32(1)])
            .unwrap();

        assert!(matches!(stack.pop(), Ok(Value::I32(2))));
        assert!(matches!(stack.pop(), Ok(Value::I32(1))));
        assert!(stack.pop().is_err());
    }

    #[test]
    fn pull_reversed_from_stack_extracts_float_and_vector_results() {
        let mut stack = OperandStack::new(4);
        stack.push(Value::F32(-0.0)).unwrap();
        stack.push(Value::F64(1.5)).unwrap();
        stack.push(Value::V128(0x1234)).unwrap();
        stack.push(Value::I32(7)).unwrap();

        let results = stack
            .pull_reversed_from_stack(&[
                wasmparser::ValType::F32,
                wasmparser::ValType::F64,
                wasmparser::ValType::V128,
                wasmparser::ValType::I32,
            ])
            .unwrap();

        assert!(matches!(results[0], Value::I32(7)));
        assert!(matches!(results[1], Value::V128(0x1234)));
        assert!(matches!(results[2], Value::F64(value) if value == 1.5));
        assert!(matches!(results[3], Value::F32(value) if value.to_bits() == (-0.0f32).to_bits()));
        assert!(stack.pop().is_err());
    }

    #[test]
    fn pull_reversed_from_stack_rejects_mismatched_typed_results() {
        let mut stack = OperandStack::new(1);
        stack.push(Value::I32(7)).unwrap();

        assert!(
            stack
                .pull_reversed_from_stack(&[wasmparser::ValType::F32])
                .is_err()
        );
    }
}
