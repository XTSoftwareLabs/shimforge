use super::*;

async fn value(seed: u64) -> u64 {
    seed + 1
}

fn poll(seed: u64) -> Poll<u64> {
    let mut context = Context::from_waker(std::task::Waker::noop());
    std::pin::pin!(value(seed)).as_mut().poll(&mut context)
}

#[test]
fn local_async_duplicates_and_original_polls_are_checked() {
    let _serial = crate::tests::serial();
    {
        let mut session = Session::new_global().unwrap();
        session
            .mock_async(value(1))
            .unwrap()
            .expect()
            .once()
            .returns(30)
            .unwrap();
        assert_eq!(poll(1), Poll::Ready(30));
    }
    let mut session = Session::new_local().unwrap();
    let mock = session.mock_async(value(1)).unwrap();
    mock.expect().once().returns(40).unwrap();
    assert!(session.mock_async(value(1)).is_err());
    assert_eq!(poll(1), Poll::Ready(40));
    assert_eq!(
        std::thread::spawn(|| poll(1)).join().unwrap(),
        Poll::Ready(2)
    );
    session.restore().unwrap();
    assert_eq!(poll(1), Poll::Ready(2));
}
