//! Finding components and calling them.
//!
//! Every typed wrapper in [`crate::components`] implements [`Component`].
//! The untyped way, for anything the wrappers don't cover (another mod's
//! components, say), is [`Proxy`] or [`invoke`].

use std::fmt;

use crate::{
    error::{Error, Result},
    sys,
    value::{Handle, Number, OcString, Value},
};

/// A component's address (a UUID string).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address(pub String);

impl Address {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// So an address can be passed wherever a method takes one as a string.
impl AsRef<[u8]> for Address {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({})", self.0)
    }
}

impl From<&str> for Address {
    fn from(s: &str) -> Self {
        Address(s.to_owned())
    }
}

impl From<String> for Address {
    fn from(s: String) -> Self {
        Address(s)
    }
}

impl From<&Address> for Value {
    fn from(a: &Address) -> Self {
        Value::from(a.0.as_str())
    }
}

impl From<Address> for Value {
    fn from(a: Address) -> Self {
        Value::from(a.0)
    }
}

/// Calls `method` on the component at `address`.
pub fn invoke(address: &str, method: &str, args: Vec<Value>) -> Result<Vec<Value>> {
    let args = Value::Array(args).encode();
    let status = unsafe {
        sys::invoke(address.as_ptr(), address.len(), method.as_ptr(), method.len(), args.as_ptr(), args.len())
    };
    reply(status)
}

/// Calls `method` on a userdata value, such as the request object
/// `internet.request` returns (Lua's `handle.read()`). A value that was
/// released is [`Error::NoSuchComponent`].
pub fn invoke_value(value: Handle, method: &str, args: Vec<Value>) -> Result<Vec<Value>> {
    let args = Value::Array(args).encode();
    let status = unsafe { sys::invoke_value(value.0, method.as_ptr(), method.len(), args.as_ptr(), args.len()) };
    reply(status)
}

/// Like [`invoke`], but without blocking: queues the call and returns a future
/// for its result. Must be awaited inside [`crate::task::run`]. Requires the
/// `async` feature.
#[cfg(feature = "async")]
pub fn invoke_async(address: &str, method: &str, args: Vec<Value>) -> crate::task::Call {
    crate::task::Call::new(crate::task::Target::Component(address.to_owned()), method, args)
}

/// Like [`invoke_value`], but without blocking; see [`invoke_async`].
#[cfg(feature = "async")]
pub fn invoke_value_async(value: Handle, method: &str, args: Vec<Value>) -> crate::task::Call {
    crate::task::Call::new(crate::task::Target::Value(value.0), method, args)
}

/// Tells the host this program is done with `value`. The host disposes of it
/// (an open socket is closed) and the handle stops working. Values that are
/// never released stay alive until the computer stops.
pub fn release_value(value: Handle) {
    unsafe { sys::release_value(value.0) };
}

fn reply(status: i32) -> Result<Vec<Value>> {
    decode_reply(status, sys::response())
}

/// A call's result from its status and response bytes (also used for the
/// results of async calls).
pub(crate) fn decode_reply(status: i32, reply: Vec<u8>) -> Result<Vec<Value>> {
    match status {
        sys::STATUS_OK => match Value::decode(&reply) {
            Some(Value::Array(items)) => Ok(items),
            Some(Value::Nil) | None if reply.is_empty() => Ok(Vec::new()),
            Some(other) => Ok(vec![other]),
            None => Err(Error::UnexpectedReply("malformed value".into())),
        },
        sys::STATUS_NO_SUCH_COMPONENT => Err(Error::NoSuchComponent),
        sys::STATUS_NO_SUCH_METHOD => Err(Error::NoSuchMethod),
        _ => Err(Error::Failed(String::from_utf8_lossy(&reply).into_owned())),
    }
}

/// Every component whose type contains `filter` (all of them for `""`), as
/// `(address, type)` pairs sorted by address.
pub fn list(filter: &str) -> Vec<(Address, String)> {
    if unsafe { sys::components(filter.as_ptr(), filter.len()) } != sys::STATUS_OK {
        return Vec::new();
    }
    let mut found: Vec<(Address, String)> = match Value::decode(&sys::response()) {
        Some(Value::Map(entries)) => entries
            .into_iter()
            .filter_map(|(k, v)| Some((Address(k.as_str()?.to_owned()), v.as_str()?.to_owned())))
            .collect(),
        _ => Vec::new(),
    };
    found.sort();
    found
}

/// A method's attributes, from [`methods`].
#[derive(Clone, Debug)]
pub struct MethodInfo {
    pub name: String,
    /// Direct calls run immediately but draw on the per-tick call budget;
    /// others wait for the next server tick.
    pub direct: bool,
    pub doc: String,
}

/// The methods a component offers, sorted by name.
pub fn methods(address: &str) -> Result<Vec<MethodInfo>> {
    match unsafe { sys::methods(address.as_ptr(), address.len()) } {
        sys::STATUS_OK => {}
        sys::STATUS_NO_SUCH_COMPONENT => return Err(Error::NoSuchComponent),
        _ => return Err(Error::Failed(String::from_utf8_lossy(&sys::response()).into_owned())),
    }
    let Some(Value::Map(entries)) = Value::decode(&sys::response()) else {
        return Err(Error::UnexpectedReply("methods".into()));
    };
    let mut methods: Vec<MethodInfo> = entries
        .into_iter()
        .filter_map(|(name, info)| {
            Some(MethodInfo {
                name: name.as_str()?.to_owned(),
                direct: info.get("direct").and_then(Value::as_bool).unwrap_or(false),
                doc: info.get("doc").and_then(Value::as_str).unwrap_or_default().to_owned(),
            })
        })
        .collect();
    methods.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(methods)
}

/// A typed component wrapper.
pub trait Component: Sized {
    /// The OpenComputers component type, such as `"gpu"`.
    const KIND: &'static str;

    /// Wraps an address without checking what is there.
    fn from_address(address: Address) -> Self;

    fn address(&self) -> &Address;

    /// The first component of this type, by address order.
    fn first() -> Option<Self> {
        Self::all().into_iter().next()
    }

    /// Every component of this type, by address order.
    fn all() -> Vec<Self> {
        list(Self::KIND)
            .into_iter()
            .filter(|(_, kind)| kind == Self::KIND)
            .map(|(address, _)| Self::from_address(address))
            .collect()
    }

    /// The component at `address`, if it is of this type.
    fn at(address: impl Into<Address>) -> Option<Self> {
        let address = address.into();
        list(Self::KIND)
            .into_iter()
            .any(|(a, kind)| a == address && kind == Self::KIND)
            .then(|| Self::from_address(address))
    }

    /// An untyped call, for methods the wrapper doesn't know.
    fn invoke(&self, method: &str, args: Vec<Value>) -> Result<Vec<Value>> {
        invoke(self.address().as_str(), method, args)
    }

    /// [`Component::invoke`] without blocking; see [`invoke_async`].
    #[cfg(feature = "async")]
    fn invoke_async(&self, method: &str, args: Vec<Value>) -> crate::task::Call {
        invoke_async(self.address().as_str(), method, args)
    }
}

/// Any component, called by method name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Proxy {
    pub address: Address,
    pub kind: String,
}

impl Proxy {
    pub fn all() -> Vec<Proxy> {
        list("").into_iter().map(|(address, kind)| Proxy { address, kind }).collect()
    }

    pub fn invoke(&self, method: &str, args: Vec<Value>) -> Result<Vec<Value>> {
        invoke(self.address.as_str(), method, args)
    }

    pub fn methods(&self) -> Result<Vec<MethodInfo>> {
        methods(self.address.as_str())
    }
}

// ---------------------------------------------------------------- replies

/// Turns a reply into a method's documented return type. OpenComputers
/// methods report failure as `nil, "reason"` (or `false, "reason"` for some
/// boolean methods); decoding into a non-optional type turns the first into
/// [`Error::Failed`].
pub trait FromReply: Sized {
    fn from_reply(reply: Vec<Value>) -> Result<Self>;
}

/// One returned value.
pub trait FromValue: Sized {
    const EXPECTED: &'static str;
    fn from_value(value: Value) -> Option<Self>;
}

impl FromValue for Value {
    const EXPECTED: &'static str = "any";
    fn from_value(value: Value) -> Option<Self> {
        Some(value)
    }
}

impl FromValue for bool {
    const EXPECTED: &'static str = "boolean";
    fn from_value(value: Value) -> Option<Self> {
        value.as_bool()
    }
}

impl FromValue for Number {
    const EXPECTED: &'static str = "number";
    fn from_value(value: Value) -> Option<Self> {
        value.as_number()
    }
}

impl FromValue for OcString {
    const EXPECTED: &'static str = "string";
    fn from_value(value: Value) -> Option<Self> {
        match value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

impl FromValue for Handle {
    const EXPECTED: &'static str = "userdata";
    fn from_value(value: Value) -> Option<Self> {
        value.as_handle()
    }
}

impl<T: FromValue> FromValue for Option<T> {
    const EXPECTED: &'static str = T::EXPECTED;
    fn from_value(value: Value) -> Option<Self> {
        match value {
            Value::Nil => Some(None),
            v => T::from_value(v).map(Some),
        }
    }
}

fn failure(reply: &[Value]) -> Option<Error> {
    match reply {
        [Value::Nil | Value::Bool(false), Value::String(reason), ..] => Some(Error::Failed(reason.to_string_lossy())),
        _ => None,
    }
}

fn take<T: FromValue>(reply: &[Value], index: usize) -> Result<T> {
    let value = reply.get(index).cloned().unwrap_or_default();
    if let Some(v) = T::from_value(value.clone()) {
        return Ok(v);
    }
    if let Some(error) = failure(reply) {
        return Err(error);
    }
    Err(Error::UnexpectedReply(format!("expected {} at position {}, got {value:?}", T::EXPECTED, index + 1)))
}

impl FromReply for () {
    fn from_reply(reply: Vec<Value>) -> Result<Self> {
        match failure(&reply) {
            Some(error) if matches!(reply.first(), Some(Value::Nil)) => Err(error),
            _ => Ok(()),
        }
    }
}

impl FromReply for Vec<Value> {
    fn from_reply(reply: Vec<Value>) -> Result<Self> {
        Ok(reply)
    }
}

macro_rules! reply_tuple {
    ($($t:ident $i:tt),+) => {
        impl<$($t: FromValue),+> FromReply for ($($t,)+) {
            fn from_reply(reply: Vec<Value>) -> Result<Self> {
                Ok(($(take::<$t>(&reply, $i)?,)+))
            }
        }
    };
}
reply_tuple!(A 0);
reply_tuple!(A 0, B 1);
reply_tuple!(A 0, B 1, C 2);
reply_tuple!(A 0, B 1, C 2, D 3);
reply_tuple!(A 0, B 1, C 2, D 3, E 4);

/// Single-value replies decode straight into `T`, not `(T,)`.
pub(crate) fn single<T: FromValue>(reply: Vec<Value>) -> Result<T> {
    take(&reply, 0)
}

/// Builds an argument list, dropping trailing omitted optionals so the
/// component applies its own defaults.
pub(crate) fn args(values: Vec<Option<Value>>) -> Vec<Value> {
    let keep = values.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
    values.into_iter().take(keep).map(Option::unwrap_or_default).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_become_errors() {
        let reply = vec![Value::Nil, "no space".into()];
        assert_eq!(single::<Number>(reply.clone()), Err(Error::Failed("no space".into())));
        assert_eq!(single::<Option<Number>>(reply), Ok(None));
        let refused = vec![false.into(), "blocked".into()];
        assert_eq!(single::<bool>(refused), Ok(false));
    }

    #[test]
    fn tuples_and_trailing_optionals() {
        let reply = vec![50.into(), 16.into()];
        let (w, h): (Number, Number) = FromReply::from_reply(reply).unwrap();
        assert_eq!((w.as_i64(), h.as_i64()), (50, 16));
        assert_eq!(args(vec![Some(1.into()), None, Some(2.into()), None]), vec![1.into(), Value::Nil, 2.into()]);
    }
}
