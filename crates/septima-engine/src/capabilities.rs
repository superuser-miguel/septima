//! What the bundled `7zz` can create — the source of truth for the create UI.
//!
//! Curated from `7zz i` on the pinned build (encode-capable codecs, creatable
//! formats) with the codec-specific `-mx` level ranges from the 7-Zip ZS docs.
//! The build is pinned, so this static model stays accurate; a future refinement
//! can parse `7zz i` at runtime.
//!
//! Encryption methods are the exception: they *are* probed at runtime
//! ([`encryption_methods`]), because Septima may run against a stock `7zz`
//! (host install, distro packaging) that lacks the bundled build's extensions.

/// A compression codec, with its `7zz` method id and `-mx` level range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codec {
    /// `7zz` method name used in `-m0=`/`-mm=`.
    pub id: &'static str,
    pub label: &'static str,
    pub level_min: u8,
    pub level_max: u8,
    pub default_level: u8,
}

impl Codec {
    /// Store (no compression) — level controls are irrelevant.
    pub fn is_store(&self) -> bool {
        self.id == "copy"
    }
}

/// An archive format that can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    /// `-t` value: `7z`, `zip`, `tar`.
    pub id: &'static str,
    pub label: &'static str,
    pub extension: &'static str,
    pub codecs: &'static [Codec],
    pub supports_encryption: bool,
    /// 7z-only encrypted headers (`-mhe`).
    pub supports_header_encryption: bool,
    pub supports_solid: bool,
}

impl Format {
    pub fn default_codec(&self) -> Option<&'static Codec> {
        self.codecs.first()
    }
}

// -mx ranges per codec. For LZMA/LZMA2/PPMd/BZip2/Deflate, -mx is the 1–9 preset;
// for the ZS plugin codecs it maps to the codec's native level (verified:
// `-m0=zstd -mx=19` -> "ZSTD:v1.5,l19"). Lizard's 10–49 is banded (family×level)
// — treated as a flat range for now; the two-part picker is a later refinement.
const LZMA2: Codec = Codec { id: "lzma2", label: "LZMA2", level_min: 1, level_max: 9, default_level: 5 };
const LZMA: Codec = Codec { id: "lzma", label: "LZMA", level_min: 1, level_max: 9, default_level: 5 };
const PPMD: Codec = Codec { id: "ppmd", label: "PPMd", level_min: 1, level_max: 9, default_level: 6 };
const BZIP2: Codec = Codec { id: "bzip2", label: "BZip2", level_min: 1, level_max: 9, default_level: 5 };
const DEFLATE: Codec = Codec { id: "deflate", label: "Deflate", level_min: 1, level_max: 9, default_level: 5 };
const DEFLATE64: Codec = Codec { id: "deflate64", label: "Deflate64", level_min: 1, level_max: 9, default_level: 5 };
const ZSTD: Codec = Codec { id: "zstd", label: "Zstandard", level_min: 1, level_max: 22, default_level: 3 };
const BROTLI: Codec = Codec { id: "brotli", label: "Brotli", level_min: 0, level_max: 11, default_level: 6 };
const LZ4: Codec = Codec { id: "lz4", label: "LZ4", level_min: 1, level_max: 12, default_level: 1 };
const LZ5: Codec = Codec { id: "lz5", label: "LZ5", level_min: 1, level_max: 15, default_level: 1 };
// Lizard's -mx encodes family (tens: 10/20/30/40) × sub-level (ones: 0-9). The
// UI picks the family separately, so the codec's own level is just the 0-9
// sub-level; the create dialog adds the family base back on.
const LIZARD: Codec = Codec { id: "lizard", label: "Lizard", level_min: 0, level_max: 9, default_level: 2 };
const FLZMA2: Codec = Codec { id: "flzma2", label: "Fast-LZMA2", level_min: 1, level_max: 9, default_level: 6 };
const COPY: Codec = Codec { id: "copy", label: "Store (no compression)", level_min: 0, level_max: 0, default_level: 0 };
// tar post-compressors: applied to the tar stream (tar → .tar.zst/.tar.xz/…).
const XZ: Codec = Codec { id: "xz", label: "xz", level_min: 0, level_max: 9, default_level: 6 };
const GZIP: Codec = Codec { id: "gzip", label: "gzip", level_min: 1, level_max: 9, default_level: 6 };

const SEVENZ_CODECS: &[Codec] =
    &[LZMA2, LZMA, PPMD, ZSTD, BROTLI, FLZMA2, BZIP2, LZ4, LZ5, LIZARD, DEFLATE, COPY];
// zip additionally accepts xz and deflate64 (verified); brotli/lz4/lz5/lizard
// are 7z-only and rejected inside zip.
const ZIP_CODECS: &[Codec] = &[DEFLATE, DEFLATE64, ZSTD, XZ, LZMA, PPMD, BZIP2, COPY];
// For tar, the "codec" chooses an optional post-compressor (tar → .tar.<ext>).
const TAR_CODECS: &[Codec] = &[COPY, ZSTD, XZ, GZIP, BZIP2, BROTLI, LZ4, LZ5, LIZARD];
// Raw single-file streams: the "codec" *is* the format (`-t<codec>`), one file,
// no container. Lizard is omitted (its family picker doesn't apply here).
const STREAM_CODECS: &[Codec] = &[ZSTD, XZ, GZIP, BZIP2, BROTLI, LZ4, LZ5];

const FORMATS: &[Format] = &[
    Format {
        id: "7z",
        label: "7z",
        extension: "7z",
        codecs: SEVENZ_CODECS,
        supports_encryption: true,
        supports_header_encryption: true,
        supports_solid: true,
    },
    Format {
        id: "zip",
        label: "Zip",
        extension: "zip",
        codecs: ZIP_CODECS,
        supports_encryption: true,
        supports_header_encryption: false,
        supports_solid: false,
    },
    Format {
        id: "tar",
        label: "Tar",
        extension: "tar",
        codecs: TAR_CODECS,
        supports_encryption: false,
        supports_header_encryption: false,
        supports_solid: false,
    },
    Format {
        id: "stream",
        label: "Single file (raw stream)",
        extension: "", // follows the chosen codec (zst/xz/gz/…)
        codecs: STREAM_CODECS,
        supports_encryption: false,
        supports_header_encryption: false,
        supports_solid: false,
    },
];

/// The single-file extension for a raw-stream codec (`zstd` -> `zst`, …).
pub fn stream_extension(codec_id: &str) -> &'static str {
    match codec_id {
        "zstd" => "zst",
        "xz" => "xz",
        "gzip" => "gz",
        "bzip2" => "bz2",
        "brotli" => "br",
        "lz4" => "lz4",
        "lz5" => "lz5",
        _ => "bin",
    }
}

/// Every creatable format, in menu order (7z first).
pub fn formats() -> &'static [Format] {
    FORMATS
}

/// An encryption method offered for a format (`-mem=<id>`).
///
/// `id: None` means "don't pass `-mem=` at all", i.e. the format's built-in
/// default (7z: AES-256-CBC; zip: the weak legacy ZipCrypto).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncryptionMethod {
    pub id: Option<&'static str>,
    pub label: &'static str,
    /// Longer explanation for the UI, where the trade-off isn't obvious.
    pub detail: Option<&'static str>,
}

const SEVENZ_AES_CBC: EncryptionMethod = EncryptionMethod {
    id: None,
    label: "AES-256 (standard)",
    detail: Some("Opens in any 7-Zip"),
};
// Only offered when the running 7zz reports the codec — see aes256gcm_available().
const SEVENZ_AES_GCM: EncryptionMethod = EncryptionMethod {
    id: Some("AES256GCM"),
    label: "AES-256-GCM + Argon2id",
    detail: Some("Stronger: tamper-detecting, resists password cracking. \
                  Septima extension — the archive opens only in Septima."),
};
const ZIP_AES: EncryptionMethod = EncryptionMethod {
    id: Some("AES256"),
    label: "AES-256",
    detail: None,
};
const ZIP_CRYPTO: EncryptionMethod = EncryptionMethod {
    id: None,
    label: "ZipCrypto (legacy)",
    detail: Some("Weak, but reads on old tools"),
};

/// Whether the `7zz` we will actually run supports the AES-256-GCM + Argon2id
/// method (the Septima patch series; absent from stock 7-Zip).
///
/// Probed once by parsing `7zz i`, then cached. Any failure to run the probe is
/// treated as "not supported", so the option simply doesn't appear.
pub fn aes256gcm_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new(crate::command::sevenzip_path())
            .arg("i")
            .stdin(std::process::Stdio::null())
            .output()
            .map(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .any(|line| line.split_whitespace().any(|w| w == "AES256GCM"))
            })
            .unwrap_or(false)
    })
}

/// Encryption methods offered for `format_id`, in menu order (default first).
///
/// Empty for formats without encryption. The 7z list gains the GCM entry only
/// when [`aes256gcm_available`] says the engine understands it.
pub fn encryption_methods(format_id: &str) -> Vec<EncryptionMethod> {
    match format_id {
        "7z" => {
            let mut v = vec![SEVENZ_AES_CBC];
            if aes256gcm_available() {
                v.push(SEVENZ_AES_GCM);
            }
            v
        }
        "zip" => vec![ZIP_AES, ZIP_CRYPTO],
        _ => Vec::new(),
    }
}

/// A pre-codec filter (`-m0=<id> -m1=<codec>`), for the create dialog's Filter
/// picker. 7z only. `id: ""` is the "None" entry (menu order, None first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filter {
    /// `7zz` method name, or `""` for no filter.
    pub id: &'static str,
    pub label: &'static str,
}

/// The filters offered in the UI, verified accepted by the pinned `7zz`. The
/// obscure ones (IA64, Swap2/Swap4) stay reachable via the advanced field.
pub fn filters() -> &'static [Filter] {
    &[
        Filter { id: "", label: "None" },
        Filter { id: "BCJ", label: "Executables — x86 (BCJ)" },
        Filter { id: "BCJ2", label: "Executables — x86 (BCJ2)" },
        Filter { id: "ARM64", label: "Executables — ARM64" },
        Filter { id: "ARM", label: "Executables — ARM" },
        Filter { id: "ARMT", label: "Executables — ARM Thumb" },
        Filter { id: "PPC", label: "Executables — PowerPC" },
        Filter { id: "SPARC", label: "Executables — SPARC" },
        Filter { id: "RISCV", label: "Executables — RISC-V" },
        Filter { id: "Delta", label: "Delta (audio, tables, bitmaps)" },
    ]
}
