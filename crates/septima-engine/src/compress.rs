use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    pub password: Option<String>,
    /// `-mhe=on` encrypted headers (7z only).
    pub encrypt_headers: bool,
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
            password: None,
            encrypt_headers: false,
        }
    }

    /// The `-m*` method arguments (without password, which `run_add` appends).
    fn method_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let method_key = if self.format == "zip" { "-mm=" } else { "-m0=" };

        if let Some(codec) = &self.codec {
            let name = if codec == "copy" { "Copy" } else { codec.as_str() };
            args.push(format!("{method_key}{name}"));
        }
        if let Some(level) = self.level {
            args.push(format!("-mx={level}"));
        }
        if let Some(dict) = &self.dictionary {
            args.push(format!("-md={dict}"));
        }
        if let Some(threads) = self.threads {
            args.push(format!("-mmt={threads}"));
        }
        if let Some(solid) = self.solid {
            args.push(format!("-ms={}", if solid { "on" } else { "off" }));
        }
        args
    }
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
}
