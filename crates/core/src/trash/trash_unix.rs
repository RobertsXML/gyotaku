//! The desktop trash, as the freedesktop.org trash spec lays it out, so a
//! screenshot moved there shows up in any file manager's trash and can be
//! restored from it like anything else. Nothing here ever deletes a file:
//! if a screenshot can't be moved to a trash, it stays where it is.

use std::ffi::OsString;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// A screenshot sitting in a trash, and how to put it back.
#[derive(Debug, Clone)]
pub struct Trashed {
    pub original: PathBuf,
    /// Where the file itself is now, in the trash's `files` folder.
    pub file: PathBuf,
    /// The `.trashinfo` that tells file managers where it came from.
    info: PathBuf,
}

/// Moves a file to the trash on the drive it lives on: the home trash for
/// anything on the home drive, otherwise the drive's own `.Trash-<uid>`.
/// Moving between drives would mean copying, so that's never done.
pub fn trash(path: &Path) -> Result<Trashed> {
    let path = std::path::absolute(path)?;
    let device = fs::symlink_metadata(&path)
        .with_context(|| format!("can't read {}", path.display()))?
        .dev();
    let home = home_trash()?;
    if nearest_existing(&home).is_some_and(|d| d.dev() == device) {
        return put(&path, &home, None);
    }
    let top = top_dir(&path, device)?;
    put(&path, &drive_trash(&top)?, Some(&top))
}

/// Puts a trashed file back where it came from. Refuses if something new has
/// taken its place since, rather than overwrite it.
pub fn restore(t: &Trashed) -> Result<()> {
    if fs::symlink_metadata(&t.original).is_ok() {
        bail!("{} exists again", t.original.display());
    }
    if let Some(dir) = t.original.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::rename(&t.file, &t.original)
        .with_context(|| format!("can't put back {}", t.original.display()))?;
    let _ = fs::remove_file(&t.info);
    Ok(())
}

fn home_trash() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("could not work out a home directory")?;
    Ok(base.data_dir().join("Trash"))
}

fn nearest_existing(path: &Path) -> Option<fs::Metadata> {
    path.ancestors().find_map(|p| fs::metadata(p).ok())
}

/// The mount point a file lives under: the last folder above it that's still
/// on the same device.
fn top_dir(path: &Path, device: u64) -> Result<PathBuf> {
    let mut top = path.parent().context("no parent folder")?;
    while let Some(up) = top.parent() {
        if fs::metadata(up).map(|m| m.dev()).ok() != Some(device) {
            break;
        }
        top = up;
    }
    Ok(top.to_path_buf())
}

/// A shared `$top/.Trash/<uid>` if the admin set one up properly (a real
/// folder, sticky, not a link someone could point elsewhere), otherwise this
/// user's own `$top/.Trash-<uid>`.
fn drive_trash(top: &Path) -> Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let shared = top.join(".Trash");
    if let Ok(m) = fs::symlink_metadata(&shared)
        && m.is_dir()
        && m.permissions().mode() & 0o1000 != 0
    {
        let mine = shared.join(uid.to_string());
        if make_private_dir(&mine).is_ok() {
            return Ok(mine);
        }
    }
    let own = top.join(format!(".Trash-{uid}"));
    make_private_dir(&own).with_context(|| format!("can't make a trash at {}", own.display()))?;
    Ok(own)
}

fn make_private_dir(dir: &Path) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let m = fs::symlink_metadata(dir)?;
    if !m.is_dir() || m.uid() != unsafe { libc::getuid() } {
        return Err(io::Error::other("not a folder of ours"));
    }
    Ok(())
}

/// `top` is the drive's root for a drive's own trash, whose info files hold
/// paths relative to it so they still make sense if it mounts elsewhere.
fn put(path: &Path, trash: &Path, top: Option<&Path>) -> Result<Trashed> {
    let (files, infos) = (trash.join("files"), trash.join("info"));
    for dir in [trash, &files, &infos] {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("can't make {}", dir.display()))?;
    }

    let recorded = match top {
        Some(top) => path.strip_prefix(top).unwrap_or(path),
        None => path,
    };
    let now = jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S");
    let body = format!(
        "[Trash Info]\nPath={}\nDeletionDate={now}\n",
        escape(recorded.as_os_str().as_bytes())
    );

    let name = path.file_name().context("no file name")?;
    for n in 1..1000 {
        let name = numbered(name, n);
        let mut info_name = name.clone();
        info_name.push(".trashinfo");
        let info = infos.join(info_name);
        // Creating the info file is what claims a name, the spec's way of
        // making sure two programs trashing at once never collide.
        let mut f = match OpenOptions::new().write(true).create_new(true).open(&info) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("can't write {}", info.display())),
        };
        let file = files.join(&name);
        if fs::symlink_metadata(&file).is_ok() {
            let _ = fs::remove_file(&info);
            continue;
        }
        let moved = f
            .write_all(body.as_bytes())
            .and_then(|_| fs::rename(path, &file));
        if let Err(e) = moved {
            let _ = fs::remove_file(&info);
            return Err(e).with_context(|| format!("can't move {} to the trash", path.display()));
        }
        return Ok(Trashed {
            original: path.to_path_buf(),
            file,
            info,
        });
    }
    bail!("no free name in {}", trash.display())
}

/// `shot.png`, then `shot.2.png`, `shot.3.png`, the way file managers do it.
fn numbered(name: &std::ffi::OsStr, n: u32) -> OsString {
    if n == 1 {
        return name.to_owned();
    }
    let p = Path::new(name);
    let mut out = p.file_stem().unwrap_or(name).to_owned();
    out.push(format!(".{n}"));
    if let Some(ext) = p.extension() {
        out.push(".");
        out.push(ext);
    }
    out
}

/// Percent-encodes a path the way the spec asks (as in a URL), byte by byte,
/// so names that aren't valid UTF-8 survive the round trip.
fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
fn unescape(s: &str) -> OsString {
    use std::os::unix::ffi::OsStringExt as _;
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    OsString::from_vec(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gyotaku-trash-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("shots")).unwrap();
        dir
    }

    #[test]
    fn trashes_and_restores() {
        let dir = scratch("roundtrip");
        let shot = dir.join("shots/Screenshot from 2026-10-05 15:32.png");
        fs::write(&shot, b"png").unwrap();

        let t = put(&shot, &dir.join("Trash"), None).unwrap();
        assert!(!shot.exists());
        assert_eq!(fs::read(&t.file).unwrap(), b"png");
        let info = fs::read_to_string(&t.info).unwrap();
        let path = info.lines().find_map(|l| l.strip_prefix("Path=")).unwrap();
        assert!(!path.contains(' '), "{path}");
        assert_eq!(unescape(path), shot.as_os_str());
        assert!(info.starts_with("[Trash Info]\n"));
        assert!(info.contains("DeletionDate=20"));

        restore(&t).unwrap();
        assert_eq!(fs::read(&shot).unwrap(), b"png");
        assert!(!t.info.exists() && !t.file.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_name_twice_gets_a_number() {
        let dir = scratch("names");
        let shot = dir.join("shots/a.png");
        fs::write(&shot, b"1").unwrap();
        let first = put(&shot, &dir.join("Trash"), None).unwrap();
        fs::write(&shot, b"2").unwrap();
        let second = put(&shot, &dir.join("Trash"), None).unwrap();
        assert_eq!(first.file.file_name().unwrap(), "a.png");
        assert_eq!(second.file.file_name().unwrap(), "a.2.png");
        assert_eq!(fs::read(&second.file).unwrap(), b"2");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn never_restores_over_a_new_file() {
        let dir = scratch("over");
        let shot = dir.join("shots/a.png");
        fs::write(&shot, b"old").unwrap();
        let t = put(&shot, &dir.join("Trash"), None).unwrap();
        fs::write(&shot, b"new").unwrap();
        assert!(restore(&t).is_err());
        assert_eq!(fs::read(&shot).unwrap(), b"new");
        assert!(t.file.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn drive_trash_paths_are_relative_to_the_drive() {
        let dir = scratch("drive");
        let shot = dir.join("shots/a.png");
        fs::write(&shot, b"x").unwrap();
        let t = put(&shot, &dir.join(".Trash-1000"), Some(&dir)).unwrap();
        assert!(
            fs::read_to_string(&t.info)
                .unwrap()
                .contains("\nPath=shots/a.png\n")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn escaping() {
        assert_eq!(escape(b"/home/a b/%.png"), "/home/a%20b/%25.png");
        assert_eq!(escape("/ü".as_bytes()), "/%C3%BC");
        assert_eq!(unescape("/home/a%20b/%25.png"), "/home/a b/%.png");
    }
}
