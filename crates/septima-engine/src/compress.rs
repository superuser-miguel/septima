use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::EngineError;
use crate::extract::CancelToken;
use crate::progress::ExtractProgress;
use crate::supervise::supervise;

/// A request to create/add to an archive with `7zz a`.
#[derive(Debug, Clone)]
pub struct CompressionRequest {
    pub output: PathBuf,
    pub inputs: Vec<PathBuf>,
    /// Format id (`-t`): `7z`, `zip`, `tar`.
    pub format: String,
    /// Codec method id (`7z` uses `-m0=`, `zip` uses `-mm=`); `None`/`copy` = store.
    pub codec: Option<String>,
    /// `-mx` level.
    pub level: Option<u8>,
    /// `-mmt` thread count.
    pub threads: Option<u32>,
    /// `-md` dictionary size, e.g. `"64m"`.
    pub dictionary: Option<String>,
    /// `-ms` solid mode (7z).
    pub solid: Option<bool>,
    /// Split into volumes of this size, e.g. `"100m"` (`-v`). `None` = single file.
    pub volume_size: Option<String>,
    /// Prepend a BCJ executable filter (`-m0=BCJ -m1=<codec>`) — 7z only.
    pub bcj: bool,
    pub password: Option<String>,
    /// `-mhe=on` encrypted headers (7z only).
    pub encrypt_headers: bool,
    /// Free-text extra `-m*`/other switches (power-user escape hatch).
    pub extra_params: Vec<String>,
}

impl CompressionRequest {
    pub fn new(output: impl Into<PathBuf>, inputs: Vec<PathBuf>, format: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            inputs,
            format: format.into(),
            codec: None,
            level: None,
            threads: None,
            dictionary: None,
            solid: None,
            volume_size: None,
            bcj: false,
            password: None,
            encrypt_headers: false,
            extra_params: Vec::new(),
        }
    }

    /// The `-m*` method arguments (without password, which `run_add` appends).
    fn method_args(&self) -> Vec<String> {
        // tar is an uncompressed container: it takes no -m* method options
        // (`-m0=Copy` makes 7zz fail with a "cannot open the file as archive").
        if self.format == "tar" {
            return Vec::new();
        }

        let mut args = Vec::new();
        let method_key = if self.format == "zip" { "-mm=" } else { "-m0=" };

        let mut bcj_active = false;
        if let Some(codec) = &self.codec {
            let name = if codec == "copy" { "Copy" } else { codec.as_str() };
            // BCJ executable filter goes ahead of the codec in the 7z chain,
            // making the codec method 1.
            if self.bcj && self.format == "7z" && codec != "copy" {
                bcj_active = true;
                args.push("-m0=BCJ".to_string());
                args.push(format!("-m1={name}"));
            } else {
                args.push(format!("{method_key}{name}"));
            }
        }
        if let Some(level) = self.level {
            args.push(format!("-mx={level}"));
        }
        if let Some(dict) = &self.dictionary {
            // Dict targets the codec method: `-m1d` inside a BCJ chain, else `-md`.
            let key = if bcj_active { "-m1d=" } else { "-md=" };
            args.push(format!("{key}{dict}"));
        }
        if let Some(threads) = self.threads {
            args.push(format!("-mmt={threads}"));
        }
        if let Some(solid) = self.solid {
            args.push(format!("-ms={}", if solid { "on" } else { "off" }));
        }
        args.extend(self.extra_params.iter().cloned());
        args
    }
}

/// Create a `tar`, then compress it into `output` with `compressor`
/// (`zstd`/`xz`/`gzip`/`bzip2`) — producing a real `.tar.<ext>`.
///
/// Two steps via a temp tar (7zz can't tar+compress multiple files in one shot).
/// The temp lives in the system temp dir (writable under Flatpak); the compress
/// phase reads a real file, so it reports accurate progress.
///
/// Uses `req.inputs`/`req.output`, `req.codec` as the compressor, and
/// `req.level`/`req.threads`.
pub fn run_tar_and_compress(
    sevenzip: &Path,
    req: &CompressionRequest,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&ExtractProgress),
) -> Result<(), EngineError> {
    let compressor = req.codec.as_deref().unwrap_or("zstd");
    let output = req.output.as_path();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Unique temp dir gives isolation; the tar keeps a clean name (it becomes the
    // inner entry name — visible for gzip and when browsing the tar later).
    let temp_dir = std::env::temp_dir().join(format!("septima-{}-{nanos}", std::process::id()));
    if let Err(err) = std::fs::create_dir_all(&temp_dir) {
        return Err(EngineError::Spawn(err));
    }
    let temp_tar = temp_dir.join(inner_tar_name(output));
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&temp_dir);
    };

    // Phase 1: build the (uncompressed) tar.
    let tar_req = CompressionRequest::new(temp_tar.clone(), req.inputs.clone(), "tar");
    if let Err(err) = run_add(sevenzip, &tar_req, cancel, &mut on_progress) {
        cleanup();
        return Err(err);
    }
    if cancel.load(Ordering::Relaxed) {
        cleanup();
        return Err(EngineError::Cancelled);
    }

    // Phase 2: compress the tar into the final output.
    let mut comp_req = CompressionRequest::new(output.to_path_buf(), vec![temp_tar], compressor);
    comp_req.level = req.level;
    comp_req.threads = req.threads;
    comp_req.volume_size = req.volume_size.clone();
    let result = run_add(sevenzip, &comp_req, cancel, on_progress);

    cleanup();
    result
}

/// The inner tar's file name: the output name minus its compressor extension,
/// ensured to end in `.tar` (e.g. `photos.tar.zst` -> `photos.tar`).
fn inner_tar_name(output: &Path) -> String {
    let name = output.file_name().and_then(|n| n.to_str()).unwrap_or("archive");
    let base = name
        .trim_end_matches(".zst")
        .trim_end_matches(".xz")
        .trim_end_matches(".gz")
        .trim_end_matches(".bz2")
        .trim_end_matches(".bzip2");
    if base.ends_with(".tar") {
        base.to_string()
    } else {
        format!("{base}.tar")
    }
}

/// Rough compression-memory estimate in bytes, for codecs where it's reliable
/// (the LZMA family — the dominant case, and where memory actually blows up).
///
/// bt4 compression needs ~10.5× the dictionary per block, roughly duplicated per
/// thread; returns `None` for codecs we won't guess at (honest > misleading).
pub fn estimate_add_memory(
    codec: &str,
    level: Option<u8>,
    dict_bytes: Option<u64>,
    threads: u32,
) -> Option<u64> {
    let dict = match codec {
        "lzma2" | "lzma" | "flzma2" => {
            dict_bytes.unwrap_or_else(|| default_lzma_dict(level.unwrap_or(5)))
        }
        _ => return None,
    };
    let threads = threads.max(1) as u64;
    // ~10.5x dict per block + ~16 MiB working overhead, per thread.
    Some(threads * (dict * 21 / 2 + 16 * 1024 * 1024))
}

/// Default LZMA2 dictionary for a `-mx` level (approx. 7-Zip presets).
fn default_lzma_dict(level: u8) -> u64 {
    let mib: u64 = match level {
        0..=2 => 1,
        3..=4 => 4,
        5..=6 => 16,
        7..=8 => 32,
        _ => 64,
    };
    mib * 1024 * 1024
}

/// Create/add to an archive, streaming progress. Blocking — run on a thread.
pub fn run_add(
    sevenzip: &Path,
    req: &CompressionRequest,
    cancel: &CancelToken,
    on_progress: impl FnMut(&ExtractProgress),
) -> Result<(), EngineError> {
    let mut cmd = Command::new(sevenzip);
    cmd.arg("a")
        .arg("-bsp1")
        .arg("-bb1")
        .arg("-y")
        .arg(format!("-t{}", req.format));

    for arg in req.method_args() {
        cmd.arg(arg);
    }
    if let Some(password) = &req.password {
        cmd.arg(format!("-p{password}"));
        if req.encrypt_headers && req.format == "7z" {
            cmd.arg("-mhe=on");
        }
    }
    if let Some(volume) = &req.volume_size {
        cmd.arg(format!("-v{volume}"));
    }

    cmd.arg("--").arg(&req.output);
    for input in &req.inputs {
        cmd.arg(input);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(EngineError::Spawn)?;
    supervise(child, cancel, on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_zip_method_args() {
        let mut req = CompressionRequest::new("out.7z", vec![PathBuf::from("a")], "7z");
        req.codec = Some("zstd".into());
        req.level = Some(19);
        req.threads = Some(4);
        assert_eq!(req.method_args(), ["-m0=zstd", "-mx=19", "-mmt=4"]);
    }

    #[test]
    fn zip_uses_mm_key() {
        let mut req = CompressionRequest::new("out.zip", vec![], "zip");
        req.codec = Some("zstd".into());
        assert_eq!(req.method_args(), ["-mm=zstd"]);
    }

    #[test]
    fn store_maps_to_copy() {
        let mut req = CompressionRequest::new("out.7z", vec![], "7z");
        req.codec = Some("copy".into());
        assert_eq!(req.method_args(), ["-m0=Copy"]);
    }

    #[test]
    fn tar_emits_no_method_args() {
        let mut req = CompressionRequest::new("out.tar", vec![], "tar");
        req.codec = Some("copy".into());
        req.threads = Some(4);
        assert!(req.method_args().is_empty());
    }

    #[test]
    fn bcj_chains_before_codec() {
        let mut req = CompressionRequest::new("out.7z", vec![], "7z");
        req.codec = Some("lzma2".into());
        req.bcj = true;
        req.level = Some(9);
        assert_eq!(req.method_args(), ["-m0=BCJ", "-m1=lzma2", "-mx=9"]);
    }

    #[test]
    fn bcj_dictionary_targets_method_one() {
        let mut req = CompressionRequest::new("out.7z", vec![], "7z");
        req.codec = Some("lzma2".into());
        req.bcj = true;
        req.dictionary = Some("16m".into());
        assert_eq!(req.method_args(), ["-m0=BCJ", "-m1=lzma2", "-m1d=16m"]);
    }

    #[test]
    fn extra_params_are_appended() {
        let mut req = CompressionRequest::new("out.7z", vec![], "7z");
        req.codec = Some("lzma2".into());
        req.extra_params = vec!["-myx=on".into()];
        assert_eq!(req.method_args(), ["-m0=lzma2", "-myx=on"]);
    }

    #[test]
    fn memory_estimate_only_for_lzma_family() {
        assert!(estimate_add_memory("lzma2", Some(9), None, 4).is_some());
        assert!(estimate_add_memory("zstd", Some(19), None, 4).is_none());
        // explicit 64 MiB dict, single thread -> ~10.5x + overhead
        let est = estimate_add_memory("lzma2", Some(9), Some(64 * 1024 * 1024), 1).unwrap();
        assert!(est > 600 * 1024 * 1024 && est < 720 * 1024 * 1024);
    }
}
