use super::*;

fn target(value: u64) -> u64 {
    value.wrapping_add(5)
}

#[test]
fn invalid_and_overlapping_entries_leave_the_first_route_intact() {
    let _serial = crate::tests::serial();
    let target = target as *const () as usize;
    let mut page = Executable::near(target).unwrap();
    page.publish(&[0x90; 64]).unwrap();
    let source = page.address() + 16;
    // SAFETY: test-owned code is never called while these routes are installed.
    unsafe {
        assert_eq!(install(source, source), Err(Error::SameAddress));
        assert_eq!(install(source, 0), Err(Error::InvalidAddress));
        assert_eq!(install(0, target), Err(Error::InvalidAddress));
        install(source, target).unwrap();
        assert!(route(target).is_some_and(|address| address != target));
        assert_eq!(install(source, target), Err(Error::Overlap));
        assert_eq!(install(source + 1, target), Err(Error::Overlap));
        assert_eq!(install(page.address(), source), Err(Error::Overlap));
        // A different dispatcher must not hide a prefix that overlaps the first.
        let other = Executable::near as *const () as usize;
        assert_eq!(install(source - 1, other), Err(Error::Overlap));
        assert_eq!(install(page.address(), target), Err(Error::Overlap));
        remove(source);
    }
    let saved = route(target).unwrap();
    LOCAL.with(|routes| {
        let _borrow = routes.borrow_mut();
        assert_eq!(route(target), Some(saved));
    });
    assert_eq!(global(source, source), Err(Error::SameAddress));
    assert!(global(source, 0).is_err());
    assert_eq!(global(source + 1, target), Err(Error::Overlap));
    assert_eq!(global(page.address(), source), Err(Error::Overlap));
    assert!(overlaps(source - 1, 2));
    assert_eq!(global(source, target), Ok(true));
    assert_eq!(global(source, target), Err(Error::Overlap));
    assert_eq!(route(target), Some(target));
    remove_global(source);
    assert_eq!(route(target), Some(saved));
    std::mem::forget(page);
}

#[test]
fn conditional_entry_branches_execute_on_unmocked_threads() {
    let _serial = crate::tests::serial();
    extern "C" fn fake(_: i32) -> i32 {
        99
    }
    let mut page = Executable::near(fake as *const () as usize).unwrap();
    let argument = if cfg!(target_os = "windows") {
        0xc9
    } else {
        0xff
    };
    page.publish(&[
        0x31, 0xc0, 0x85, argument, 0x74, 6, 0xb8, 42, 0, 0, 0, 0xc3, 0xc3,
    ])
    .unwrap();
    // SAFETY: the page contains a complete C function with this signature.
    let source: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(page.address()) };
    assert_eq!(source(0), 0);
    assert_eq!(source(1), 42);
    let mut session = crate::Session::new().unwrap();
    crate::replace!(session, source => fake, extern "C" fn(i32) -> i32).unwrap();
    assert_eq!(source(0), 99);
    std::thread::spawn(move || {
        assert_eq!(source(0), 0);
        assert_eq!(source(1), 42);
    })
    .join()
    .unwrap();
    session.restore().unwrap();
    assert_eq!(source(1), 42);
    drop(session);
    let mut session = crate::Session::new_global().unwrap();
    crate::replace!(session, source => fake, extern "C" fn(i32) -> i32).unwrap();
    assert_eq!(source(1), 99);
    session.restore().unwrap();
    assert_eq!(source(1), 42);
    std::mem::forget(page);
}

#[test]
fn a_relocated_call_returns_to_the_original_function() {
    let _serial = crate::tests::serial();
    extern "C" fn callee(value: i32) -> i32 {
        value + 4
    }
    extern "C" fn replacement(_: i32) -> i32 {
        99
    }
    let mut page = Executable::near(callee as *const () as usize).unwrap();
    let mut bytes = if cfg!(target_os = "windows") {
        vec![0x48, 0x83, 0xec, 0x28]
    } else {
        vec![0x50]
    };
    let mut call = code::jump(page.address() + bytes.len(), callee as *const () as usize).unwrap();
    assert_eq!(call.len(), 5);
    call[0] = 0xe8;
    bytes.extend_from_slice(&call);
    if cfg!(target_os = "windows") {
        bytes.extend_from_slice(&[0x48, 0x83, 0xc4, 0x28]);
    } else {
        bytes.push(0x59);
    }
    bytes.push(0xc3);
    page.publish(&bytes).unwrap();
    // SAFETY: the page is a complete C function with this signature.
    let source: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(page.address()) };
    assert_eq!(source(1), 5);
    let mut session = crate::Session::new().unwrap();
    crate::replace!(session, source => replacement, extern "C" fn(i32) -> i32).unwrap();
    assert_eq!(source(1), 99);
    assert_eq!(std::thread::spawn(move || source(1)).join().unwrap(), 5);
    session.restore().unwrap();
    assert_eq!(source(1), 5);
    std::mem::forget(page);
}

#[test]
fn unsupported_prefixes_do_not_leave_routes() {
    let _serial = crate::tests::serial();
    let target = unsupported_prefixes_do_not_leave_routes as *const () as usize;
    let mut page = Executable::near(target).unwrap();
    page.publish(&[0xe3, 0, 0x90, 0x90, 0x90, 0xc3]).unwrap();
    // SAFETY: this test-owned entry is inspected but never called.
    let result = unsafe { install(page.address(), target) };
    assert_eq!(result, Err(Error::InvalidInstruction));
    assert_eq!(route(target), None);
}

#[test]
fn shared_routes_outlive_the_first_owner() {
    let _serial = crate::tests::serial();
    let mut session = crate::Session::new_local().unwrap();
    let mock = crate::mock!(session, target, fn(u64) -> u64).unwrap();
    mock.expect().once().returns(10).unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (run_tx, run_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut session = crate::Session::new_local().unwrap();
        let mock = crate::mock!(session, target, fn(u64) -> u64).unwrap();
        mock.expect().once().returns(20).unwrap();
        ready_tx.send(()).unwrap();
        run_rx.recv().unwrap();
        assert_eq!(target(1), 20);
    });
    ready_rx.recv().unwrap();
    assert_eq!(target(1), 10);
    session.restore().unwrap();
    assert_eq!(target(1), 6);
    run_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(target(1), 6);
}
