use std::fmt;

/// A mock could not be installed, checked, or restored.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Another session is active, including on the current thread.
    Busy,
    /// An address is null or outside a readable mapping.
    InvalidAddress,
    /// A byte range is empty, mismatched, or overflows the address space.
    InvalidRange,
    /// The target is not in readable executable memory.
    NotExecutable,
    /// Code has changed since it was inspected.
    MemoryChanged,
    /// Source and replacement have the same address.
    SameAddress,
    /// This target overlaps an existing replacement.
    Overlap,
    /// The function ends before there is room for a jump.
    InsufficientSpace,
    /// The function prefix contains an unsupported or invalid instruction.
    InvalidInstruction,
    /// This entry needs a call bridge that cannot run with shadow stacks enabled.
    ShadowStack,
    /// An operating system call failed.
    Os {
        /// Name of the failed operation.
        operation: &'static str,
        /// OS error code.
        code: i32,
    },
    /// The operating system's memory map could not be read or parsed.
    Mapping(String),
    /// A call or expectation did not meet the mock's rules.
    Expectation(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("another shimforge session is active"),
            Self::InvalidAddress => f.write_str("address is not readable"),
            Self::InvalidRange => f.write_str("invalid memory range"),
            Self::NotExecutable => f.write_str("address is not readable executable memory"),
            Self::MemoryChanged => f.write_str("function bytes changed unexpectedly"),
            Self::SameAddress => f.write_str("source and replacement have the same address"),
            Self::Overlap => f.write_str("target overlaps an active replacement"),
            Self::InsufficientSpace => f.write_str("function prefix is too short for a jump"),
            Self::InvalidInstruction => {
                f.write_str("function prefix contains an unsupported or invalid instruction")
            }
            Self::ShadowStack => {
                f.write_str("this function entry cannot be mocked with shadow stacks enabled")
            }
            Self::Os { operation, code } => write!(f, "{operation} failed (OS error {code})"),
            Self::Mapping(message) => write!(f, "cannot inspect memory mapping: {message}"),
            Self::Expectation(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// Returns the value, or panics with the error's message.
///
/// The panic reports the caller's location, so a failed setup points at the test.
#[doc(hidden)]
#[track_caller]
pub fn check<T>(result: Result<T, Error>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{error}"),
    }
}
