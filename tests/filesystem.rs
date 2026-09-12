//! Disk-backed code paths exercised without touching the filesystem.

use shimforge::{Session, mock};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

/// Opens a readable handle that is backed by a pipe instead of a file.
fn detached_handle() -> File {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::fd::FromRawFd;
        let mut ends = [0; 2];
        // SAFETY: the array holds room for both descriptors.
        let created = unsafe { libc::pipe(ends.as_mut_ptr()) };
        assert_eq!(created, 0);
        // SAFETY: the writing end is closed once and never used again.
        unsafe { libc::close(ends[1]) };
        // SAFETY: the reading end is a fresh descriptor with no other owner.
        unsafe { File::from_raw_fd(ends[0]) }
    }
    #[cfg(target_os = "windows")]
    {
        use std::ffi::c_void;
        use std::os::windows::io::FromRawHandle;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreatePipe(
                read: *mut *mut c_void,
                write: *mut *mut c_void,
                attributes: *const c_void,
                size: u32,
            ) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        // SAFETY: both out pointers are writable and the attributes are optional.
        let created = unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) };
        assert_ne!(created, 0);
        // SAFETY: the writing end is closed once and never used again.
        unsafe { CloseHandle(write) };
        // SAFETY: the reading end is a fresh handle with no other owner.
        unsafe { File::from_raw_handle(read) }
    }
}

// Business code below calls the standard library directly and stays unchanged.

fn prepare_cache(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| format!("cannot prepare cache: {error}"))?;
    Ok(())
}

fn describe(path: &Path) -> &'static str {
    if path.is_dir() {
        "directory"
    } else if path.exists() {
        "file"
    } else {
        "missing"
    }
}

fn read_header(path: &str) -> io::Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line.trim_end().to_owned())
}

fn append_entry(path: &str, entry: &str) -> io::Result<usize> {
    let mut file = File::open(path)?;
    file.write_all(entry.as_bytes())?;
    Ok(entry.len())
}

#[test]
fn directory_creation_succeeds_without_creating_a_directory() {
    let _serial = serial();
    let root = Path::new("virtual/cache/reports");
    assert!(!root.exists());
    let mut session = Session::new().unwrap();
    let create = mock!(
        session,
        fs::create_dir_all::<&Path>,
        fn(&Path) -> io::Result<()>
    )
    .unwrap();
    create
        .expect()
        .with(|path| **path == *Path::new("virtual/cache/reports"))
        .once()
        .returning(|_| Ok(()))
        .unwrap();

    assert!(prepare_cache(root).is_ok());
    session.verify().unwrap();
    assert!(!root.exists());
}

#[test]
fn directory_creation_failures_reach_the_caller() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let create = mock!(
        session,
        fs::create_dir_all::<&Path>,
        fn(&Path) -> io::Result<()>
    )
    .unwrap();
    create
        .expect()
        .once()
        .returning(|_| Err(io::ErrorKind::PermissionDenied.into()))
        .unwrap();

    let error = prepare_cache(Path::new("virtual/cache/reports")).unwrap_err();
    assert!(error.starts_with("cannot prepare cache:"), "{error}");
}

#[test]
fn path_predicates_answer_for_paths_that_do_not_exist() {
    let _serial = serial();
    let missing = Path::new("virtual/archive");
    assert_eq!(describe(missing), "missing");
    {
        let mut session = Session::new().unwrap();
        let is_dir = mock!(session, Path::is_dir, fn(&Path) -> bool).unwrap();
        is_dir.expect().once().returns(true).unwrap();
        assert_eq!(describe(missing), "directory");
        session.verify().unwrap();
    }
    {
        let mut session = Session::new().unwrap();
        let is_dir = mock!(session, Path::is_dir, fn(&Path) -> bool).unwrap();
        is_dir.expect().once().returns(false).unwrap();
        let exists = mock!(session, Path::exists, fn(&Path) -> bool).unwrap();
        exists
            .expect()
            .with(|path| **path == *Path::new("virtual/archive"))
            .once()
            .returns(true)
            .unwrap();
        assert_eq!(describe(missing), "file");
        session.verify().unwrap();
    }
    assert_eq!(describe(missing), "missing");
}

#[test]
fn a_predicate_mock_can_require_an_exact_number_of_questions() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let is_dir = mock!(session, Path::is_dir, fn(&Path) -> bool).unwrap();
    let expected = is_dir.expect().times(3).returns(true).unwrap();
    for name in ["virtual/a", "virtual/b", "virtual/c"] {
        assert!(Path::new(name).is_dir());
    }
    assert_eq!(expected.calls(), 3);
    session.restore().unwrap();
    assert!(!Path::new("virtual/a").is_dir());
}

#[test]
fn a_line_is_read_from_a_handle_that_never_reaches_disk() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let open = mock!(session, File::open::<&str>, fn(&str) -> io::Result<File>).unwrap();
    open.expect()
        .with(|path| **path == *"virtual/inventory.csv")
        .once()
        .returning(|_| Ok(detached_handle()))
        .unwrap();
    let read_line = mock!(
        session,
        BufReader::<File>::read_line,
        fn(&mut BufReader<File>, &mut String) -> io::Result<usize>
    )
    .unwrap();
    read_line
        .expect()
        .once()
        .returning(|_, line| {
            line.push_str("sku,count,location\n");
            Ok(line.len())
        })
        .unwrap();

    assert_eq!(
        read_header("virtual/inventory.csv").unwrap(),
        "sku,count,location"
    );
    session.verify().unwrap();
}

#[test]
fn a_write_is_accepted_without_a_writable_handle() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let open = mock!(session, File::open::<&str>, fn(&str) -> io::Result<File>).unwrap();
    open.expect()
        .once()
        .returning(|_| Ok(detached_handle()))
        .unwrap();
    let write_all = mock!(
        session,
        File::write_all,
        fn(&mut File, &[u8]) -> io::Result<()>
    )
    .unwrap();
    write_all
        .expect()
        .with(|_, bytes| **bytes == *b"sku-77,3,aisle-2\n")
        .once()
        .returning(|_, _| Ok(()))
        .unwrap();

    assert_eq!(
        append_entry("virtual/inventory.csv", "sku-77,3,aisle-2\n").unwrap(),
        17
    );
    session.verify().unwrap();
}

#[test]
fn a_write_error_is_reported_to_the_caller() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let open = mock!(session, File::open::<&str>, fn(&str) -> io::Result<File>).unwrap();
    open.expect()
        .once()
        .returning(|_| Ok(detached_handle()))
        .unwrap();
    let write_all = mock!(
        session,
        File::write_all,
        fn(&mut File, &[u8]) -> io::Result<()>
    )
    .unwrap();
    write_all
        .expect()
        .once()
        .returning(|_, _| Err(io::ErrorKind::StorageFull.into()))
        .unwrap();

    assert_eq!(
        append_entry("virtual/inventory.csv", "sku-77,3,aisle-2\n")
            .unwrap_err()
            .kind(),
        io::ErrorKind::StorageFull
    );
}
