//! Configuration read from environment variables, tested without setting any.
//!
//! In the 2024 edition `env::set_var` is `unsafe`, and it changes the environment
//! of the whole test process, so tests that set variables race each other unless
//! they run one at a time. Mocking `env::var` gives each test thread its own
//! values instead: nothing is written, no `unsafe` is needed, and the tests below
//! still run in parallel.

#![forbid(unsafe_code)]

use shimforge::{Session, mock};
use std::env::{self, VarError};
use std::sync::Barrier;
use std::thread;

// Business code below reads the process environment directly and stays unchanged.

#[derive(Debug, PartialEq)]
enum LogLevel {
    Error,
    Info,
    Debug,
}

fn log_level() -> LogLevel {
    match env::var("ORDERS_LOG_LEVEL").as_deref() {
        Ok("error") => LogLevel::Error,
        Ok("debug") => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

fn database_url() -> Result<String, String> {
    let host = env::var("ORDERS_DB_HOST").map_err(|error| format!("ORDERS_DB_HOST: {error}"))?;
    let port = env::var("ORDERS_DB_PORT").unwrap_or_else(|_| "5432".to_owned());
    Ok(format!("postgres://{host}:{port}/orders"))
}

#[test]
fn a_log_level_is_read_without_setting_the_variable() {
    let mut session = Session::new();
    let var = mock!(
        session,
        env::var::<&str>,
        fn(&str) -> Result<String, VarError>
    );
    var.expect()
        .with(|name| *name == "ORDERS_LOG_LEVEL")
        .once()
        .returns(Ok("debug".to_owned()));

    assert_eq!(log_level(), LogLevel::Debug);
    session.verify();
}

#[test]
fn parallel_threads_each_see_their_own_value() {
    let start = Barrier::new(3);
    thread::scope(|scope| {
        for (value, level) in [
            ("error", LogLevel::Error),
            ("info", LogLevel::Info),
            ("debug", LogLevel::Debug),
        ] {
            let start = &start;
            scope.spawn(move || {
                let mut session = Session::new();
                let var = mock!(
                    session,
                    env::var::<&str>,
                    fn(&str) -> Result<String, VarError>
                );
                var.expect().times(100).returns(Ok(value.to_owned()));
                // Start reading together so the three threads really overlap.
                start.wait();
                for _ in 0..100 {
                    assert_eq!(log_level(), level);
                }
                session.verify();
            });
        }
    });
}

#[test]
fn an_unset_port_falls_back_to_the_default() {
    let mut session = Session::new();
    let var = mock!(
        session,
        env::var::<&str>,
        fn(&str) -> Result<String, VarError>
    );
    var.expect()
        .with(|name| *name == "ORDERS_DB_HOST")
        .once()
        .returns(Ok("orders-db.internal".to_owned()));
    var.expect()
        .with(|name| *name == "ORDERS_DB_PORT")
        .once()
        .returns(Err(VarError::NotPresent));

    assert_eq!(
        database_url().unwrap(),
        "postgres://orders-db.internal:5432/orders"
    );
    session.verify();
}

#[test]
fn a_missing_host_is_reported_by_name() {
    let mut session = Session::new();
    let var = mock!(
        session,
        env::var::<&str>,
        fn(&str) -> Result<String, VarError>
    );
    // Reading any other variable would be an unmatched call and fail the test.
    var.expect()
        .with(|name| *name == "ORDERS_DB_HOST")
        .once()
        .returns(Err(VarError::NotPresent));

    assert_eq!(
        database_url().unwrap_err(),
        "ORDERS_DB_HOST: environment variable not found"
    );
    session.verify();
}

#[test]
fn threads_without_a_session_read_the_real_environment() {
    let mut session = Session::new();
    let var = mock!(
        session,
        env::var::<&str>,
        fn(&str) -> Result<String, VarError>
    );
    var.expect().returns(Ok("debug".to_owned()));
    assert_eq!(log_level(), LogLevel::Debug);

    // Nothing was written to the process environment, so other threads see it unset.
    assert_eq!(thread::spawn(log_level).join().unwrap(), LogLevel::Info);
    assert_eq!(
        thread::spawn(|| env::var("ORDERS_LOG_LEVEL"))
            .join()
            .unwrap(),
        Err(VarError::NotPresent)
    );
}
