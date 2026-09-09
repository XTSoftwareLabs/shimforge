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

/// A scope for function replacements that affect all threads.
///
/// Drop restores replacements in reverse order, including during a panic.
/// A session cannot move to another thread.
#[must_use = "keep the session alive while using the mocks"]
pub struct Session {
    patches: Vec<Patch>,
    _lock: MutexGuard<'static, ()>,
}

impl Session {
    /// Opens a session, or returns [`Error::Busy`] without blocking.
    ///
    /// Only one session can be active. Its lock does not stop function calls.
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
    /// Both pointers must name live functions with matching signatures, ABIs,
    /// and lifetimes for every caller. Keep their code loaded and unchanged
    /// until restored. No thread may run the patched bytes during installation
    /// or restoration, including drop. No branch may enter those bytes midway.
    /// Calls must reach the source entry. Inlined or merged calls cannot be isolated.
    /// Do not replace functions used by shimforge or its memory and OS code.
    /// The replacement must meet the safety rules its callers rely on.
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
        // Check for aliases and cycles before decoding patched bytes.
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
        // Reserve space before changing code so push cannot allocate.
        self.patches.reserve(1);
        // SAFETY: the caller keeps both functions loaded and idle during the write.
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
    /// Keep target calls stopped as required by [`Self::replace_raw`]. On error,
    /// the patch stays in the session so you can retry.
    pub fn restore(&mut self) -> Result<(), Error> {
        while let Some(patch) = self.patches.last() {
            // SAFETY: replace_raw requires the target to stay loaded and idle here.
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
        // Abort if a patch cannot be removed, even during a panic.
        fatal();
    }
}

/// Replaces a function and checks its signature at compile time.
///
/// ```no_run
/// # fn original(x: i32) -> i32 { x + 1 }
/// # fn fake(x: i32) -> i32 { x + 10 }
/// let mut session = shimforge::Session::new()?;
/// shimforge::replace!(session, original => fake, fn(i32) -> i32)?;
/// # Ok::<(), shimforge::Error>(())
/// ```
///
/// No `unsafe` block is needed. Follow the crate's safety rules. Do not narrow
/// lifetimes to force a type match. Closures without captures are accepted.
///
/// Incompatible signatures are rejected:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// fn source(x: u32) -> u32 { x }
/// shimforge::replace!(session, source => |x: u64| x, fn(u32) -> u32);
/// ```
/// The source must also match:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// fn source(x: u64) -> u64 { x }
/// shimforge::replace!(session, source => |x| x, fn(u32) -> u32);
/// ```
/// Calling conventions must match:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// extern "C" fn source(x: u32) -> u32 { x }
/// shimforge::replace!(session, source => |x| x, fn(u32) -> u32);
/// ```
/// Capturing closures are rejected:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// let captured = String::from("hello");
/// fn source() -> usize { 1 }
/// shimforge::replace!(session, source => || captured.len(), fn() -> usize);
/// ```
#[macro_export]
macro_rules! replace {
    (@infer $type:ty) => { _ };
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source = $source as fn($($crate::replace!(@infer $arg)),*) -> _;
        let target: $(for<$($lt),+>)? fn($($arg),*) $(-> $ret)? = $target;
        // Infer source lifetimes, then require matching pointer types.
        fn checked<T>(source: T, _: T) -> T { source }
        let source = checked(source, target);
        let session = &mut $session;
        // SAFETY: types are checked above; callers must follow the runtime safety rules.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? unsafe fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source = $source as unsafe fn($($crate::replace!(@infer $arg)),*) -> _;
        let target: $(for<$($lt),+>)? unsafe fn($($arg),*) $(-> $ret)? = $target;
        // Infer source lifetimes, then require matching pointer types.
        fn checked<T>(source: T, _: T) -> T { source }
        let source = checked(source, target);
        let session = &mut $session;
        // SAFETY: types are checked above; callers must follow the runtime safety rules.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? extern $abi:literal fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source = $source as extern $abi fn($($crate::replace!(@infer $arg)),*) -> _;
        let target: $(for<$($lt),+>)? extern $abi fn($($arg),*) $(-> $ret)? = $target;
        // Infer source lifetimes, then require matching pointer types.
        fn checked<T>(source: T, _: T) -> T { source }
        let source = checked(source, target);
        let session = &mut $session;
        // SAFETY: types are checked above; callers must follow the runtime safety rules.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? unsafe extern $abi:literal fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source = $source as unsafe extern $abi fn($($crate::replace!(@infer $arg)),*) -> _;
        let target: $(for<$($lt),+>)? unsafe extern $abi fn($($arg),*) $(-> $ret)? = $target;
        // Infer source lifetimes, then require matching pointer types.
        fn checked<T>(source: T, _: T) -> T { source }
        let source = checked(source, target);
        let session = &mut $session;
        // SAFETY: types are checked above; callers must follow the runtime safety rules.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
}
