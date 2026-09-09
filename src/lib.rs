#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

#[cfg(not(all(
    target_arch = "x86_64",
    any(target_os = "windows", target_os = "linux")
)))]
compile_error!("shimforge supports only Windows x86-64 and Linux x86-64");

mod code;
mod error;
mod memory;

#[cfg(test)]
mod tests;

pub use error::Error;

use std::sync::{Mutex, MutexGuard, TryLockError};

static SESSION: Mutex<()> = Mutex::new(());

struct Patch {
    entry: usize,
    address: usize,
    original: Vec<u8>,
    replacement: Vec<u8>,
}

/// An exclusive scope for process-wide function replacements.
///
/// All replacements are restored in reverse order when the session is dropped,
/// including during unwinding. A session cannot move to another thread.
#[must_use = "keep the session alive while exercising the replacements"]
pub struct Session {
    patches: Vec<Patch>,
    _lock: MutexGuard<'static, ()>,
}

impl Session {
    /// Opens a session, or returns [`Error::Busy`] without blocking.
    ///
    /// A session serializes other shimforge sessions, not calls to target functions.
    pub fn new() -> Result<Self, Error> {
        let lock = match SESSION.try_lock() {
            Ok(lock) => lock,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(Error::Busy),
        };
        Ok(Self {
            patches: Vec::new(),
            _lock: lock,
        })
    }

    /// Installs a replacement at a raw function entry point.
    ///
    /// Prefer [`replace!`] to check function signatures at compile time.
    ///
    /// # Safety
    ///
    /// Both pointers must be live function entry points with exactly compatible
    /// signatures, ABIs, and lifetime requirements for **every** call to the source.
    /// Their code and mappings must remain live and unchanged until restoration.
    /// No thread may execute the overwritten prefix during installation or
    /// restoration (including session drop). No branch may enter the middle of
    /// that prefix. The source must have a distinct, non-inlined entry point.
    /// Do not replace functions used by shimforge, its allocator, or OS backend.
    /// Replacement effects must respect any invariants required by source callers.
    pub unsafe fn replace_raw(
        &mut self,
        source: *const (),
        target: *const (),
    ) -> Result<(), Error> {
        let source = source as usize;
        let target = target as usize;
        if source == target {
            return Err(Error::SameAddress);
        }
        // Reject aliases and replacement cycles before inspecting patched instructions.
        if self.patches.iter().any(|patch| {
            let end = patch.address + patch.original.len();
            (patch.entry <= source && source < end) || (patch.entry <= target && target < end)
        }) {
            return Err(Error::Overlap);
        }
        memory::read(target, 1)?;
        let bytes = memory::read(source, code::MAX_PREFIX)?;
        let plan = code::plan(source, target, &bytes)?;
        let address = source.checked_add(plan.offset).ok_or(Error::InvalidRange)?;
        let end = address
            .checked_add(plan.original.len())
            .ok_or(Error::InvalidRange)?;
        if self
            .patches
            .iter()
            .any(|patch| address < patch.address + patch.original.len() && patch.address < end)
        {
            return Err(Error::Overlap);
        }
        // Allocate all ownership bookkeeping before changing executable memory.
        self.patches.reserve(1);
        // SAFETY: the caller guarantees quiescence and the validity of these functions.
        unsafe {
            memory::write(address, &plan.original, &plan.replacement)?;
        }
        self.patches.push(Patch {
            entry: source,
            address,
            original: plan.original,
            replacement: plan.replacement,
        });
        Ok(())
    }

    /// Restores all replacements, newest first. Safe to call repeatedly.
    ///
    /// The quiescence requirements of [`Self::replace_raw`] still apply. On error,
    /// the affected replacement remains tracked so restoration can be retried.
    pub fn restore(&mut self) -> Result<(), Error> {
        while let Some(patch) = self.patches.last() {
            // SAFETY: installation's contract extends through restoration and drop.
            unsafe {
                memory::write(patch.address, &patch.replacement, &patch.original)?;
            }
            self.patches.pop();
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        finish(self.restore(), std::process::abort);
    }
}

fn finish(result: Result<(), Error>, fatal: fn() -> !) {
    if result.is_err() {
        // Continuing could execute a stale replacement. Never panic during unwinding.
        fatal();
    }
}

/// Replaces a function using an explicit, compiler-checked function pointer type.
///
/// ```no_run
/// # fn original(x: i32) -> i32 { x + 1 }
/// # fn fake(x: i32) -> i32 { x + 10 }
/// let mut session = shimforge::Session::new()?;
/// shimforge::replace!(session, original => fake, fn(i32) -> i32)?;
/// # Ok::<(), shimforge::Error>(())
/// ```
///
/// Low-level memory operations are encapsulated by this macro. The runtime
/// requirements in the crate documentation still apply. A narrower lifetime
/// annotation must not be used to make an incompatible replacement compile.
/// Non-capturing closures are accepted.
///
/// Incompatible signatures are rejected:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// fn source(x: u32) -> u32 { x }
/// shimforge::replace!(session, source => |x: u64| x, fn(u32) -> u32);
/// ```
/// Capturing closures cannot outlive their captured state through a code patch:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// let captured = String::from("hello");
/// fn source() -> usize { 1 }
/// shimforge::replace!(session, source => || captured.len(), fn() -> usize);
/// ```
#[macro_export]
macro_rules! replace {
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source: $(for<$($lt),+>)? fn($($arg),*) $(-> $ret)? = $source;
        let target: $(for<$($lt),+>)? fn($($arg),*) $(-> $ret)? = $target;
        let session = &mut $session;
        // SAFETY: signatures are checked above; runtime requirements are documented.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? unsafe fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source: $(for<$($lt),+>)? unsafe fn($($arg),*) $(-> $ret)? = $source;
        let target: $(for<$($lt),+>)? unsafe fn($($arg),*) $(-> $ret)? = $target;
        let session = &mut $session;
        // SAFETY: signatures are checked above; runtime requirements are documented.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? extern $abi:literal fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source: $(for<$($lt),+>)? extern $abi fn($($arg),*) $(-> $ret)? = $source;
        let target: $(for<$($lt),+>)? extern $abi fn($($arg),*) $(-> $ret)? = $target;
        let session = &mut $session;
        // SAFETY: signatures are checked above; runtime requirements are documented.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? unsafe extern $abi:literal fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source: $(for<$($lt),+>)? unsafe extern $abi fn($($arg),*) $(-> $ret)? = $source;
        let target: $(for<$($lt),+>)? unsafe extern $abi fn($($arg),*) $(-> $ret)? = $target;
        let session = &mut $session;
        // SAFETY: signatures are checked above; runtime requirements are documented.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
}
