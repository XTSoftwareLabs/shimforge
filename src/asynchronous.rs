use crate::expectation::{Config, Control, Meta, Rule, State, lock};
use crate::{CallCount, Error, Expectation, Sequence, Session};
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

type RegistryEntry = (usize, Arc<dyn Any + Send + Sync>);
static REGISTRY: Mutex<Vec<RegistryEntry>> = Mutex::new(Vec::new());

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
    pub fn mock_async<F>(&mut self, witness: F) -> Result<AsyncMock<F::Output>, Error>
    where
        F: Future,
        F::Output: Send + 'static,
    {
        let source = F::poll as *const ();
        drop(witness);
        let state = State::new(std::any::type_name::<F>());
        register(source as usize, state.clone())?;
        let control: Arc<dyn Control> = state.clone();
        let address = source as usize;
        // SAFETY: both poll functions use F's exact signature and output type.
        unsafe {
            self.__install(
                source,
                poll_mock::<F> as *const (),
                control,
                Box::new(move || remove(address)),
            )?;
        }
        Ok(AsyncMock { state })
    }
}

fn register<R: Send + 'static>(
    address: usize,
    state: Arc<State<AsyncRule<R>>>,
) -> Result<(), Error> {
    let mut registry = lock(&REGISTRY);
    if registry.iter().any(|entry| entry.0 == address) {
        return Err(Error::Expectation("this future is already mocked".into()));
    }
    registry.push((address, state));
    Ok(())
}

fn remove(address: usize) {
    lock(&REGISTRY).retain(|entry| entry.0 != address);
}

fn lookup<R: Send + 'static>(address: usize) -> Arc<State<AsyncRule<R>>> {
    let registry = lock(&REGISTRY);
    let state = &registry
        .iter()
        .find(|entry| entry.0 == address)
        .expect("async mock is no longer active")
        .1;
    state
        .clone()
        .downcast::<State<AsyncRule<R>>>()
        .expect("async mock output type changed")
}

fn poll_mock<F: Future>(_: Pin<&mut F>, _: &mut Context<'_>) -> Poll<F::Output>
where
    F::Output: Send + 'static,
{
    let state = lookup::<F::Output>(F::poll as *const () as usize);
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

    /// Checks call counts and reports unexpected calls.
    pub fn verify(&self) -> Result<(), Error> {
        self.state.verify()
    }

    /// Checks expectations, then clears them for the next phase.
    pub fn checkpoint(&self) -> Result<(), Error> {
        self.state.checkpoint()
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
    pub fn returning(
        self,
        action: impl FnMut() -> R + Send + 'static,
    ) -> Result<Expectation, Error> {
        self.state.add(self.config, |meta| AsyncRule {
            meta,
            action: Mutex::new(Box::new(action)),
        })
    }

    /// Calls a closure at most once, moving its captured values if needed.
    pub fn returning_once(
        mut self,
        action: impl FnOnce() -> R + Send + 'static,
    ) -> Result<Expectation, Error> {
        self.config = self.config.for_once()?;
        let mut action = Some(action);
        self.returning(move || action.take().expect("one-time response was already used")())
    }

    /// Clones the value for each response.
    pub fn returns(self, value: R) -> Result<Expectation, Error>
    where
        R: Clone,
    {
        self.returning(move || value.clone())
    }

    /// Moves the value into one response. It need not implement `Clone`.
    pub fn return_once(self, value: R) -> Result<Expectation, Error> {
        self.returning_once(move || value)
    }

    /// Creates a default value for each response.
    pub fn returns_default(self) -> Result<Expectation, Error>
    where
        R: Default,
    {
        self.returning(R::default)
    }

    /// Panics with this message when called.
    pub fn panics(self, message: impl Into<String>) -> Result<Expectation, Error> {
        let message = message.into();
        self.returning(move || panic!("{message}"))
    }

    /// Rejects every poll of this future type.
    pub fn never(self) -> Result<Expectation, Error> {
        self.times(0).panics("poll is forbidden")
    }
}
