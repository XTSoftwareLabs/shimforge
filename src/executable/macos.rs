#[cfg(target_arch = "x86_64")]
use crate::Error;

pub(super) use super::unix::{allocate, release, seal};

#[cfg(target_arch = "x86_64")]
pub(super) fn shadow_stack() -> Result<bool, Error> {
    // Intel macOS has no user-space CET shadow stacks.
    Ok(false)
}
