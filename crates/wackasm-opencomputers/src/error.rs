use std::fmt;

/// Why a component call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// No component with that address is connected to this computer.
    NoSuchComponent,
    /// The component has no such method.
    NoSuchMethod,
    /// The component refused the call (a bad argument, or an exception
    /// inside it), or reported failure the Lua way: `nil, "reason"`.
    Failed(String),
    /// The reply wasn't the shape the method's documentation promised.
    UnexpectedReply(String),
    /// No program by that name on any disk (see [`crate::process::Command`]).
    ProgramNotFound(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoSuchComponent => f.write_str("no such component"),
            Error::NoSuchMethod => f.write_str("no such method"),
            Error::Failed(reason) => f.write_str(reason),
            Error::UnexpectedReply(what) => write!(f, "unexpected reply: {what}"),
            Error::ProgramNotFound(name) => write!(f, "{name}: command not found"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
