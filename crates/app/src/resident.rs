#[cfg(unix)]
mod resident_unix;

#[cfg(windows)]
mod resident_windows;

#[cfg(unix)]
pub use resident_unix::*;

#[cfg(windows)]
pub use resident_windows::*;
