use super::*;

#[test]
fn imported_functions_have_executable_leaf_regions() {
    let address = libc::gethostname as *const () as usize;
    let region = region_at(address).unwrap().unwrap();
    assert!(region.readable && region.executable);
    assert!(region.start <= address && address < region.end);
    assert_eq!(read_bytes(address, 8).unwrap().len(), 8);
}

#[test]
fn region_layout_matches_mach() {
    assert_eq!(size_of::<RegionInfo>(), 64);
    assert_eq!(align_of::<RegionInfo>(), 4);
    assert!(region_at(usize::MAX).unwrap().is_none());
    assert!(region_at(1).unwrap().is_none());
}

#[test]
fn kernel_errors_keep_the_mach_status() {
    assert_eq!(
        check("query memory", FAILURE),
        Err(Error::Os {
            operation: "query memory",
            code: FAILURE,
        })
    );
    assert!(read_bytes(1, 1).is_err());
}
