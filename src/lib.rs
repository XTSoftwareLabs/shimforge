// The README examples are `#[test]` functions run by tests/readme.rs, not doctests.
#![cfg_attr(not(doctest), doc = include_str!("../README.md"))]
#![allow(clippy::test_attr_in_doctest)]
#![deny(missing_docs)]

#[cfg(not(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "windows", target_os = "linux", target_os = "macos")
)))]
compile_error!("shimforge supports Windows, Linux, and macOS on x86-64 and ARM64");

mod asynchronous;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod cache;
mod code;
mod error;
mod executable;
mod expectation;
mod memory;
mod routing;

#[cfg(test)]
mod tests;

pub use asynchronous::{AsyncExpectation, AsyncMock};
pub use error::Error;
pub use expectation::{CallCount, Expectation, Sequence};

#[doc(hidden)]
pub use shimforge_macros::{__check_signature, __mock, __replace_local};

#[doc(hidden)]
pub mod __private {
    pub use crate::expectation::{CallGuard, Config, Control, Meta, Rule, State, lock};
    pub use crate::routing::route;
}

use std::cell::Cell;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::thread::ThreadId;

static SESSION: RwLock<()> = RwLock::new(());
thread_local! { static SESSION_ACTIVE: Cell<bool> = const { Cell::new(false) }; }

enum SessionLock {
    Global {
        _guard: RwLockWriteGuard<'static, ()>,
    },
    Local {
        _guard: RwLockReadGuard<'static, ()>,
    },
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        SESSION_ACTIVE.set(false);
    }
}

struct Patch {
    entry: usize,
    address: usize,
    original: Vec<u8>,
    replacement: Vec<u8>,
    #[cfg(target_arch = "aarch64")]
    _relay: Option<executable::Executable>,
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

/// A scope for global or thread-local function mocks.
///
/// Drop removes mocks and restores original behavior, including during a panic.
/// A session cannot move to another thread.
#[must_use = "keep the session alive while using the mocks"]
pub struct Session {
    patches: Vec<Patch>,
    mocks: Vec<OwnedMock>,
    local_patches: Vec<usize>,
    global_routes: Vec<usize>,
    lock: SessionLock,
}

impl Session {
    #[doc(hidden)]
    pub fn __borrow(&mut self) -> &mut Self {
        self
    }

    /// Opens a thread-local session. See [`Self::new_local`].
    pub fn new() -> Result<Self, Error> {
        Self::new_local()
    }

    /// Opens a global session. Mocks affect all threads.
    /// Waits for other threads' sessions to end. Nested sessions return [`Error::Busy`].
    pub fn new_global() -> Result<Self, Error> {
        Self::open(false, true)
    }

    /// Opens a session whose mocks affect only the current thread.
    /// Other threads keep their own mocks or call the original function.
    /// Only one local session may be active per thread. Global sessions exclude
    /// local sessions, so this waits for them to end. Stop target calls during
    /// the first installation. Later local installs and cleanup do not patch code.
    pub fn new_local() -> Result<Self, Error> {
        Self::open(true, true)
    }

    /// Opens a local session without waiting for an active global session.
    pub fn try_new_local() -> Result<Self, Error> {
        Self::open(true, false)
    }

    /// Opens a global session without waiting for other sessions.
    pub fn try_new_global() -> Result<Self, Error> {
        Self::open(false, false)
    }

    fn open(local: bool, wait: bool) -> Result<Self, Error> {
        if SESSION_ACTIVE.get() {
            return Err(Error::Busy);
        }
        let lock = if local {
            let result = if wait {
                SESSION.read().map_err(TryLockError::Poisoned)
            } else {
                SESSION.try_read()
            };
            let guard = match result {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => return Err(Error::Busy),
            };
            SessionLock::Local { _guard: guard }
        } else {
            let result = if wait {
                SESSION.write().map_err(TryLockError::Poisoned)
            } else {
                SESSION.try_write()
            };
            let guard = match result {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => return Err(Error::Busy),
            };
            SessionLock::Global { _guard: guard }
        };
        SESSION_ACTIVE.set(true);
        Ok(Self {
            patches: Vec::new(),
            mocks: Vec::new(),
            local_patches: Vec::new(),
            global_routes: Vec::new(),
            lock,
        })
    }

    #[doc(hidden)]
    pub fn __thread(&self) -> Option<ThreadId> {
        matches!(self.lock, SessionLock::Local { .. }).then(|| std::thread::current().id())
    }

    #[doc(hidden)]
    /// # Safety
    /// All three pointers must use the checked source signature and stay loaded
    /// until process exit. Stop calls during the first installation.
    pub unsafe fn __replace_local(
        &mut self,
        source: *const (),
        dispatcher: *const (),
        target: *const (),
    ) -> Result<(), Error> {
        self.local_patches.reserve(1);
        // SAFETY: the macro checks the function types before calling this method.
        unsafe {
            routing::install_replacement(source as usize, dispatcher as usize, target as usize)?
        };
        self.local_patches.push(source as usize);
        Ok(())
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
        if self.__thread().is_some() {
            return Err(Error::Expectation(
                "use mock! or mock_async in a local session".into(),
            ));
        }
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
        self.global_routes.reserve(1);
        if routing::global(source, target)? {
            self.global_routes.push(source);
            return Ok(());
        }
        memory::read(target, 1)?;
        let bytes = memory::read(source, code::MAX_PREFIX)?;
        #[cfg(target_arch = "aarch64")]
        let relay = {
            let mut relay = executable::Executable::near(source)?;
            relay.publish(&code::jump(relay.address(), target)?)?;
            relay
        };
        #[cfg(target_arch = "aarch64")]
        let target = relay.address();
        let plan = code::plan(source, target, &bytes)?;
        let address = source.checked_add(plan.offset).ok_or(Error::InvalidRange)?;
        let end = address
            .checked_add(plan.original.len())
            .ok_or(Error::InvalidRange)?;
        if self
            .patches
            .iter()
            .any(|patch| address < patch.address + patch.original.len() && patch.address < end)
            || routing::overlaps(address, end - address)
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
            #[cfg(target_arch = "aarch64")]
            _relay: Some(relay),
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
            if self.__thread().is_some() {
                self.local_patches.reserve(1);
                routing::install(source as usize, target as usize)?;
                self.local_patches.push(source as usize);
            } else {
                self.replace_raw(source, target)?;
            }
        }
        self.mocks.push(mock);
        Ok(())
    }

    /// Removes all mocks, then checks their expectations.
    ///
    /// Stop global target calls as required by [`Self::replace_raw`]. On error,
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
        while let Some(source) = self.local_patches.last() {
            routing::remove(*source);
            self.local_patches.pop();
        }
        while let Some(source) = self.global_routes.pop() {
            routing::remove_global(source);
        }
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
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(value: u64) -> u64 { value }
/// shimforge::mock!(session, source, fn(u32) -> u32);
/// ```
/// A replacement cannot require a longer input borrow:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(value: &str) -> usize { value.len() }
/// shimforge::mock!(session, source, fn(&'static str) -> usize);
/// ```
/// Type aliases do not bypass this check:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(value: &str) -> usize { value.len() }
/// type Input = &'static str;
/// shimforge::mock!(session, source, fn(Input) -> usize);
/// ```
/// A static result cannot become a shorter borrow:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(_: &str) -> &'static str { "fixed" }
/// shimforge::mock!(session, source, fn(&str) -> &str);
/// ```
/// A result must stay tied to the same argument:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source<'a, 'b>(left: &'a str, _: &'b str) -> &'a str { left }
/// shimforge::mock!(session, source, for<'a, 'b> fn(&'a str, &'b str) -> &'b str);
/// ```
/// Caller names cannot shadow the checks:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn __target(_: &str) -> &'static str { "fixed" }
/// shimforge::mock!(session, __target, fn(&str) -> &str);
/// ```
/// Captures must be safe to send between threads:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source() -> usize { 1 }
/// let mock = shimforge::mock!(session, source, fn() -> usize).unwrap();
/// let value = std::rc::Rc::new(2);
/// mock.expect().returning(move || *value);
/// ```
/// Captures must outlive the test's stack:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source() -> usize { 1 }
/// let mock = shimforge::mock!(session, source, fn() -> usize).unwrap();
/// let value = String::from("token");
/// mock.expect().returning(|| value.len());
/// ```
/// Return closures cannot create dangling references:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(value: &str) -> &str { value }
/// let mock = shimforge::mock!(session, source, fn(&str) -> &str).unwrap();
/// mock.expect().returning(|_| String::from("temporary").as_str());
/// ```
/// Calling conventions must match:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// extern "C" fn source() -> usize { 1 }
/// shimforge::mock!(session, source, fn() -> usize);
/// ```
#[macro_export]
macro_rules! mock {
    ($($input:tt)*) => { $crate::__mock!($crate, $($input)*) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __install_replacement {
    ($session:expr, $source:expr, $dispatcher:expr, $target:expr) => {{
        // SAFETY: replace! checks the types before generating this dispatcher.
        unsafe { $session.__replace_local($source, $dispatcher, $target) }
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __invoke {
    ($address:expr, $signature:ty, ($($argument:ident),* $(,)?)) => {{
        let address = $address;
        // SAFETY: routing stores only targets with this checked signature.
        #[allow(clippy::type_complexity)]
        let function: $signature = unsafe { ::std::mem::transmute(address) };
        function($($argument),*)
    }};
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
/// let mut session = shimforge::Session::new_global()?;
/// shimforge::replace!(session, original => fake, fn(i32) -> i32)?;
/// # Ok::<(), shimforge::Error>(())
/// ```
///
/// No `unsafe` block is needed. Follow the crate's safety rules. Lifetime checks
/// are best effort; do not narrow lifetimes to force a type match. Closures
/// without captures are accepted.
///
/// Incompatible signatures are rejected:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(x: u32) -> u32 { x }
/// shimforge::replace!(session, source => |x: u64| x, fn(u32) -> u32);
/// ```
/// The source must also match:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(x: u64) -> u64 { x }
/// shimforge::replace!(session, source => |x| x, fn(u32) -> u32);
/// ```
/// Input borrows cannot be narrowed:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(value: &str) -> usize { value.len() }
/// shimforge::replace!(session, source => |value| value.len(), fn(&'static str) -> usize);
/// ```
/// This also applies to mutable borrows:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(value: &mut usize) { *value += 1; }
/// shimforge::replace!(session, source => |_| (), fn(&'static mut usize));
/// ```
/// A static result cannot become a shorter borrow:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source(_: &str) -> &'static str { "fixed" }
/// shimforge::replace!(session, source => |value| value, fn(&str) -> &str);
/// ```
/// A result must stay tied to the same argument:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// fn source<'a, 'b>(left: &'a str, _: &'b str) -> &'a str { left }
/// shimforge::replace!(session, source => |_, right| right,
///     for<'a, 'b> fn(&'a str, &'b str) -> &'b str);
/// ```
/// Calling conventions must match:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// extern "C" fn source(x: u32) -> u32 { x }
/// shimforge::replace!(session, source => |x| x, fn(u32) -> u32);
/// ```
/// Capturing closures are rejected:
/// ```compile_fail
/// let mut session = shimforge::Session::new_global().unwrap();
/// let captured = String::from("hello");
/// fn source() -> usize { 1 }
/// shimforge::replace!(session, source => || captured.len(), fn() -> usize);
/// ```
#[macro_export]
macro_rules! replace {
    (@infer $type:ty) => { _ };
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let original = $source;
        let target: $(for<$($lt),+>)? fn($($arg),*) $(-> $ret)? = $target;
        $crate::__check_signature!($source, original, target, $(for<$($lt),+>)? fn($($arg),*) $(-> $ret)?);
        let source = original as fn($($crate::replace!(@infer $arg)),*) -> _;
        // Infer source lifetimes, then require matching pointer types.
        fn checked<T>(source: T, _: T) -> T { source }
        let source = checked(source, target);
        let session = ($session).__borrow();
        if session.__thread().is_some() {
            $crate::__replace_local!($crate, session, source, target, $(for<$($lt),+>)? fn($($arg),*) $(-> $ret)?)
        } else {
            // SAFETY: the types above match; callers follow the runtime safety rules.
            unsafe { session.replace_raw(source as *const (), target as *const ()) }
        }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? unsafe fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source = $source as unsafe fn($($crate::replace!(@infer $arg)),*) -> _;
        let target: $(for<$($lt),+>)? unsafe fn($($arg),*) $(-> $ret)? = $target;
        // Infer source lifetimes, then require matching pointer types.
        fn checked<T>(source: T, _: T) -> T { source }
        let source = checked(source, target);
        let session = ($session).__borrow();
        if session.__thread().is_some() {
            $crate::__replace_local!($crate, session, source, target, $(for<$($lt),+>)? unsafe fn($($arg),*) $(-> $ret)?)
        } else {
            // SAFETY: the types above match; callers follow the runtime safety rules.
            unsafe { session.replace_raw(source as *const (), target as *const ()) }
        }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? extern $abi:literal fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source = $source as extern $abi fn($($crate::replace!(@infer $arg)),*) -> _;
        let target: $(for<$($lt),+>)? extern $abi fn($($arg),*) $(-> $ret)? = $target;
        // Infer source lifetimes, then require matching pointer types.
        fn checked<T>(source: T, _: T) -> T { source }
        let source = checked(source, target);
        let session = ($session).__borrow();
        if session.__thread().is_some() {
            $crate::__replace_local!($crate, session, source, target, $(for<$($lt),+>)? extern $abi fn($($arg),*) $(-> $ret)?)
        } else {
            // SAFETY: the types above match; callers follow the runtime safety rules.
            unsafe { session.replace_raw(source as *const (), target as *const ()) }
        }
    }};
    ($session:expr, $source:expr => $target:expr,
        $(for<$($lt:lifetime),+>)? unsafe extern $abi:literal fn($($arg:ty),* $(,)?) $(-> $ret:ty)? $(,)?) => {{
        let source = $source as unsafe extern $abi fn($($crate::replace!(@infer $arg)),*) -> _;
        let target: $(for<$($lt),+>)? unsafe extern $abi fn($($arg),*) $(-> $ret)? = $target;
        // Infer source lifetimes, then require matching pointer types.
        fn checked<T>(source: T, _: T) -> T { source }
        let source = checked(source, target);
        let session = ($session).__borrow();
        if session.__thread().is_some() {
            $crate::__replace_local!($crate, session, source, target, $(for<$($lt),+>)? unsafe extern $abi fn($($arg),*) $(-> $ret)?)
        } else {
            // SAFETY: the types above match; callers follow the runtime safety rules.
            unsafe { session.replace_raw(source as *const (), target as *const ()) }
        }
    }};
}
