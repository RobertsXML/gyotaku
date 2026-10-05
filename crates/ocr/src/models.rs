use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub struct Model {
    pub file: &'static str,
    url: &'static str,
    sha256: &'static str,
}

// PP-OCRv6 exported to onnx by the RapidOCR folks. Same files their python
// package downloads, pinned by hash so a changed upstream file fails loudly
// instead of quietly reading text differently.
macro_rules! url {
    ($rest:literal) => {
        concat!(
            "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6",
            $rest
        )
    };
}

// Tiny detector, small recognizer. On 25 of my own screenshots the tiny
// detector ran 4.6x faster than small and kept 99.6% of the words (its misses
// are mostly word gaps, which substring search doesn't care about). The tiny
// recognizer genuinely misreads things (mock -> meek), so rec stays small.
// Numbers are in notes.md.
pub const DET: Model = Model {
    file: "PP-OCRv6_det_tiny.onnx",
    url: url!("/det/PP-OCRv6_det_tiny.onnx"),
    sha256: "f42c0fbd294d95eac1a550e131b277dac97462c8025fa4b6c3cec1b7894bd3d5",
};

pub const REC: Model = Model {
    file: "PP-OCRv6_rec_small.onnx",
    url: url!("/rec/PP-OCRv6_rec_small.onnx"),
    sha256: "6f327246b50388f3c176ae304bd95767ea6dc0c9ae92153ef8cbe210b3c14884",
};

/// ONNX Runtime itself: Microsoft's official Linux build, fetched on first use
/// like the models. It's built against glibc 2.27 and GCC 5's libstdc++, so it
/// loads on anything from Ubuntu 18.04 and Debian 10 on. The prebuilt that the
/// ort crate links statically needs glibc 2.38, which CI showed doesn't even
/// link on Ubuntu 22.04 or Debian 12.
struct Runtime {
    url: &'static str,
    sha256: &'static str,
    /// Where the library sits inside Microsoft's tarball.
    inner: &'static str,
}

const RUNTIME_FILE: &str = "libonnxruntime.so.1.28.2";

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const RUNTIME: Option<Runtime> = Some(Runtime {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.2/onnxruntime-linux-x64-1.28.2.tgz",
    sha256: "d7209b8751b27b862b0c76332c2e20e203396edb5dab700ecf4bb485cf147415",
    inner: "onnxruntime-linux-x64-1.28.2/lib/libonnxruntime.so.1.28.2",
});

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const RUNTIME: Option<Runtime> = Some(Runtime {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.2/onnxruntime-linux-aarch64-1.28.2.tgz",
    sha256: "f020b3d31106cc7db03889b4a5c21e7c38ce4a09ad26119c11d1ad6d3fa0ec04",
    inner: "onnxruntime-linux-aarch64-1.28.2/lib/libonnxruntime.so.1.28.2",
});

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
const RUNTIME: Option<Runtime> = None;

pub fn models_dir() -> Result<PathBuf> {
    Ok(gyotaku_core::data_dir()?.join("models"))
}

/// Returns the path to the model, downloading it first if it isn't there yet.
pub fn ensure(model: &Model) -> Result<PathBuf> {
    let dir = models_dir()?;
    let path = dir.join(model.file);
    if path.exists() {
        return Ok(path);
    }
    eprintln!("downloading {} (first run only)", model.file);
    let bytes = fetch(model.url, model.sha256)?;
    write_atomically(&path, &bytes)?;
    Ok(path)
}

/// The ONNX Runtime library to load. ORT_DYLIB_PATH wins if it's set, which
/// is how a distro package can use its own onnxruntime instead.
pub fn runtime() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH") {
        return Ok(path.into());
    }

    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let bundled = exe.with_file_name("onnxruntime.dll");
        if bundled.exists() {
            return Ok(bundled);
        }
    }
    let path = gyotaku_core::data_dir()?.join("runtime").join(RUNTIME_FILE);
    if path.exists() {
        return Ok(path);
    }
    let Some(runtime) = RUNTIME else {
        bail!(
            "there's no official ONNX Runtime build for this cpu, install onnxruntime \
             and point ORT_DYLIB_PATH at libonnxruntime.so"
        );
    };
    eprintln!("downloading ONNX Runtime 1.28.2 (first run only)");
    let archive = fetch(runtime.url, runtime.sha256)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive.as_slice()));
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new(runtime.inner) {
            let mut lib = Vec::new();
            entry.read_to_end(&mut lib)?;
            write_atomically(&path, &lib)?;
            return Ok(path);
        }
    }
    bail!("{} wasn't in the ONNX Runtime archive", runtime.inner)
}

/// Downloads into memory and checks the hash before anything touches disk, so
/// a changed or truncated file upstream fails loudly instead of quietly
/// reading text differently.
fn fetch(url: &str, sha256: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    ureq::get(url)
        .call()
        .with_context(|| format!("downloading {url}"))?
        .into_body()
        .into_reader()
        .read_to_end(&mut bytes)
        .with_context(|| format!("downloading {url}"))?;
    let got = format!("{:x}", Sha256::digest(&bytes));
    if got != sha256 {
        bail!("{url} doesn't match its checksum, expected {sha256} got {got}");
    }
    Ok(bytes)
}

/// Written next to it and renamed over, so a half written file never looks
/// like a real one.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("no parent folder")?)?;
    let part = path.with_extension("part");
    fs::write(&part, bytes)?;
    fs::rename(&part, path)?;
    Ok(())
}
