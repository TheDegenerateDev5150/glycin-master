#[cfg(target_os = "linux")]
pub use glycin_external::*;

#[cfg(not(target_os = "linux"))]
pub use glycin_builtin::*;
