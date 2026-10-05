//! The parts of onboarding and settings that aren't drawing: working out where
//! screenshot tools save, counting what's there, and running the watcher as a
//! systemd user service.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
const SERVICE: &str = "gyotaku-watch.service";

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    /// Why it's suggested, shown next to it.
    pub why: String,
    /// Set when a screenshot tool is configured to save here, which is the
    /// strongest hint there is.
    pub tool: bool,
}

/// Folders worth offering, most specific first: wherever an installed
/// screenshot tool is set to save, then the usual defaults. Only ones that
/// exist, and each only once.
pub fn candidates() -> Vec<Candidate> {
    let home = match directories::BaseDirs::new() {
        Some(d) => d.home_dir().to_path_buf(),
        None => return Vec::new(),
    };
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));

    let mut found = Vec::new();
    let mut add = |path: Option<PathBuf>, why: &str, tool: bool| {
        let Some(path) = path else { return };
        if path.is_dir() && !found.iter().any(|c: &Candidate| c.path == path) {
            found.push(Candidate {
                path,
                why: why.into(),
                tool,
            });
        }
    };

    // Where the common tools are told to save. They're all described the same
    // way on screen, the person knows which tool they use.
    let tool = "your screenshot tool saves here";
    let read = |p: PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let env_dir = |var: &str| std::env::var(var).ok().map(|v| expand(&v, &home));
    add(env_dir("XDG_SCREENSHOTS_DIR"), tool, true);
    add(
        niri_folder(&read(config.join("niri/config.kdl")), &home),
        tool,
        true,
    );
    add(
        ini_value(&read(config.join("flameshot/flameshot.ini")), "savePath")
            .map(|v| expand(&v, &home)),
        tool,
        true,
    );
    add(
        ini_value(&read(config.join("spectaclerc")), "imageSaveLocation")
            .map(|v| expand(v.trim_start_matches("file://"), &home)),
        tool,
        true,
    );
    add(
        ini_value(&read(config.join("ksnip/ksnip.conf")), "SaveDirectory")
            .map(|v| expand(&v, &home)),
        tool,
        true,
    );
    add(env_dir("GRIM_DEFAULT_DIR"), tool, true);
    add(env_dir("HYPRSHOT_DIR"), tool, true);
    let pictures = gyotaku_core::pictures_dir();
    add(
        pictures.as_ref().map(|p| p.join("Screenshots")),
        "where most desktops save screenshots",
        false,
    );
    add(pictures, "your pictures folder", false);
    add(Some(home.join("Desktop")), "your desktop", false);
    found
}

/// niri's `screenshot-path "~/Pictures/Screenshots/Screenshot from %Y.png"`,
/// minus the file name pattern.
fn niri_folder(config: &str, home: &Path) -> Option<PathBuf> {
    let line = config
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("screenshot-path") && !l.starts_with("//"))?;
    let value = line.split('"').nth(1)?;
    Some(expand(value, home).parent()?.to_path_buf())
}

/// `key=value` out of an ini style file, whichever section it's in.
fn ini_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key && !v.trim().is_empty()).then(|| v.trim().to_owned())
    })
}

fn expand(path: &str, home: &Path) -> PathBuf {
    let path = path.trim().trim_matches('"');
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_path_buf(),
        None => PathBuf::from(path.replace("$HOME", &home.to_string_lossy())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Counts {
    pub images: usize,
    /// Images whose names look like a screenshot tool made them.
    pub screenshots: usize,
}

/// Images under a folder, counted up to `cap` so a huge photo library
/// doesn't hold the answer up.
pub fn count_images(dir: &Path, cap: usize) -> Counts {
    let mut counts = Counts::default();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(t) if t.is_dir() && !name.starts_with('.') => stack.push(path),
                Ok(t) if t.is_file() && is_image(&path) => {
                    counts.images += 1;
                    counts.screenshots += looks_like_a_screenshot(&name) as usize;
                    if counts.images >= cap {
                        return counts;
                    }
                }
                _ => {}
            }
        }
    }
    counts
}

/// Screenshot tools name files after what they are or when they were taken:
/// `Screenshot from 2025-10-01 15-48-28.png`, `2026-06-07_19-06-02.png`,
/// `shot_1779298058.png`. Photos from a camera or a download rarely look
/// like either.
pub fn looks_like_a_screenshot(name: &str) -> bool {
    let lower = name.to_lowercase();
    if ["screenshot", "screen shot", "shot", "capture", "grim"]
        .iter()
        .any(|w| lower.contains(w))
    {
        return true;
    }
    // a yyyy-mm-dd (or yyyy_mm_dd) date anywhere in the name
    let b = lower.as_bytes();
    b.windows(10).any(|w| {
        let digits = |r: std::ops::Range<usize>| w[r].iter().all(u8::is_ascii_digit);
        digits(0..4) && digits(5..7) && digits(8..10) && matches!(w[4], b'-' | b'_') && w[4] == w[7]
    })
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Drops folders that sit inside another picked folder, since the outer one
/// already covers them.
pub fn without_nested(mut folders: Vec<PathBuf>) -> Vec<PathBuf> {
    folders.sort();
    folders.dedup();
    let all = folders.clone();
    folders.retain(|f| !all.iter().any(|o| o != f && f.starts_with(o)));
    folders
}

/// Total size of the files in a folder, not following links.
pub fn folder_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(m) if m.is_dir() => stack.push(entry.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Running,
    Stopped,
}

/// Whether there's a systemd user manager to hand the watcher to. The same
/// test sd_booted(3) does, plus the user instance's socket, since having the
/// systemctl binary around proves nothing (containers, systemd installed but
/// not running as init).
fn has_systemd() -> bool {
    Path::new("/run/systemd/system").is_dir()
        && std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|dir| Path::new(&dir).join("systemd/private").exists())
}

/// Running if the service is up, or if a `gyotaku watch` started some other
/// way (by hand, by the autostart entry) is.
pub fn service_status() -> Service {
    let service = has_systemd() && systemctl(&["is-active", SERVICE]) == Some(true);
    if service || !watcher_pids().is_empty() {
        Service::Running
    } else {
        Service::Stopped
    }
}

/// Starts the watcher and makes it start again at every login: a systemd
/// user service where there's systemd, an XDG autostart entry anywhere else.
/// Every desktop that follows the freedesktop specs runs those.
pub fn start_service() -> bool {
    if has_systemd() {
        if install_unit().is_err() {
            return false;
        }
        let _ = systemctl(&["daemon-reload"]);
        return systemctl(&["enable", "--now", SERVICE]) == Some(true);
    }
    if install_autostart().is_err() {
        return false;
    }
    if watcher_pids().is_empty() {
        spawn_watcher()
    } else {
        true
    }
}

pub fn stop_service() -> bool {
    #[cfg(windows)]
    {
        return true;
    }

    #[cfg(not(windows))]
    {
        #[cfg(windows)]
            {
                return true;
            }
        
            #[cfg(not(windows))]
            {
                if let Some(entry) = autostart_path() {
                        let _ = std::fs::remove_file(entry);
                    }
                    if has_systemd() {
                        return systemctl(&["disable", "--now", SERVICE]) == Some(true);
                    }
                    for pid in watcher_pids() {
                        unsafe { libc::kill(pid, libc::SIGTERM) };
                    }
                    true
            }
    }
}

/// The command line tool, installed next to this binary by `cargo install`
/// and by packages alike. Pointing at it directly beats hoping it's on the
/// PATH of whatever starts it.
fn cli_path() -> String {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("gyotaku"))
        .filter(|cli| cli.exists())
        .map_or_else(|| "gyotaku".into(), |cli| cli.display().to_string())
}

/// `setsid -f` forks the watcher off into its own session, so it isn't a
/// child of this window, doesn't die with it, and never lingers as a zombie.
/// Busybox's setsid has no -f, so failing that it's started directly.
fn spawn_watcher() -> bool {
    let quiet = |c: &mut Command| {
        c.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    };
    let mut detached = Command::new("setsid");
    detached.args(["-f", &cli_path(), "watch"]);
    quiet(&mut detached);
    if detached.status().is_ok_and(|s| s.success()) {
        return true;
    }
    let mut direct = Command::new(cli_path());
    direct.arg("watch");
    quiet(&mut direct);
    direct.spawn().is_ok()
}

/// Processes running `gyotaku watch`, found by reading /proc.
fn watcher_pids() -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
            let cmdline = std::fs::read(entry.path().join("cmdline")).ok()?;
            let mut args = cmdline.split(|b| *b == 0);
            let program = Path::new(std::str::from_utf8(args.next()?).ok()?);
            let is_watcher = program.file_name()? == "gyotaku" && args.next()? == b"watch";
            is_watcher.then_some(pid)
        })
        .collect()
}

fn autostart_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join(".config")))?;
    Some(config.join("autostart/gyotaku-watch.desktop"))
}

fn install_autostart() -> std::io::Result<()> {
    let path = autostart_path().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    std::fs::write(path, autostart_entry(&cli_path()))
}

fn autostart_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=gyotaku watcher\n\
         Comment=Reads new screenshots so they can be searched\n\
         Exec={exec} watch\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// None if systemctl couldn't be run at all, otherwise whether it succeeded.
fn systemctl(args: &[&str]) -> Option<bool> {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .map(|s| s.success())
}

fn install_unit() -> std::io::Result<()> {
    let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) else {
        return Err(std::io::ErrorKind::NotFound.into());
    };
    let unit = home.join(".config/systemd/user").join(SERVICE);
    if unit.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(unit.parent().expect("has a parent"))?;
    std::fs::write(&unit, unit_file(&cli_path()))
}

fn unit_file(exec: &str) -> String {
    format!(
        "[Unit]\n\
         Description=gyotaku, reads new screenshots so they can be searched\n\
         Documentation=https://github.com/xevrion/gyotaku\n\
         \n\
         [Service]\n\
         ExecStart={exec} watch\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         Nice=19\n\
         CPUSchedulingPolicy=idle\n\
         IOSchedulingClass=idle\n\
         MemoryHigh=400M\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niri_path_loses_its_file_pattern() {
        let config = r#"
            // screenshot-path "~/old/%Y.png"
            screenshot-path "~/Pictures/Screenshots/Screenshot from %Y-%m-%d.png"
        "#;
        let home = Path::new("/home/me");
        assert_eq!(
            niri_folder(config, home),
            Some("/home/me/Pictures/Screenshots".into())
        );
        assert_eq!(niri_folder("binds {}", home), None);
    }

    #[test]
    fn ini_values_are_found_in_any_section() {
        let ini = "[General]\nsavePath=/home/me/shots\nother=1\n";
        assert_eq!(ini_value(ini, "savePath"), Some("/home/me/shots".into()));
        assert_eq!(ini_value("[General]\nsavePath=\n", "savePath"), None);
    }

    #[test]
    fn home_is_expanded() {
        let home = Path::new("/home/me");
        assert_eq!(expand("~/a", home), PathBuf::from("/home/me/a"));
        assert_eq!(expand("$HOME/b", home), PathBuf::from("/home/me/b"));
        assert_eq!(expand("/abs", home), PathBuf::from("/abs"));
    }

    #[test]
    fn nested_folders_are_covered_by_their_parent() {
        let folders = vec![
            PathBuf::from("/p/Screenshots"),
            PathBuf::from("/p"),
            PathBuf::from("/q"),
            PathBuf::from("/p"),
            PathBuf::from("/pq"),
        ];
        assert_eq!(
            without_nested(folders),
            [PathBuf::from("/p"), "/pq".into(), "/q".into()]
        );
    }

    #[test]
    fn counting_stops_at_the_cap_and_skips_hidden() {
        let dir = std::env::temp_dir().join(format!("gyotaku-count-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        for name in [
            "a.png",
            "Screenshot 1.JPG",
            "sub/2026-01-02.webp",
            ".hidden/d.png",
            "notes.txt",
        ] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(
            count_images(&dir, 100),
            Counts {
                images: 3,
                screenshots: 2
            }
        );
        assert_eq!(count_images(&dir, 2).images, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn screenshot_names() {
        for name in [
            "Screenshot from 2025-10-01 15-48-28.png",
            "2026-06-07_19-06-02.png",
            "shot_1779298058.png",
            "utsushot_1786555075.png",
            "2026_01_02 thing.png",
        ] {
            assert!(looks_like_a_screenshot(name), "{name}");
        }
        for name in [
            "IMG_4032.jpg",
            "cookmarked.png",
            "99adebae-fa8d-441f-9606.jpeg",
            "2026-0102.png",
        ] {
            assert!(!looks_like_a_screenshot(name), "{name}");
        }
    }

    #[test]
    fn the_autostart_entry_runs_the_watcher_hidden() {
        let entry = autostart_entry("/usr/bin/gyotaku");
        assert!(entry.contains("Exec=/usr/bin/gyotaku watch"));
        assert!(entry.contains("NoDisplay=true"));
    }

    #[test]
    fn the_unit_runs_the_watcher_at_idle() {
        let unit = unit_file("/x/gyotaku");
        assert!(unit.contains("ExecStart=/x/gyotaku watch"));
        assert!(unit.contains("CPUSchedulingPolicy=idle"));
    }
}
