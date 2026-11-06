/// Builds a WebAssembly-defined runtime trap.
macro_rules! trap {
    ($($arg:tt)*) => {
        $crate::HaltError::Trap(::anyhow::anyhow!($($arg)*))
    };
}

pub(crate) use trap;

/// Marks valid WebAssembly that wackasm does not implement yet.
macro_rules! unsupported {
    ($($arg:tt)*) => {
        $crate::HaltError::Unsupported(::anyhow::anyhow!($($arg)*))
    };
}

pub(crate) use unsupported;
