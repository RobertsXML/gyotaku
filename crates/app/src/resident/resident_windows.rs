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
