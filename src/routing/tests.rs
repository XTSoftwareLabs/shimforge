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
        assert_eq!(route(target), Some(target));
        assert_eq!(install(source, target), Err(Error::Overlap));
        assert_eq!(install(source + 1, target), Err(Error::Overlap));
        assert_eq!(install(page.address(), source), Err(Error::Overlap));
        // A different dispatcher must not hide a prefix that overlaps the first.
        let other = Executable::near as *const () as usize;
        assert_eq!(install(source - 1, other), Err(Error::Overlap));
        assert_eq!(install(page.address(), target), Err(Error::Overlap));
        remove(source).unwrap();
    }
    assert_eq!(route(target), None);
}

#[test]
fn unsupported_prefixes_do_not_leave_routes() {
    let _serial = crate::tests::serial();
    let target = target as *const () as usize;
    let mut page = Executable::near(target).unwrap();
    page.publish(&[0xe8, 0, 0, 0, 0, 0xc3]).unwrap();
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
