use super::*;

#[test]
fn parses_permissions_and_holes() {
    let maps =
        "1000-2000 rwxp 0000 00:00 0\n3000-4000 ---s 0000 00:00 0\n5000-6000 r-xp 0000 00:00 0\n";
    let region = parse_region(maps, 0x1234).unwrap().unwrap();
    assert_eq!(
        region.protection,
        (libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC) as u32
    );
    let region = parse_region(maps, 0x3333).unwrap().unwrap();
    assert_eq!(region.protection, 0);
    assert!(!region.readable && !region.executable);
    assert!(parse_region(maps, 0x2500).unwrap().is_none());
    assert!(parse_region("", 0x1000).unwrap().is_none());
}

#[test]
fn rejects_malformed_maps() {
    for maps in [
        "\n",
        "garbage r-xp",
        "z-2000 r-xp",
        "1000-z r-xp",
        "2000-1000 r-xp",
        "1000-2000",
        "1000-2000 xr-p",
        "1000-2000 rx-p",
        "1000-2000 rw-px",
        "1000-2000 rwyp",
        "1000-2000 rwxy",
    ] {
        assert!(
            matches!(parse_region(maps, 0x1000), Err(Error::Mapping(_))),
            "{maps:?}"
        );
    }
}

#[test]
fn invalid_kernel_reads_and_protection_changes_return_errors() {
    assert!(matches!(
        read_bytes(1, 1),
        Err(Error::Os {
            operation: "read memory",
            ..
        })
    ));
    assert!(matches!(
        protect(
            PageRange {
                address: 1,
                length: 1,
                protection: 0
            },
            true
        ),
        Err(Error::Os {
            operation: "protect memory",
            ..
        })
    ));
}
