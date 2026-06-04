#[cfg(target_os = "macos")]
pub(crate) mod macos;

#[cfg(any(target_os = "linux", test))]
pub(crate) mod linux;
