use crate::error::check;
use crate::expectation::{Config, Control, Meta, Rule, State, lock};
use crate::{CallCount, Error, Expectation, Sequence, Session};
use std::any::Any;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

type RegistryKey = (usize, Option<std::thread::ThreadId>);
type RegistryEntry = (usize, Arc<dyn Any + Send + Sync>);
static REGISTRY: Mutex<Vec<RegistryEntry>> = Mutex::new(Vec::new());
thread_local! {
    static LOCAL_REGISTRY: RefCell<Vec<RegistryEntry>> = const { RefCell::new(Vec::new()) };
}
static GLOBAL_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct AsyncRule<R> {
    meta: Arc<Meta>,
    action: Mutex<Box<dyn FnMut() -> R + Send>>,
}

impl<R: Send + 'static> Rule for AsyncRule<R> {
    fn meta(&self) -> &Arc<Meta> {
        &self.meta
    }
}

/// Expectations for one concrete future type.
///
/// Each matching poll returns `Ready`. Calls are counted when polled, not when
/// the future is created. Arguments stored inside a future cannot be matched.
pub struct AsyncMock<R: Send + 'static> {
    state: Arc<State<AsyncRule<R>>>,
}

/// A pending async expectation. Finish it with a return behavior or `never`.
#[must_use = "finish the expectation with a return behavior or never()"]
pub struct AsyncExpectation<R: Send + 'static> {
    state: Arc<State<AsyncRule<R>>>,
    config: Config,
}

impl Session {
    /// Mocks the poll method of the witness's concrete future type.
    ///
    /// The witness is dropped without polling. Other futures of the same type
    /// use the mock, including those created before this call. Their original
    /// bodies are not run. Join tasks before installing or restoring the mock.
    /// The future's layout and drop code stay unchanged.
    /// Do not pass a boxed trait object: its poll wrapper is shared by unrelated
    /// futures. Mock the method returning that box with [`crate::mock!`] instead.
    ///
    /// # Panics
    ///
    /// Panics if this future type is already mocked or its poll method cannot be patched.
    #[track_caller]
    pub fn mock_async<F>(&mut self, witness: F) -> AsyncMock<F::Output>
    where
        F: Future,
        F::Output: Send + 'static,
    {
        let source = F::poll as *const ();
        drop(witness);
        let state = State::new(std::any::type_name::<F>());
        let key = (source as usize, self.__thread());
        check(register(key, state.clone()));
        let control: Arc<dyn Control> = state.clone();
        // SAFETY: both poll functions use F's exact signature and output type.
        check(unsafe {
            self.__install(
                source,
                poll_mock::<F> as *const (),
                control,
                Box::new(move || remove(key)),
            )
        });
        AsyncMock { state }
    }
}

fn register(key: RegistryKey, state: Arc<dyn Any + Send + Sync>) -> Result<(), Error> {
    if key.1.is_some() {
        return LOCAL_REGISTRY.with(|registry| insert(&mut registry.borrow_mut(), key.0, state));
    }
    let mut registry = lock(&REGISTRY);
    insert(&mut registry, key.0, state)?;
    GLOBAL_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

fn insert(
    registry: &mut Vec<RegistryEntry>,
    address: usize,
    state: Arc<dyn Any + Send + Sync>,
) -> Result<(), Error> {
    if registry.iter().any(|entry| entry.0 == address) {
        return Err(Error::Expectation("this future is already mocked".into()));
    }
    registry.push((address, state));
    Ok(())
}

fn remove(key: RegistryKey) {
    if key.1.is_some() {
        LOCAL_REGISTRY.with(|registry| registry.borrow_mut().retain(|entry| entry.0 != key.0));
    } else {
        let mut registry = lock(&REGISTRY);
        registry.retain(|entry| entry.0 != key.0);
        GLOBAL_ACTIVE.store(!registry.is_empty(), std::sync::atomic::Ordering::Release);
    }
}

fn lookup<R: Send + 'static>(address: usize) -> Option<Arc<State<AsyncRule<R>>>> {
    registered(address).map(|state| {
        state
            .downcast::<State<AsyncRule<R>>>()
            .expect("async mock output type changed")
    })
}

fn registered(address: usize) -> Option<Arc<dyn Any + Send + Sync>> {
    let local = LOCAL_REGISTRY
        .try_with(|registry| {
            registry
                .try_borrow()
                .ok()
                .and_then(|registry| find(&registry, address))
        })
        .ok()
        .flatten();
    local.or_else(|| {
        if GLOBAL_ACTIVE.load(std::sync::atomic::Ordering::Acquire) {
            find(&lock(&REGISTRY), address)
        } else {
            None
        }
    })
}

fn find(registry: &[RegistryEntry], address: usize) -> Option<Arc<dyn Any + Send + Sync>> {
    registry
        .iter()
        .find(|entry| entry.0 == address)
        .map(|entry| entry.1.clone())
}

fn poll_mock<F: Future>(future: Pin<&mut F>, context: &mut Context<'_>) -> Poll<F::Output>
where
    F::Output: Send + 'static,
{
    let state = match lookup::<F::Output>(F::poll as *const () as usize) {
        Some(state) => state,
        None => {
            let address = poll_mock::<F> as *const () as usize;
            let target = crate::routing::route(address).expect("async mock is no longer active");
            // SAFETY: the original trampoline preserves F::poll's exact ABI and type.
            let original: fn(Pin<&mut F>, &mut Context<'_>) -> Poll<F::Output> =
                unsafe { std::mem::transmute(target) };
            return original(future, context);
        }
    };
    let _guard = state.enter();
    let rule = state.select(&|_| true);
    let result = (lock(&rule.action))();
    Poll::Ready(result)
}

impl<R: Send + 'static> AsyncMock<R> {
    /// Starts an expectation. By default any number of calls is allowed.
    pub fn expect(&self) -> AsyncExpectation<R> {
        AsyncExpectation {
            state: self.state.clone(),
            config: Config::default(),
        }
    }

    /// Checks call counts and unexpected calls, and panics if either failed.
    #[track_caller]
    pub fn verify(&self) {
        check(self.state.verify());
    }

    /// Checks expectations like [`Self::verify`], then clears them for the next phase.
    #[track_caller]
    pub fn checkpoint(&self) {
        check(self.state.checkpoint());
    }
}

impl<R: Send + 'static> AsyncExpectation<R> {
    /// Requires a count or range of counts.
    pub fn times(mut self, count: impl Into<CallCount>) -> Self {
        self.config = self.config.times(count);
        self
    }

    /// Requires exactly one call.
    pub fn once(mut self) -> Self {
        self.config = self.config.once();
        self
    }

    /// Orders this expectation with others. Use an exact positive count.
    pub fn in_sequence(mut self, sequence: &Sequence) -> Self {
        self.config = self.config.in_sequence(sequence);
        self
    }

    /// Calls a closure for each response. The closure may own captured values.
    ///
    /// # Panics
    ///
    /// Panics if the call count is invalid or the mock is no longer active.
    #[track_caller]
    pub fn returning(self, action: impl FnMut() -> R + Send + 'static) -> Expectation {
        check(self.state.add(self.config, |meta| AsyncRule {
            meta,
            action: Mutex::new(Box::new(action)),
        }))
    }

    /// Calls a closure at most once, moving its captured values if needed.
    #[track_caller]
    pub fn returning_once(mut self, action: impl FnOnce() -> R + Send + 'static) -> Expectation {
        self.config = check(self.config.for_once());
        let mut action = Some(action);
        self.returning(move || action.take().expect("one-time response was already used")())
    }

    /// Clones the value for each response.
    #[track_caller]
    pub fn returns(self, value: R) -> Expectation
    where
        R: Clone,
    {
        self.returning(move || value.clone())
    }

    /// Moves the value into one response. It need not implement `Clone`.
    #[track_caller]
    pub fn return_once(self, value: R) -> Expectation {
        self.returning_once(move || value)
    }

    /// Creates a default value for each response.
    #[track_caller]
    pub fn returns_default(self) -> Expectation
    where
        R: Default,
    {
        self.returning(R::default)
    }

    /// Panics with this message when called.
    #[track_caller]
    pub fn panics(self, message: impl Into<String>) -> Expectation {
        let message = message.into();
        self.returning(move || panic!("{message}"))
    }

    /// Rejects every poll of this future type.
    #[track_caller]
    pub fn never(self) -> Expectation {
        self.times(0).panics("poll is forbidden")
    }
}

#[cfg(test)]
mod tests;
