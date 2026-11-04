use std::{
    hash::{DefaultHasher, Hash as _, Hasher, SipHasher},
    mem::discriminant,
};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{
    HaltError, Value,
    macros::trap,
    module::BlockIndex,
    stack::{GlobalStack, MAX_OPERAND_STACK_SIZE, StackSection, basic::OperandStack},
};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FlatBlockAwareStack {
    stack: OperandStack,
    current_floor: usize,

    sections: Vec<(StackSection, usize)>,
}

impl FlatBlockAwareStack {
    pub fn new(sections: usize) -> Self {
        Self {
            stack: OperandStack::new(MAX_OPERAND_STACK_SIZE * (sections + 1)),
            current_floor: 0,
            sections: Vec::with_capacity(sections),
        }
    }
}

impl GlobalStack for FlatBlockAwareStack {
    fn push(&mut self, value: Value) -> Result<(), HaltError> {
        self.stack.push(value)
    }

    fn pop(&mut self) -> Result<Value, HaltError> {
        self.stack.pop()
    }

    fn peek(&mut self) -> Result<&Value, HaltError> {
        self.stack.peek()
    }

    fn push_section(
        &mut self,
        section: StackSection,
        pull_from_parent: Option<&[ValType]>,
    ) -> Result<(), crate::HaltError> {
        self.current_floor = self.stack.len() - pull_from_parent.map_or(0, |v| v.len());
        self.sections.push((section, self.current_floor));

        Ok(())
    }

    fn return_section(
        &mut self,
        preserve: &[ValType],
        position: Option<usize>,
    ) -> Result<Option<StackSection>, HaltError> {
        let pos = match position {
            Some(p) => p,
            None => match self.sections.len().checked_sub(1) {
                Some(p) => p,
                None => return Ok(None),
            },
        };
        let (section, floor) = self
            .sections
            .get(pos)
            .ok_or_else(|| anyhow!("invalid return_section position"))?
            .clone();

        let len = self.stack.len();
        if len < floor + preserve.len() {
            return Err(trap!("stack underflow"));
        }

        self.stack
            .get_mut()
            .copy_within(len - preserve.len()..len, floor);
        self.stack.set_len(floor + preserve.len());

        self.sections.truncate(pos);
        self.current_floor = self.sections.last().map_or(0, |v| v.1);

        Ok(Some(section))
    }

    fn reset_section(&mut self, preserve: &[ValType], pos: usize) -> Result<(), HaltError> {
        let floor = self
            .sections
            .get(pos)
            .ok_or_else(|| anyhow!("invalid reset_section position"))?
            .1;

        let len = self.stack.len();
        if len < floor + preserve.len() {
            return Err(trap!("stack underflow"));
        }

        self.stack
            .get_mut()
            .copy_within(len - preserve.len()..len, floor);
        self.stack.set_len(floor + preserve.len());

        self.sections.truncate(pos + 1);
        self.current_floor = floor;

        Ok(())
    }

    fn return_function(&mut self, preserve: &[ValType]) -> Result<Option<StackSection>, HaltError> {
        let Some(pos) = self
            .sections
            .iter()
            .rposition(|(s, _)| matches!(s, StackSection::Function(_)))
        else {
            return Ok(None);
        };

        self.return_section(preserve, Some(pos))
    }

    fn peek_section(&self, pos: Option<usize>) -> Option<&StackSection> {
        self.sections
            .get(pos.unwrap_or(self.sections.len().saturating_sub(1)))
            .map(|v| &v.0)
    }

    fn pull_results(&mut self, types: &[ValType]) -> Result<Vec<Value>, HaltError> {
        self.stack.pull_results(types)
    }

    fn len_sections(&self) -> usize {
        self.sections.len()
    }
}
