#[cfg(unix)]
mod trash_unix;

#[cfg(windows)]
mod trash_windows;

#[cfg(unix)]
pub use trash_unix::*;

#[cfg(windows)]
pub use trash_windows::*;
