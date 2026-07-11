//! What the bundled `7zz` can create — the source of truth for the create UI.
//!
//! Curated from `7zz i` on the pinned build (encode-capable codecs, creatable
//! formats) with the codec-specific `-mx` level ranges from the 7-Zip ZS docs.
//! The build is pinned, so this static model stays accurate; a future refinement
//! can parse `7zz i` at runtime.

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
const ZSTD: Codec = Codec { id: "zstd", label: "Zstandard", level_min: 1, level_max: 22, default_level: 3 };
const BROTLI: Codec = Codec { id: "brotli", label: "Brotli", level_min: 0, level_max: 11, default_level: 6 };
const LZ4: Codec = Codec { id: "lz4", label: "LZ4", level_min: 1, level_max: 12, default_level: 1 };
const LZ5: Codec = Codec { id: "lz5", label: "LZ5", level_min: 1, level_max: 15, default_level: 1 };
const LIZARD: Codec = Codec { id: "lizard", label: "Lizard", level_min: 10, level_max: 49, default_level: 10 };
const FLZMA2: Codec = Codec { id: "flzma2", label: "Fast-LZMA2", level_min: 1, level_max: 9, default_level: 6 };
const COPY: Codec = Codec { id: "copy", label: "Store (no compression)", level_min: 0, level_max: 0, default_level: 0 };
// tar post-compressors: applied to the tar stream (tar → .tar.zst/.tar.xz/…).
const XZ: Codec = Codec { id: "xz", label: "xz", level_min: 0, level_max: 9, default_level: 6 };
const GZIP: Codec = Codec { id: "gzip", label: "gzip", level_min: 1, level_max: 9, default_level: 6 };

const SEVENZ_CODECS: &[Codec] =
    &[LZMA2, LZMA, PPMD, ZSTD, BROTLI, FLZMA2, BZIP2, LZ4, LZ5, LIZARD, DEFLATE, COPY];
const ZIP_CODECS: &[Codec] = &[DEFLATE, ZSTD, BZIP2, LZMA, PPMD, COPY];
// For tar, the "codec" chooses an optional post-compressor (tar → .tar.<ext>).
const TAR_CODECS: &[Codec] = &[COPY, ZSTD, XZ, GZIP, BZIP2];

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
];

/// Every creatable format, in menu order (7z first).
pub fn formats() -> &'static [Format] {
    FORMATS
}
