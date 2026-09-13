//! Home-directory handling tested without writing `HOME`.
//!
//! Tests that point `HOME` somewhere else usually call `env::set_var`, which is
//! `unsafe` in the 2024 edition and changes the environment of the whole test
//! process. Tests running in parallel then read each other's value and fail at
//! random, so projects add environment locks, run those tests one at a time, or
//! change the code to take the home directory as a parameter. Mocking
//! `env::var_os` gives each test thread its own `HOME` instead: nothing is
//! written, no `unsafe` is needed, and the tests below still run in parallel.

#![forbid(unsafe_code)]

use shimforge::{Session, mock};
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::thread;

// Business code below reads the process environment directly and stays unchanged.

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Shows a path with the home directory shortened to `~`.
fn display_path(path: &Path) -> String {
    let Some(home) = home_dir() else {
        return path.display().to_string();
    };
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => {
            let parts: Vec<_> = rest.iter().map(|part| part.to_string_lossy()).collect();
            format!("~/{}", parts.join("/"))
        }
        Err(_) => path.display().to_string(),
    }
}

#[test]
fn a_path_under_home_is_shortened() {
    let mut session = Session::new();
    let var_os = mock!(session, env::var_os::<&str>, fn(&str) -> Option<OsString>);
    var_os
        .expect()
        .with(|name| *name == "HOME")
        .once()
        .returns(Some(OsString::from("/home/ops")));

    assert_eq!(
        display_path(Path::new("/home/ops/projects/shimforge")),
        "~/projects/shimforge"
    );
    session.verify();
}

#[test]
fn home_itself_is_shown_as_a_tilde() {
    let mut session = Session::new();
    let var_os = mock!(session, env::var_os::<&str>, fn(&str) -> Option<OsString>);
    var_os
        .expect()
        .with(|name| *name == "HOME")
        .once()
        .returns(Some(OsString::from("/home/ops")));

    assert_eq!(display_path(Path::new("/home/ops")), "~");
    session.verify();
}

#[test]
fn a_sibling_that_shares_the_prefix_is_left_alone() {
    let mut session = Session::new();
    let var_os = mock!(session, env::var_os::<&str>, fn(&str) -> Option<OsString>);
    var_os
        .expect()
        .with(|name| *name == "HOME")
        .once()
        .returns(Some(OsString::from("/home/ops")));

    assert_eq!(
        display_path(Path::new("/home/opsx/notes")),
        "/home/opsx/notes"
    );
    session.verify();
}

#[test]
fn an_unset_or_empty_home_leaves_paths_alone() {
    let mut session = Session::new();
    let var_os = mock!(session, env::var_os::<&str>, fn(&str) -> Option<OsString>);
    var_os.expect().once().returns(None);
    var_os.expect().once().returns(Some(OsString::new()));

    assert_eq!(display_path(Path::new("/home/ops/logs")), "/home/ops/logs");
    assert_eq!(display_path(Path::new("/home/ops/logs")), "/home/ops/logs");
    session.verify();
}

#[test]
fn parallel_threads_each_get_their_own_home() {
    let start = Barrier::new(3);
    thread::scope(|scope| {
        for (home, shown) in [
            ("/home/ci", "~/logs"),
            ("/home", "~/ci/logs"),
            ("/srv", "/home/ci/logs"),
        ] {
            let start = &start;
            scope.spawn(move || {
                let mut session = Session::new();
                let var_os = mock!(session, env::var_os::<&str>, fn(&str) -> Option<OsString>);
                var_os
                    .expect()
                    .with(|name| *name == "HOME")
                    .times(100)
                    .returns(Some(OsString::from(home)));
                // Start reading together so the three threads really overlap.
                start.wait();
                for _ in 0..100 {
                    assert_eq!(display_path(Path::new("/home/ci/logs")), shown);
                }
                session.verify();
            });
        }
    });
}

#[test]
fn other_threads_still_see_the_real_home() {
    let mut session = Session::new();
    let var_os = mock!(session, env::var_os::<&str>, fn(&str) -> Option<OsString>);
    var_os
        .expect()
        .with(|name| *name == "HOME")
        .returns(Some(OsString::from("/home/ops")));
    assert_eq!(display_path(Path::new("/home/ops")), "~");

    // Nothing was written to the process environment.
    let real = thread::spawn(|| env::var_os("HOME")).join().unwrap();
    assert_ne!(real, Some(OsString::from("/home/ops")));
}
