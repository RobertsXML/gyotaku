#[cfg(unix)]
mod unix {
//! Staying resident between searches.
//!
//! Most of a cold start is not ours. Our side is ready in about 60 ms, but
//! setting up the gpu (enumerating adapters, probing the GL driver, testing
//! the device) costs another 300 to 800 ms on this Nvidia laptop, and that's
//! far too slow for something bound to a key. So the first launch keeps the
//! process around after its window closes, and later launches just knock on
//! a socket and leave. The warm process opens a window in a few frames.

use std::io::Write as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("gyotaku.sock"),
        None => std::env::temp_dir().join(format!("gyotaku-{}.sock", unsafe { libc::getuid() })),
    }
}

/// Asks a running instance to toggle its window. False if there isn't one.
pub fn wake(socket: &PathBuf) -> bool {
    match UnixStream::connect(socket) {
        Ok(mut stream) => stream.write_all(b"toggle\n").is_ok(),
        Err(_) => false,
    }
}

/// Becomes the running instance. None if another process won the race to
/// the socket, in which case this one just runs once and exits.
pub fn listen(socket: &PathBuf) -> Option<UnixListener> {
    // Left behind by an instance that crashed or was killed, nobody answered
    // `wake` so nobody is using it.
    let _ = std::fs::remove_file(socket);
    UnixListener::bind(socket).ok()
}

}

#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows {
use std::path::PathBuf;

pub struct Listener;

impl Listener {
    pub fn incoming(&self) -> std::iter::Empty<std::io::Result<()>> {
        std::iter::empty()
    }
}

pub fn socket_path() -> PathBuf {
    std::env::temp_dir().join("gyotaku-windows-instance")
}

pub fn wake(_socket: &PathBuf) -> bool {
    false
}

pub fn listen(_socket: &PathBuf) -> Option<Listener> {
    None
}
}

#[cfg(windows)]
pub use windows::*;
