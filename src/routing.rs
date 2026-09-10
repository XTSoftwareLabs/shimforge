use crate::executable::Executable;
use crate::expectation::lock;
use crate::{Error, Patch, code, memory};
use std::sync::Mutex;
use std::thread::ThreadId;

struct Entry {
    patch: Patch,
    dispatcher: usize,
    original: Executable,
    routes: Vec<(ThreadId, usize)>,
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// Returns this thread's target, or the original code, for a local dispatcher.
#[doc(hidden)]
pub fn route(dispatcher: usize) -> Option<usize> {
    let entries = lock(&ENTRIES);
    entries
        .iter()
        .find(|entry| {
            entry.dispatcher == dispatcher || entry.routes.iter().any(|route| route.1 == dispatcher)
        })
        .map(|entry| {
            entry
                .routes
                .iter()
                .find(|route| route.0 == std::thread::current().id())
                .map_or(entry.original.address(), |route| route.1)
        })
}

pub(crate) unsafe fn install(source: usize, target: usize) -> Result<(), Error> {
    if source == target {
        return Err(Error::SameAddress);
    }
    let mut entries = lock(&ENTRIES);
    let thread = std::thread::current().id();
    if entries.iter().any(|entry| {
        let patch = &entry.patch;
        let end = patch.address + patch.original.len();
        (patch.entry <= target && target < end)
            || (source != patch.entry
                && ((patch.entry <= source && source < end)
                    || entry.dispatcher == target
                    || entry.routes.iter().any(|route| route.1 == target)))
    }) {
        return Err(Error::Overlap);
    }
    memory::read(target, 1)?;
    if let Some(entry) = entries.iter_mut().find(|entry| entry.patch.entry == source) {
        if entry.routes.iter().any(|route| route.0 == thread) {
            return Err(Error::Overlap);
        }
        entry.routes.push((thread, target));
        return Ok(());
    }
    let bytes = memory::read(source, code::MAX_PREFIX)?;
    let plan = code::plan(source, target, &bytes)?;
    let address = source.checked_add(plan.offset).ok_or(Error::InvalidRange)?;
    let end = address
        .checked_add(plan.original.len())
        .ok_or(Error::InvalidRange)?;
    if entries.iter().any(|entry| {
        address < entry.patch.address + entry.patch.original.len() && entry.patch.address < end
    }) {
        return Err(Error::Overlap);
    }
    let mut original = Executable::near(address)?;
    original.publish(&code::trampoline(
        address,
        original.address(),
        &plan.original,
    )?)?;
    let entry = Entry {
        patch: Patch {
            entry: source,
            address,
            original: plan.original,
            replacement: plan.replacement,
        },
        dispatcher: target,
        original,
        routes: vec![(thread, target)],
    };
    entries.reserve(1);
    // SAFETY: the caller keeps the target idle and loaded during installation.
    unsafe { memory::write(address, &entry.patch.original, &entry.patch.replacement)? };
    entries.push(entry);
    Ok(())
}

pub(crate) unsafe fn remove(source: usize) -> Result<(), Error> {
    let mut entries = lock(&ENTRIES);
    let index = entries
        .iter()
        .position(|entry| entry.patch.entry == source)
        .expect("local patch is missing");
    let entry = &mut entries[index];
    if entry.routes.len() == 1 {
        // SAFETY: the last owner stops calls before restoring and freeing code.
        unsafe {
            memory::write(
                entry.patch.address,
                &entry.patch.replacement,
                &entry.patch.original,
            )?
        };
        entries.remove(index);
    } else {
        let thread = std::thread::current().id();
        entry.routes.retain(|route| route.0 != thread);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
