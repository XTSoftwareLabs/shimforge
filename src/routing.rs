use crate::executable::Executable;
use crate::expectation::lock;
use crate::{Error, Patch, code, memory};
use std::cell::RefCell;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

struct Entry {
    patch: Patch,
    dispatcher: usize,
    original: Executable,
    // Relocated x86-64 calls need shadow stacks off. ARM64 calls set x30 directly.
    #[cfg(target_arch = "x86_64")]
    call_bridge: bool,
    global: AtomicUsize,
    next: *mut Entry,
}

// SAFETY: entries are immutable after publication, except for the atomic target.
unsafe impl Sync for Entry {}

struct Record {
    entry: &'static Entry,
    sites: Vec<usize>,
}

struct Local {
    source: usize,
    dispatcher: usize,
    target: usize,
}

static ENTRIES: Mutex<Vec<Record>> = Mutex::new(Vec::new());
static HEAD: AtomicPtr<Entry> = AtomicPtr::new(ptr::null_mut());
thread_local! {
    static LOCAL: RefCell<Vec<Local>> = const { RefCell::new(Vec::new()) };
}

/// Returns the current replacement or saved original entry.
#[doc(hidden)]
pub fn route(dispatcher: usize) -> Option<usize> {
    let local = LOCAL
        .try_with(|routes| {
            routes.try_borrow().ok().and_then(|routes| {
                routes
                    .iter()
                    .find(|route| route.dispatcher == dispatcher)
                    .map(|route| route.target)
            })
        })
        .ok()
        .flatten();
    local
        .filter(|target| *target != dispatcher)
        .or_else(|| fallback(dispatcher))
}

fn fallback(dispatcher: usize) -> Option<usize> {
    let mut cursor = HEAD.load(Ordering::Acquire);
    while !cursor.is_null() {
        // SAFETY: published entries remain allocated until process exit.
        let entry = unsafe { &*cursor };
        if entry.dispatcher == dispatcher {
            let global = entry.global.load(Ordering::Acquire);
            return Some(if global == 0 {
                #[cfg(target_arch = "x86_64")]
                if entry.call_bridge {
                    crate::executable::check_call_bridge()
                        .expect("original call is not supported with shadow stacks");
                }
                entry.original.address()
            } else {
                global
            });
        }
        cursor = entry.next;
    }
    None
}

pub(crate) unsafe fn install(source: usize, target: usize) -> Result<(), Error> {
    // SAFETY: the generated handler has the checked source signature.
    unsafe { install_replacement(source, target, target) }
}

pub(crate) unsafe fn install_replacement(
    source: usize,
    dispatcher: usize,
    target: usize,
) -> Result<(), Error> {
    if source == target {
        return Err(Error::SameAddress);
    }
    LOCAL.with(|routes| {
        let mut routes = routes.borrow_mut();
        if routes.iter().any(|route| route.source == source) {
            return Err(Error::Overlap);
        }
        routes.reserve(1);
        Ok(())
    })?;
    let mut entries = lock(&ENTRIES);
    if entries.iter().any(|record| {
        let patch = &record.entry.patch;
        let end = patch.address + patch.original.len();
        (patch.entry <= target && target < end)
            || (source != patch.entry
                && ((patch.entry <= source && source < end) || record.sites.contains(&dispatcher)))
    }) {
        return Err(Error::Overlap);
    }
    memory::read(target, 1)?;
    memory::read(dispatcher, 1)?;
    let entry = if let Some(record) = entries
        .iter_mut()
        .find(|record| record.entry.patch.entry == source)
    {
        if !record.sites.contains(&dispatcher) {
            record.sites.push(dispatcher);
        }
        record.entry
    } else {
        let bytes = memory::read(source, code::MAX_PREFIX)?;
        let mut original = Executable::near(source)?;
        let relay = original.address() + 512;
        let plan = code::plan(source, relay, &bytes)?;
        let address = source.checked_add(plan.offset).ok_or(Error::InvalidRange)?;
        let end = address
            .checked_add(plan.original.len())
            .ok_or(Error::InvalidRange)?;
        if entries.iter().any(|record| {
            address < record.entry.patch.address + record.entry.patch.original.len()
                && record.entry.patch.address < end
        }) {
            return Err(Error::Overlap);
        }
        let mut saved = code::trampoline(address, original.address(), &plan.original)?;
        #[cfg(target_arch = "x86_64")]
        let call_bridge = code::needs_call_bridge(&plan.original)?;
        #[cfg(target_arch = "x86_64")]
        if call_bridge {
            crate::executable::check_call_bridge()?;
        }
        saved.resize(512, 0xcc);
        saved.extend_from_slice(&code::jump(relay, dispatcher)?);
        original.publish(&saved)?;
        let entry = Box::new(Entry {
            patch: Patch {
                entry: source,
                address,
                original: plan.original,
                replacement: plan.replacement,
                #[cfg(target_arch = "aarch64")]
                _relay: None,
            },
            dispatcher,
            original,
            #[cfg(target_arch = "x86_64")]
            call_bridge,
            global: AtomicUsize::new(0),
            next: HEAD.load(Ordering::Relaxed),
        });
        let sites = vec![dispatcher];
        entries.reserve(1);
        let entry = Box::leak(entry);
        let previous = entry.next;
        // Publish the route before patching. A thread that reaches the dispatcher
        // while the entry is being written must still find the saved original.
        HEAD.store(entry, Ordering::Release);
        let entry: &'static Entry = entry;
        // SAFETY: the caller stops calls during the first entry patch.
        let written =
            unsafe { memory::write(address, &entry.patch.original, &entry.patch.replacement) };
        if let Err(error) = written {
            // The source is unpatched again, so nothing reaches this entry. It stays
            // allocated because a thread may still hold it from the failed window.
            HEAD.store(previous, Ordering::Release);
            return Err(error);
        }
        entries.push(Record { entry, sites });
        entry
    };
    LOCAL.with(|routes| {
        routes.borrow_mut().push(Local {
            source,
            dispatcher: entry.dispatcher,
            target,
        })
    });
    Ok(())
}

pub(crate) fn remove(source: usize) {
    LOCAL.with(|routes| routes.borrow_mut().retain(|route| route.source != source));
}

pub(crate) fn global(source: usize, target: usize) -> Result<bool, Error> {
    let entries = lock(&ENTRIES);
    if source == target {
        return Err(Error::SameAddress);
    }
    if entries.iter().any(|record| {
        let patch = &record.entry.patch;
        let end = patch.address + patch.original.len();
        (patch.entry <= target && target < end)
            || (source != patch.entry && patch.entry <= source && source < end)
    }) {
        return Err(Error::Overlap);
    }
    let Some(record) = entries
        .iter()
        .find(|record| record.entry.patch.entry == source)
    else {
        return Ok(false);
    };
    memory::read(target, 1)?;
    if record
        .entry
        .global
        .compare_exchange(0, target, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Error::Overlap);
    }
    Ok(true)
}

pub(crate) fn overlaps(address: usize, length: usize) -> bool {
    lock(&ENTRIES).iter().any(|record| {
        let patch = &record.entry.patch;
        address < patch.address + patch.original.len() && patch.address < address + length
    })
}

pub(crate) fn remove_global(source: usize) {
    let entries = lock(&ENTRIES);
    let record = entries
        .iter()
        .find(|record| record.entry.patch.entry == source)
        .expect("global route is missing");
    record.entry.global.store(0, Ordering::Release);
}

#[cfg(test)]
mod tests;
