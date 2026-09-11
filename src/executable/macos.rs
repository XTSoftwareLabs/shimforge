use crate::Error;

pub(super) use super::unix::{allocate, release, seal};

pub(super) fn shadow_stack() -> Result<bool, Error> {
    // Intel macOS has no user-space CET shadow stacks.
    Ok(false)
}
