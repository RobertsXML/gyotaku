use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Trashed {
    pub original: PathBuf,
    pub file: PathBuf,
    info: PathBuf,
}

pub fn trash(path: &Path) -> Result<Trashed> {
    let path = std::path::absolute(path)?;
    let base = directories::BaseDirs::new().context("could not work out a home directory")?;
    let root = base.data_dir().join("gyotaku").join("Trash");
    let files = root.join("files");
    let infos = root.join("info");
    fs::create_dir_all(&files)?;
    fs::create_dir_all(&infos)?;

    let name = path.file_name().context("no file name")?;
    let mut chosen = name.to_owned();
    let mut info = infos.join({
        let mut n = name.to_owned();
        n.push(".trashinfo");
        n
    });

    for n in 1..1000u32 {
        if n > 1 {
            chosen = numbered(name, n);
            let mut info_name = chosen.clone();
            info_name.push(".trashinfo");
            info = infos.join(info_name);
        }
        if !files.join(&chosen).exists() && !info.exists() {
            break;
        }
        if n == 999 {
            bail!("no free name in {}", files.display());
        }
    }

    let dest = files.join(&chosen);
    fs::rename(&path, &dest)
        .or_else(|_| {
            fs::copy(&path, &dest)?;
            fs::remove_file(&path)
        })
        .with_context(|| format!("can't move {} to the trash", path.display()))?;

    if let Err(e) = fs::write(&info, format!("OriginalPath={}\n", path.display())) {
        let _ = fs::rename(&dest, &path)
            .or_else(|_| fs::copy(&dest, &path).and_then(|_| fs::remove_file(&dest)));
        return Err(e).with_context(|| format!("can't write {}", info.display()));
    }

    Ok(Trashed {
        original: path,
        file: dest,
        info,
    })
}

pub fn restore(t: &Trashed) -> Result<()> {
    if fs::symlink_metadata(&t.original).is_ok() {
        bail!("{} exists again", t.original.display());
    }
    if let Some(dir) = t.original.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::rename(&t.file, &t.original)
        .or_else(|_| {
            fs::copy(&t.file, &t.original)?;
            fs::remove_file(&t.file)
        })
        .with_context(|| format!("can't put back {}", t.original.display()))?;
    let _ = fs::remove_file(&t.info);
    Ok(())
}

fn numbered(name: &std::ffi::OsStr, n: u32) -> std::ffi::OsString {
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
