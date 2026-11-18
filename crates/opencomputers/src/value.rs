//! Lua-shaped values, as components take and return them, and their wire
//! format (the same as `crates/wackasm-oc/src/value.rs` and `OcValueCodec.java`).
//!
//! Numbers stay integers when OpenComputers sends integers, so they round-trip
//! exactly; [`Number`] converts to either on request.

use std::{collections::BTreeMap, fmt};

const TAG_NIL: u8 = 0;
const TAG_FALSE: u8 = 1;
const TAG_TRUE: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_FLOAT: u8 = 4;
const TAG_STRING: u8 = 5;
const TAG_BYTES: u8 = 6;
const TAG_ARRAY: u8 = 7;
const TAG_MAP: u8 = 8;
const TAG_USERDATA: u8 = 9;
const MAX_DEPTH: usize = 64;

/// A Lua number: an integer, or a float when OpenComputers sends one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Int(i64),
    Float(f64),
}

impl Number {
    /// Truncates floats toward zero (saturating; NaN is 0).
    pub fn as_i64(self) -> i64 {
        match self {
            Number::Int(i) => i,
            Number::Float(f) => f as i64,
        }
    }

    pub fn as_i32(self) -> i32 {
        self.as_i64().clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    pub fn as_u32(self) -> u32 {
        self.as_i64().clamp(0, u32::MAX as i64) as u32
    }

    pub fn as_f64(self) -> f64 {
        match self {
            Number::Int(i) => i as f64,
            Number::Float(f) => f,
        }
    }
}

macro_rules! number_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Number {
            fn from(v: $t) -> Self { Number::Int(v as i64) }
        }
    )*};
}
number_from_int!(i8, i16, i32, i64, u8, u16, u32, usize, isize);

impl From<f64> for Number {
    fn from(v: f64) -> Self {
        Number::Float(v)
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Number::Int(i) => i.fmt(f),
            Number::Float(x) => x.fmt(f),
        }
    }
}

/// A Lua string: bytes that are usually, but not always, UTF-8 (file
/// contents, hashes and EEPROM code come back as binary).
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OcString(pub Vec<u8>);

impl OcString {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Text, with invalid UTF-8 replaced.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
}

impl std::ops::Deref for OcString {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Display for OcString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

impl fmt::Debug for OcString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(s) => s.fmt(f),
            None => write!(f, "b{:?}", self.0),
        }
    }
}

impl PartialEq<str> for OcString {
    fn eq(&self, other: &str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl PartialEq<&str> for OcString {
    fn eq(&self, other: &&str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl From<OcString> for String {
    fn from(s: OcString) -> Self {
        String::from_utf8(s.0).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }
}

/// A host object the guest holds by reference, such as an open file handle
/// from `filesystem.open`. Pass it back to the component that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle(pub u32);

/// Any value a component takes or returns.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Number(Number),
    String(OcString),
    Array(Vec<Value>),
    /// A Lua table with non-sequence keys, in the order OpenComputers sent them.
    Map(Vec<(Value, Value)>),
    Handle(Handle),
}

impl Value {
    /// A string argument: sent as text when it is UTF-8, as bytes otherwise.
    pub fn text(bytes: &[u8]) -> Value {
        Value::String(OcString(bytes.to_vec()))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<Number> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().map(Number::as_i64)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => s.as_str(),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_handle(&self) -> Option<Handle> {
        match self {
            Value::Handle(h) => Some(*h),
            _ => None,
        }
    }

    /// Elements of a sequence. OpenComputers sends some sequences as maps
    /// keyed 1..n; those are returned in key order.
    pub fn as_array(&self) -> Option<Vec<&Value>> {
        match self {
            Value::Array(items) => Some(items.iter().collect()),
            Value::Map(entries) => {
                let mut indexed: Vec<(i64, &Value)> =
                    entries.iter().filter_map(|(k, v)| Some((k.as_i64()?, v))).collect();
                if indexed.len() != entries.len() {
                    return None;
                }
                indexed.sort_by_key(|(k, _)| *k);
                Some(indexed.into_iter().map(|(_, v)| v).collect())
            }
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// `table.key` for string keys.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_map()?.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
    }

    /// The map as a `BTreeMap` with string keys; other keys are dropped.
    pub fn to_string_map(&self) -> BTreeMap<String, Value> {
        self.as_map()
            .unwrap_or_default()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.clone())))
            .collect()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        fn len(out: &mut Vec<u8>, n: usize) {
            out.extend_from_slice(&(n as u32).to_le_bytes());
        }
        match self {
            Value::Nil => out.push(TAG_NIL),
            Value::Bool(false) => out.push(TAG_FALSE),
            Value::Bool(true) => out.push(TAG_TRUE),
            Value::Number(Number::Int(i)) => {
                out.push(TAG_INT);
                out.extend_from_slice(&i.to_le_bytes());
            }
            Value::Number(Number::Float(f)) => {
                out.push(TAG_FLOAT);
                out.extend_from_slice(&f.to_bits().to_le_bytes());
            }
            Value::String(s) => {
                out.push(if s.as_str().is_some() { TAG_STRING } else { TAG_BYTES });
                len(out, s.len());
                out.extend_from_slice(s);
            }
            Value::Array(items) => {
                out.push(TAG_ARRAY);
                len(out, items.len());
                items.iter().for_each(|v| v.encode_into(out));
            }
            Value::Map(entries) => {
                out.push(TAG_MAP);
                len(out, entries.len());
                for (k, v) in entries {
                    k.encode_into(out);
                    v.encode_into(out);
                }
            }
            Value::Handle(Handle(id)) => {
                out.push(TAG_USERDATA);
                out.extend_from_slice(&id.to_le_bytes());
            }
        }
    }

    /// Decodes one value. `None` if the bytes are malformed.
    pub fn decode(bytes: &[u8]) -> Option<Value> {
        let mut r = Reader { bytes, pos: 0 };
        let v = r.value(0)?;
        (r.pos == bytes.len()).then_some(v)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn count(&mut self, min_each: usize) -> Option<usize> {
        let n = self.u32()? as usize;
        (n.saturating_mul(min_each) <= self.bytes.len() - self.pos).then_some(n)
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        Some(match *self.take(1)?.first()? {
            TAG_NIL => Value::Nil,
            TAG_FALSE => Value::Bool(false),
            TAG_TRUE => Value::Bool(true),
            TAG_INT => Value::Number(Number::Int(self.u64()? as i64)),
            TAG_FLOAT => Value::Number(Number::Float(f64::from_bits(self.u64()?))),
            TAG_STRING | TAG_BYTES => {
                let n = self.u32()? as usize;
                Value::String(OcString(self.take(n)?.to_vec()))
            }
            TAG_ARRAY => {
                let n = self.count(1)?;
                Value::Array((0..n).map(|_| self.value(depth + 1)).collect::<Option<_>>()?)
            }
            TAG_MAP => {
                let n = self.count(2)?;
                Value::Map((0..n).map(|_| Some((self.value(depth + 1)?, self.value(depth + 1)?))).collect::<Option<_>>()?)
            }
            TAG_USERDATA => Value::Handle(Handle(self.u32()?)),
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------- conversions

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl<T: Into<Number>> From<T> for Value {
    fn from(v: T) -> Self {
        Value::Number(v.into())
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::text(v.as_bytes())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(OcString(v.into_bytes()))
    }
}
impl From<&[u8]> for Value {
    fn from(v: &[u8]) -> Self {
        Value::text(v)
    }
}
impl From<OcString> for Value {
    fn from(v: OcString) -> Self {
        Value::String(v)
    }
}
impl From<Handle> for Value {
    fn from(v: Handle) -> Self {
        Value::Handle(v)
    }
}
impl From<&Handle> for Value {
    fn from(v: &Handle) -> Self {
        Value::Handle(*v)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Nil, Into::into)
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_shared_vector() {
        // Also asserted by crates/wackasm-oc/src/value.rs and OcValueCodecTest.java.
        let vector: &[u8] = &[7, 5, 0, 0, 0, 0, 2, 3, 7, 0, 0, 0, 0, 0, 0, 0, 5, 2, 0, 0, 0, b'h', b'i', 9, 1, 0, 0, 0];
        let value = Value::Array(vec![Value::Nil, true.into(), 7.into(), "hi".into(), Value::Handle(Handle(1))]);
        assert_eq!(value.encode(), vector);
        assert_eq!(Value::decode(vector), Some(value));
    }

    #[test]
    fn binary_strings_round_trip_as_bytes() {
        let v = Value::text(&[0xff, 0x00]);
        assert_eq!(v.encode()[0], TAG_BYTES);
        assert_eq!(Value::decode(&v.encode()), Some(v));
    }

    #[test]
    fn sequences_sent_as_maps() {
        let m = Value::Map(vec![(2.into(), "b".into()), (1.into(), "a".into())]);
        let items: Vec<_> = m.as_array().unwrap().into_iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(items, ["a", "b"]);
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(Value::decode(&[]), None);
        assert_eq!(Value::decode(&[TAG_INT, 1]), None);
        assert_eq!(Value::decode(&[TAG_ARRAY, 255, 255, 255, 127]), None);
    }
}
