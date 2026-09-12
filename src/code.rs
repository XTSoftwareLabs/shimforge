#[cfg(any(test, target_arch = "aarch64"))]
mod arm64;
#[cfg(target_arch = "aarch64")]
pub(crate) use arm64::*;
#[cfg(target_arch = "x86_64")]
mod x86;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86::*;
