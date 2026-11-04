use std::marker::PhantomData;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{
    HaltError, Value,
    compiler::DropKeep,
    macros::trap,
    stack::{GlobalStack, RuntimeStack, StackHandle, val_types_to_size},
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OperandStack {
    inner: Vec<Value>,
    len: usize,
}

impl GlobalStack for OperandStack {
    fn reconcile(&mut self, handle: *mut Value) {
        let start = self.inner.as_ptr();
        let len = unsafe { handle.offset_from_unsigned(start) };
        debug_assert!(len <= self.inner.len(), "stack handle ran past the buffer");
        self.len = len;
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    fn clear(&mut self) {
        self.len = 0;
    }

    #[inline(always)]
    fn space(&self) -> usize {
        self.inner.len() - self.len
    }

    #[inline(always)]
    fn push_all(&mut self, values: &[Value]) {
        let end = self.len + values.len();
        self.inner[self.len..end].copy_from_slice(values);
        self.len = end;
    }

    #[inline(always)]
    fn unwind_to(&mut self, target: usize, keep: usize) {
        let len = self.len;
        self.inner.copy_within(len - keep..len, target);
        self.len = target + keep;
    }

    #[inline(always)]
    fn pull_results(&mut self, types: &[ValType]) -> Result<Vec<Value>, HaltError> {
        let type_len = val_types_to_size(types);
        let start = self
            .len
            .checked_sub(type_len)
            .ok_or_else(|| trap!("stack underflow"))?;
        let end = self.len;
        self.len = start;
        Ok(self
            .inner
            .get(start..end)
            .ok_or_else(|| trap!("stack under or overflow"))?
            .iter()
            .cloned()
            .collect())
    }
}

#[repr(transparent)]
pub(crate) struct LocalOperandStack<'a> {
    inner: *mut Value,
    _borrow: PhantomData<&'a mut OperandStack>,
}

const _: () = assert!(size_of::<LocalOperandStack<'static>>() == size_of::<*mut Value>());

impl<'a> RuntimeStack for LocalOperandStack<'a> {
    fn into_raw(self) -> *mut Value {
        self.inner
    }

    #[inline(always)]
    fn push(&mut self, value: Value) {
        unsafe {
            self.inner.write(value);
            self.inner = self.inner.add(1);
        }
    }

    #[inline(always)]
    fn pop(&mut self) -> Value {
        unsafe {
            self.inner = self.inner.sub(1);
            *self.inner // - 1 not required to access by index here because we -1 above
        }
    }

    #[inline(always)]
    fn peek(&mut self) -> &Value {
        unsafe { &*self.inner.sub(1) }
    }

    #[inline(always)]
    fn peek_wide(&mut self) -> u128 {
        unsafe {
            let low = (*self.inner.sub(2)).0 as u128;
            let high = (*self.inner.sub(1)).0 as u128;
            (high << 64) | low
        }
    }

    #[inline(always)]
    fn peek_mut(&mut self) -> &mut Value {
        unsafe { &mut *self.inner.sub(1) }
    }

    #[inline(always)]
    fn drop_keep(&mut self, dk: DropKeep) {
        let (drop, keep) = (dk.drop as usize, dk.keep as usize);
        unsafe {
            match keep {
                // Move nothing
                0 => {}
                // Move a single value down
                1 => {
                    self.inner
                        .sub(drop + keep)
                        .write(self.inner.sub(keep).read());
                }
                _ => {
                    let src = self.inner.sub(keep);
                    let dst = self.inner.sub(keep + drop);
                    let count = keep;
                    core::ptr::copy(src, dst, count);
                }
            }
            self.inner = self.inner.sub(drop);
        }
    }
}

#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct Locals<'a> {
    base: *mut Value,
    _borrow: PhantomData<&'a mut OperandStack>,
}

impl Locals<'_> {
    #[inline(always)]
    pub fn get(self, slot: u32) -> Value {
        unsafe { self.base.add(slot as usize).read() }
    }
    #[inline(always)]
    pub fn set(self, slot: u32, value: Value) {
        unsafe { self.base.add(slot as usize).write(value) }
    }
    #[inline(always)]
    pub fn get_wide(self, slot: u32) -> u128 {
        (self.get(slot).0 as u128) | ((self.get(slot + 1).0 as u128) << 64)
    }
    #[inline(always)]
    pub fn set_wide(self, slot: u32, v: u128) {
        self.set(slot, Value(v as u64));
        self.set(slot + 1, Value((v >> 64) as u64));
    }
}

impl OperandStack {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: vec![Value(u64::MAX); capacity],
            len: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn handle<'a>(
        &'a mut self,
        local_base: usize,
    ) -> (LocalOperandStack<'a>, Locals<'a>) {
        let pointer = unsafe { self.inner.as_mut_ptr() };
        (
            LocalOperandStack {
                inner: unsafe { pointer.add(self.len) },
                _borrow: PhantomData,
            },
            Locals {
                base: unsafe { pointer.add(local_base) },
                _borrow: PhantomData,
            },
        )
    }
}
