//! The wire format shared by the guest, this crate and the Java side.
//!
//! OpenComputers values are Lua-shaped (nil, booleans, numbers, strings,
//! byte strings, tables). They are encoded as a tag byte followed by a
//! little-endian payload:
//!
//! | tag | value  | payload                                   |
//! |-----|--------|-------------------------------------------|
//! | 0   | nil    | -                                         |
//! | 1   | false  | -                                         |
//! | 2   | true   | -                                         |
//! | 3   | int    | `i64`                                     |
//! | 4   | float  | `f64`                                     |
//! | 5   | string | `u32` byte length + UTF-8 bytes           |
//! | 6   | bytes  | `u32` byte length + raw bytes             |
//! | 7   | array  | `u32` element count + values              |
//! | 8   | map    | `u32` entry count + alternating key/value |
//!
//! `OcValueCodec.java` is the Java twin of this module; keep them in sync.

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

pub const TAG_NIL: u8 = 0;
pub const TAG_FALSE: u8 = 1;
pub const TAG_TRUE: u8 = 2;
pub const TAG_INT: u8 = 3;
pub const TAG_FLOAT: u8 = 4;
pub const TAG_STRING: u8 = 5;
pub const TAG_BYTES: u8 = 6;
pub const TAG_ARRAY: u8 = 7;
pub const TAG_MAP: u8 = 8;

/// Nesting limit when decoding, so a hostile guest can't blow the host stack.
pub const MAX_DEPTH: usize = 64;

// Serialize/Deserialize are only for persisting pending requests in machine
// snapshots. The wire format is `encode`/`decode`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OcValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Array(Vec<OcValue>),
    Map(Vec<(OcValue, OcValue)>),
}

impl OcValue {
    pub fn str(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            OcValue::Nil => out.push(TAG_NIL),
            OcValue::Bool(false) => out.push(TAG_FALSE),
            OcValue::Bool(true) => out.push(TAG_TRUE),
            OcValue::Int(v) => {
                out.push(TAG_INT);
                out.extend_from_slice(&v.to_le_bytes());
            }
            OcValue::Float(v) => {
                out.push(TAG_FLOAT);
                out.extend_from_slice(&v.to_le_bytes());
            }
            OcValue::String(s) => {
                out.push(TAG_STRING);
                put_len(out, s.len());
                out.extend_from_slice(s.as_bytes());
            }
            OcValue::Bytes(b) => {
                out.push(TAG_BYTES);
                put_len(out, b.len());
                out.extend_from_slice(b);
            }
            OcValue::Array(items) => {
                out.push(TAG_ARRAY);
                put_len(out, items.len());
                for item in items {
                    item.encode_into(out);
                }
            }
            OcValue::Map(entries) => {
                out.push(TAG_MAP);
                put_len(out, entries.len());
                for (k, v) in entries {
                    k.encode_into(out);
                    v.encode_into(out);
                }
            }
        }
    }

    /// Decodes exactly one value; trailing bytes are an error.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader { bytes, pos: 0 };
        let value = reader.value(0)?;
        if reader.pos != bytes.len() {
            bail!("{} trailing bytes after value", bytes.len() - reader.pos);
        }
        Ok(value)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            OcValue::String(s) => Some(s),
            _ => None,
        }
    }
}

fn put_len(out: &mut Vec<u8>, len: usize) {
    let len = u32::try_from(len).expect("value too large to encode");
    out.extend_from_slice(&len.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| anyhow!("value truncated at byte {}", self.pos))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<usize> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()) as usize)
    }

    fn u64(&mut self) -> Result<[u8; 8]> {
        Ok(self.take(8)?.try_into().unwrap())
    }

    /// Rejects counts that could not possibly fit in the remaining input
    /// (every element is at least one tag byte), so a forged count can't
    /// trigger a huge allocation.
    fn count(&mut self, min_bytes_each: usize) -> Result<usize> {
        let count = self.u32()?;
        let remaining = self.bytes.len() - self.pos;
        if count.saturating_mul(min_bytes_each) > remaining {
            bail!("element count {count} exceeds remaining input");
        }
        Ok(count)
    }

    fn value(&mut self, depth: usize) -> Result<OcValue> {
        if depth > MAX_DEPTH {
            bail!("value nested deeper than {MAX_DEPTH}");
        }
        let tag = self.take(1)?[0];
        Ok(match tag {
            TAG_NIL => OcValue::Nil,
            TAG_FALSE => OcValue::Bool(false),
            TAG_TRUE => OcValue::Bool(true),
            TAG_INT => OcValue::Int(i64::from_le_bytes(self.u64()?)),
            TAG_FLOAT => OcValue::Float(f64::from_le_bytes(self.u64()?)),
            TAG_STRING => {
                let len = self.u32()?;
                let raw = self.take(len)?;
                OcValue::String(
                    std::str::from_utf8(raw)
                        .map_err(|e| anyhow!("invalid UTF-8 in string: {e}"))?
                        .to_owned(),
                )
            }
            TAG_BYTES => {
                let len = self.u32()?;
                OcValue::Bytes(self.take(len)?.to_vec())
            }
            TAG_ARRAY => {
                let count = self.count(1)?;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(self.value(depth + 1)?);
                }
                OcValue::Array(items)
            }
            TAG_MAP => {
                let count = self.count(2)?;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let k = self.value(depth + 1)?;
                    let v = self.value(depth + 1)?;
                    entries.push((k, v));
                }
                OcValue::Map(entries)
            }
            other => bail!("unknown value tag {other}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let value = OcValue::Array(vec![
            OcValue::Nil,
            OcValue::Bool(true),
            OcValue::Bool(false),
            OcValue::Int(-42),
            OcValue::Float(1.5),
            OcValue::str("héllo"),
            OcValue::Bytes(vec![0, 255, 7]),
            OcValue::Map(vec![(OcValue::str("k"), OcValue::Int(1))]),
        ]);
        assert_eq!(OcValue::decode(&value.encode()).unwrap(), value);
    }

    #[test]
    fn rejects_malformed() {
        assert!(OcValue::decode(&[]).is_err());
        assert!(OcValue::decode(&[TAG_INT, 1, 2]).is_err());
        assert!(OcValue::decode(&[TAG_NIL, TAG_NIL]).is_err());
        assert!(OcValue::decode(&[99]).is_err());
        // Forged count far larger than the input.
        assert!(OcValue::decode(&[TAG_ARRAY, 0xff, 0xff, 0xff, 0x7f]).is_err());
        // Over-deep nesting.
        let mut deep = Vec::new();
        for _ in 0..=MAX_DEPTH + 1 {
            deep.extend_from_slice(&[TAG_ARRAY, 1, 0, 0, 0]);
        }
        deep.push(TAG_NIL);
        assert!(OcValue::decode(&deep).is_err());
    }
}
