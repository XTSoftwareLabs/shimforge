#![allow(missing_docs)]

use crate::Error;
use std::cell::RefCell;
use std::ops::{Range, RangeFrom, RangeFull, RangeInclusive, RangeTo, RangeToInclusive};
use std::sync::{Arc, Mutex, MutexGuard};

#[cfg(test)]
mod tests;

/// An exact call count or a range of counts.
#[derive(Clone, Copy, Debug)]
pub struct CallCount {
    min: usize,
    max: Option<usize>,
}

impl CallCount {
    fn validate(self) -> Result<Self, Error> {
        if self.max.is_some_and(|max| self.min > max) {
            return Err(error("call count range is empty"));
        }
        Ok(self)
    }
}

impl From<usize> for CallCount {
    fn from(count: usize) -> Self {
        Self {
            min: count,
            max: Some(count),
        }
    }
}

impl From<Range<usize>> for CallCount {
    fn from(range: Range<usize>) -> Self {
        match range.end.checked_sub(1) {
            Some(max) => Self {
                min: range.start,
                max: Some(max),
            },
            None => Self {
                min: 1,
                max: Some(0),
            },
        }
    }
}

impl From<RangeInclusive<usize>> for CallCount {
    fn from(range: RangeInclusive<usize>) -> Self {
        Self {
            min: *range.start(),
            max: Some(*range.end()),
        }
    }
}

impl From<RangeFrom<usize>> for CallCount {
    fn from(range: RangeFrom<usize>) -> Self {
        Self {
            min: range.start,
            max: None,
        }
    }
}

impl From<RangeTo<usize>> for CallCount {
    fn from(range: RangeTo<usize>) -> Self {
        (0..range.end).into()
    }
}

impl From<RangeToInclusive<usize>> for CallCount {
    fn from(range: RangeToInclusive<usize>) -> Self {
        (0..=range.end).into()
    }
}

impl From<RangeFull> for CallCount {
    fn from(_: RangeFull) -> Self {
        (0..).into()
    }
}

/// Checks the order of expectations across mocks.
#[derive(Clone, Default)]
pub struct Sequence {
    progress: Arc<Mutex<SequenceProgress>>,
}

#[derive(Default)]
struct SequenceProgress {
    next: usize,
    cursor: usize,
}

impl Sequence {
    /// Starts an empty sequence.
    pub fn new() -> Self {
        Self::default()
    }

    fn reserve(&self) -> Result<Ticket, Error> {
        let mut progress = lock(&self.progress);
        let next = progress
            .next
            .checked_add(1)
            .ok_or_else(|| error("sequence is full"))?;
        let ticket = Ticket {
            sequence: self.clone(),
            index: progress.next,
        };
        progress.next = next;
        Ok(ticket)
    }
}

#[derive(Clone)]
struct Ticket {
    sequence: Sequence,
    index: usize,
}

#[doc(hidden)]
#[derive(Clone, Default)]
pub struct Config {
    count: Option<CallCount>,
    sequence: Option<Sequence>,
}

impl Config {
    pub fn times(mut self, count: impl Into<CallCount>) -> Self {
        self.count = Some(count.into());
        self
    }

    pub fn once(self) -> Self {
        self.times(1)
    }

    pub fn in_sequence(mut self, sequence: &Sequence) -> Self {
        self.sequence = Some(sequence.clone());
        self
    }

    pub fn for_once(mut self) -> Result<Self, Error> {
        let count = self.count.unwrap_or_else(|| 1.into()).validate()?;
        if count.max.is_none_or(|max| max > 1) {
            return Err(error("a one-shot response allows at most one call"));
        }
        self.count = Some(count);
        Ok(self)
    }

    fn validate(&self) -> Result<CallCount, Error> {
        let count = self.count.unwrap_or_else(|| (..).into()).validate()?;
        if self.sequence.is_some() && (count.min == 0 || count.max != Some(count.min)) {
            return Err(error("sequence expectations need an exact positive count"));
        }
        Ok(count)
    }
}

/// Tracks one expectation without keeping its response alive.
#[derive(Clone)]
pub struct Expectation {
    meta: Arc<Meta>,
}

impl Expectation {
    /// Returns the number of calls that used this expectation.
    pub fn calls(&self) -> usize {
        lock(&self.meta.progress).calls
    }

    /// Checks the call count and any order failure.
    ///
    /// # Panics
    ///
    /// Panics if the expectation missed calls or received a call it rejected.
    #[track_caller]
    pub fn verify(&self) {
        crate::error::check(self.meta.verify());
    }
}

#[doc(hidden)]
pub struct Meta {
    name: &'static str,
    count: CallCount,
    progress: Mutex<Progress>,
}

#[derive(Default)]
struct Progress {
    calls: usize,
    ticket: Option<Ticket>,
    failure: Option<String>,
}

enum Claim {
    Taken,
    Exhausted,
}

impl Meta {
    fn claim(&self) -> Result<Claim, Error> {
        let mut progress = lock(&self.progress);
        if self.count.max == Some(0) {
            return self.fail(&mut progress, "a forbidden call was made");
        }
        if self.count.max.is_some_and(|max| progress.calls >= max) {
            return Ok(Claim::Exhausted);
        }
        let Some(calls) = progress.calls.checked_add(1) else {
            return self.fail(&mut progress, "call count overflow");
        };
        if let Some(ticket) = progress.ticket.clone() {
            let mut sequence = lock(&ticket.sequence.progress);
            if sequence.cursor != ticket.index {
                return self.fail(&mut progress, "call arrived out of order");
            }
            if self.count.max == Some(calls) {
                sequence.cursor += 1;
            }
        }
        progress.calls = calls;
        Ok(Claim::Taken)
    }

    fn fail(&self, progress: &mut Progress, reason: &str) -> Result<Claim, Error> {
        let message = format!("{}: {reason}", self.name);
        progress.failure.get_or_insert_with(|| message.clone());
        Err(error(message))
    }

    fn verify(&self) -> Result<(), Error> {
        let progress = lock(&self.progress);
        if let Some(message) = &progress.failure {
            return Err(error(message.clone()));
        }
        if progress.calls < self.count.min {
            return Err(error(format!(
                "{}: expected at least {} calls, observed {}",
                self.name, self.count.min, progress.calls
            )));
        }
        Ok(())
    }
}

#[doc(hidden)]
pub trait Rule: Send + Sync + 'static {
    fn meta(&self) -> &Arc<Meta>;
}

#[doc(hidden)]
pub trait Control: Send + Sync {
    fn verify(&self) -> Result<(), Error>;
    fn deactivate(&self);
}

#[doc(hidden)]
pub struct State<R: Rule> {
    name: &'static str,
    inner: Mutex<Inner<R>>,
}

struct Inner<R> {
    active: bool,
    rules: Vec<Arc<R>>,
    failure: Option<Error>,
}

impl<R: Rule> State<R> {
    pub fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            inner: Mutex::new(Inner {
                active: true,
                rules: Vec::new(),
                failure: None,
            }),
        })
    }

    pub fn enter(&self) -> CallGuard {
        match CallGuard::try_enter(self as *const Self as usize) {
            Ok(guard) => guard,
            Err(failure) => self.fail(error(format!("{}: {failure}", self.name))),
        }
    }

    pub fn add(
        &self,
        config: Config,
        make: impl FnOnce(Arc<Meta>) -> R,
    ) -> Result<Expectation, Error> {
        self.add_with(config, Box::new(make))
    }

    fn add_with(
        &self,
        config: Config,
        make: Box<dyn FnOnce(Arc<Meta>) -> R + '_>,
    ) -> Result<Expectation, Error> {
        let count = config.validate()?;
        if !lock(&self.inner).active {
            return Err(error(format!("{}: mock is no longer active", self.name)));
        }
        let meta = Arc::new(Meta {
            name: self.name,
            count,
            progress: Mutex::new(Progress::default()),
        });
        let rule = Arc::new(make(meta.clone()));
        let mut inner = lock(&self.inner);
        if !inner.active {
            return Err(error(format!("{}: mock is no longer active", self.name)));
        }
        if let Some(sequence) = config.sequence {
            lock(&meta.progress).ticket = Some(sequence.reserve()?);
        }
        inner.rules.push(rule);
        Ok(Expectation { meta })
    }

    pub fn select(&self, matches: &dyn Fn(&R) -> bool) -> Arc<R> {
        let rules = {
            let inner = lock(&self.inner);
            if !inner.active {
                drop(inner);
                self.fail(error(format!("{}: mock is no longer active", self.name)));
            }
            inner.rules.clone()
        };
        let mut matched = false;
        for rule in rules {
            if matches(&rule) {
                matched = true;
                match rule.meta().claim() {
                    Ok(Claim::Taken) => return rule,
                    Ok(Claim::Exhausted) => {}
                    Err(failure) => self.fail(failure),
                }
            }
        }
        let reason = if matched {
            "matching expectations are exhausted"
        } else {
            "no expectation matched this call"
        };
        self.fail(error(format!("{}: {reason}", self.name)));
    }

    fn fail(&self, failure: Error) -> ! {
        lock(&self.inner).failure.get_or_insert(failure.clone());
        panic!("{failure}");
    }

    pub fn verify(&self) -> Result<(), Error> {
        Self::verify_inner(&lock(&self.inner))
    }

    fn verify_inner(inner: &Inner<R>) -> Result<(), Error> {
        if let Some(failure) = &inner.failure {
            return Err(failure.clone());
        }
        for rule in &inner.rules {
            rule.meta().verify()?;
        }
        Ok(())
    }

    pub fn checkpoint(&self) -> Result<(), Error> {
        let rules = {
            let mut inner = lock(&self.inner);
            if !inner.active {
                return Err(error(format!("{}: mock is no longer active", self.name)));
            }
            Self::verify_inner(&inner)?;
            std::mem::take(&mut inner.rules)
        };
        drop(rules);
        Ok(())
    }

    pub fn deactivate(&self) {
        let rules = {
            let mut inner = lock(&self.inner);
            inner.active = false;
            std::mem::take(&mut inner.rules)
        };
        drop(rules);
    }
}

impl<R: Rule> Control for State<R> {
    fn verify(&self) -> Result<(), Error> {
        self.verify()
    }

    fn deactivate(&self) {
        self.deactivate();
    }
}

fn error(message: impl Into<String>) -> Error {
    Error::Expectation(message.into())
}

#[doc(hidden)]
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

thread_local! {
    static ACTIVE_CALLS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

#[doc(hidden)]
pub struct CallGuard {
    key: usize,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl CallGuard {
    fn try_enter(key: usize) -> Result<Self, Error> {
        ACTIVE_CALLS.with(|calls| {
            let mut calls = calls.borrow_mut();
            if calls.contains(&key) {
                return Err(error("a mock called itself recursively"));
            }
            calls.push(key);
            Ok(())
        })?;
        Ok(Self {
            key,
            _thread: std::marker::PhantomData,
        })
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        ACTIVE_CALLS.with(|calls| {
            calls.borrow_mut().retain(|key| *key != self.key);
        });
    }
}
