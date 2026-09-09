#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

#[cfg(not(all(
    target_arch = "x86_64",
    any(target_os = "windows", target_os = "linux")
)))]
compile_error!("shimforge supports only Windows x86-64 and Linux x86-64");

mod asynchronous;
mod code;
mod error;
mod expectation;
mod memory;

#[cfg(test)]
mod tests;

pub use asynchronous::{AsyncExpectation, AsyncMock};
pub use error::Error;
pub use expectation::{CallCount, Expectation, Sequence};

#[doc(hidden)]
pub use shimforge_macros::__mock;

#[doc(hidden)]
pub mod __private {
    pub use crate::expectation::{CallGuard, Config, Control, Meta, Rule, State, lock};
}

use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

static SESSION: Mutex<()> = Mutex::new(());

struct Patch {
    entry: usize,
    address: usize,
    original: Vec<u8>,
    replacement: Vec<u8>,
}

struct OwnedMock {
    control: Arc<dyn expectation::Control>,
    detach: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for OwnedMock {
    fn drop(&mut self) {
        (self.detach.take().expect("mock was already detached"))();
        self.control.deactivate();
    }
}

/// A scope for function replacements that affect all threads.
///
/// Drop restores replacements in reverse order, including during a panic.
/// A session cannot move to another thread.
#[must_use = "keep the session alive while using the mocks"]
pub struct Session {
    patches: Vec<Patch>,
    mocks: Vec<OwnedMock>,
    _lock: MutexGuard<'static, ()>,
}

impl Session {
    #[doc(hidden)]
    pub fn __borrow(&mut self) -> &mut Self {
        self
    }

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
            mocks: Vec::new(),
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

    /// Checks all call expectations without removing the mocks.
    pub fn verify(&self) -> Result<(), Error> {
        for mock in &self.mocks {
            mock.control.verify()?;
        }
        Ok(())
    }

    #[doc(hidden)]
    /// # Safety
    /// Follow `replace_raw` rules. Detach must release the matching mock state.
    pub unsafe fn __install(
        &mut self,
        source: *const (),
        target: *const (),
        control: Arc<dyn expectation::Control>,
        detach: Box<dyn FnOnce() + Send>,
    ) -> Result<(), Error> {
        let mock = OwnedMock {
            control,
            detach: Some(detach),
        };
        self.mocks.reserve(1);
        // SAFETY: the caller provides matching functions and keeps them idle.
        unsafe {
            self.replace_raw(source, target)?;
        }
        self.mocks.push(mock);
        Ok(())
    }

    /// Restores all replacements, then checks their expectations.
    ///
    /// Keep target calls stopped as required by [`Self::replace_raw`]. On error,
    /// a failed patch stays in the session so you can retry. Expectation errors
    /// are returned after all patches have been removed.
    pub fn restore(&mut self) -> Result<(), Error> {
        self.restore_patches()?;
        let result = self.verify();
        self.detach();
        result
    }

    fn detach(&mut self) {
        self.mocks.clear();
    }

    fn restore_patches(&mut self) -> Result<(), Error> {
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
        finish(self.restore_patches(), std::process::abort);
        let result = self.verify();
        self.detach();
        if !std::thread::panicking() {
            if let Err(error) = result {
                panic!("{error}");
            }
        }
    }
}

/// Creates a mock with argument matching and checked call counts.
///
/// ```
/// fn read_count(key: &str) -> usize { key.len() }
/// let mut session = shimforge::Session::new()?;
/// let mock = shimforge::mock!(session, read_count, fn(&str) -> usize)?;
/// mock.expect().with(|key| *key == "orders").once().returns(12)?;
/// assert_eq!(read_count("orders"), 12);
/// # Ok::<(), shimforge::Error>(())
/// ```
///
/// Matchers borrow arguments. Return closures may capture owned values and must
/// be `Send + 'static`. Follow the same runtime safety rules as [`replace!`].
///
/// The source signature must match:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// fn source(value: u64) -> u64 { value }
/// shimforge::mock!(session, source, fn(u32) -> u32);
/// ```
/// Captures must be safe to send between threads:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// fn source() -> usize { 1 }
/// let mock = shimforge::mock!(session, source, fn() -> usize).unwrap();
/// let value = std::rc::Rc::new(2);
/// mock.expect().returning(move || *value);
/// ```
/// Captures must outlive the test's stack:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// fn source() -> usize { 1 }
/// let mock = shimforge::mock!(session, source, fn() -> usize).unwrap();
/// let value = String::from("token");
/// mock.expect().returning(|| value.len());
/// ```
/// Return closures cannot create dangling references:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// fn source(value: &str) -> &str { value }
/// let mock = shimforge::mock!(session, source, fn(&str) -> &str).unwrap();
/// mock.expect().returning(|_| String::from("temporary").as_str());
/// ```
/// Use [`replace!`] for unsafe or native functions:
/// ```compile_fail
/// let mut session = shimforge::Session::new().unwrap();
/// extern "C" fn source() -> usize { 1 }
/// shimforge::mock!(session, source, extern "C" fn() -> usize);
/// ```
#[macro_export]
macro_rules! mock {
    ($($input:tt)*) => { $crate::__mock!($crate, $($input)*) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __install {
    ($session:expr, $source:expr, $target:expr, $control:expr, $detach:expr) => {{
        let (session, source, target, control, detach) =
            (($session).__borrow(), $source, $target, $control, $detach);
        // SAFETY: the generated mock checks types; callers keep patching idle.
        unsafe { session.__install(source, target, control, detach) }
    }};
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
        let session = ($session).__borrow();
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
        let session = ($session).__borrow();
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
        let session = ($session).__borrow();
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
        let session = ($session).__borrow();
        // SAFETY: types are checked above; callers must follow the runtime safety rules.
        unsafe { session.replace_raw(source as *const (), target as *const ()) }
    }};
}
