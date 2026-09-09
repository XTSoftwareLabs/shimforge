use super::api::{MemoryInfo, SystemInfo};
use std::mem::{align_of, offset_of, size_of};

#[test]
fn native_structs_match_the_windows_amd64_layout() {
    assert_eq!(size_of::<MemoryInfo>(), 48);
    assert_eq!(align_of::<MemoryInfo>(), 8);
    assert_eq!(offset_of!(MemoryInfo, AllocationProtect), 16);
    assert_eq!(offset_of!(MemoryInfo, PartitionId), 20);
    assert_eq!(offset_of!(MemoryInfo, RegionSize), 24);
    assert_eq!(offset_of!(MemoryInfo, State), 32);
    assert_eq!(offset_of!(MemoryInfo, Protect), 36);
    assert_eq!(size_of::<SystemInfo>(), 48);
    assert_eq!(align_of::<SystemInfo>(), 8);
    assert_eq!(offset_of!(SystemInfo, dwPageSize), 4);
    assert_eq!(offset_of!(SystemInfo, MinimumApplicationAddress), 8);
    assert_eq!(offset_of!(SystemInfo, ActiveProcessorMask), 24);
    assert_eq!(offset_of!(SystemInfo, AllocationGranularity), 40);
    assert_eq!(offset_of!(SystemInfo, ProcessorRevision), 46);
}
