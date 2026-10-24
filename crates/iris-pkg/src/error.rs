//! Parse errors for packages, textures, and scene JSON.
//!
//! Every failure is returned. The parsers do not panic on a short or corrupt input.

use std::path::PathBuf;

/// Failure while reading a package, texture, or scene document.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Filesystem read failed. The in-memory parsers do not produce this.
    #[error("failed to read {path}: {source}")]
    Io {
        /// Path passed to [`crate::Package::open`].
        path: PathBuf,
        /// OS error.
        #[source]
        source: std::io::Error,
    },

    /// The cursor needed more bytes than the buffer still held.
    #[error("{context}: need {expected} more bytes, {available} remain")]
    UnexpectedEof {
        /// Which field was being read.
        context: &'static str,
        /// Bytes the field required.
        expected: usize,
        /// Bytes left in the buffer.
        available: usize,
    },

    /// A magic string was not one this parser accepts.
    #[error("{context}: unexpected magic `{found}`")]
    BadMagic {
        /// Which magic was expected.
        context: &'static str,
        /// Bytes interpreted as text.
        found: String,
    },

    /// A length-prefixed or NUL-terminated string was not UTF-8.
    #[error("{context}: text is not utf-8")]
    InvalidUtf8 {
        /// Which string was being read.
        context: &'static str,
    },

    /// A count, dimension, or byte length was above the safety cap.
    #[error("{context}: value {value} exceeds limit {limit}")]
    Limit {
        /// Which field was too large.
        context: &'static str,
        /// Value from the file.
        value: u64,
        /// Cap enforced by this crate.
        limit: u64,
    },

    /// A signed field that must be `>= 0` was negative.
    #[error("{context}: negative value {value}")]
    Negative {
        /// Which field was negative.
        context: &'static str,
        /// Raw value.
        value: i32,
    },

    /// An entry's offset and length do not fit in the archive.
    #[error(
        "entry `{path}` points outside the package (start {start}, length {length}, file {file_len})"
    )]
    BadRange {
        /// Entry path, after normalization.
        path: String,
        /// Absolute start byte.
        start: u64,
        /// Declared length.
        length: u64,
        /// Archive length.
        file_len: u64,
    },

    /// An entry path is absolute, empty, or contains `..`.
    #[error("package path `{0}` is empty or escapes the archive")]
    UnsafePath(String),

    /// Pixel or block bytes do not match the declared dimensions.
    #[error("{context}: expected at least {expected} bytes, got {actual}")]
    SizeMismatch {
        /// Which buffer was short.
        context: &'static str,
        /// Minimum size for the declared dimensions.
        expected: usize,
        /// Bytes actually present.
        actual: usize,
    },

    /// LZ4 rejected a mip block.
    #[error("lz4 decompress failed while reading {context}: {detail}")]
    Lz4 {
        /// Which mip was being decompressed.
        context: &'static str,
        /// Compressor detail.
        detail: String,
    },

    /// The compression flag was neither raw (`0`) nor LZ4 (`1`).
    #[error("{context}: unknown compression flag {flag}")]
    BadCompression {
        /// Which mip declared the flag.
        context: &'static str,
        /// Raw flag.
        flag: u32,
    },

    /// `image` / `mip` is outside the texture.
    #[error("texture has no image {image} mip {mip}")]
    MissingMip {
        /// Image index.
        image: usize,
        /// Mip index inside that image.
        mip: usize,
    },

    /// The format id is known, but this crate does not expand it to RGBA8.
    ///
    /// The mip's raw bytes are still available from [`crate::Texture::mip`].
    #[error("pixel format `{format}` is not expanded to rgba8")]
    UnsupportedPixels {
        /// Short format name.
        format: String,
    },

    /// A mip width or height was zero at decode time.
    #[error("{context}: dimension is zero")]
    EmptyDimension {
        /// Which dimension was zero.
        context: &'static str,
    },

    /// A crop was larger than the decoded mip.
    #[error("mip {width}x{height} cannot be cropped to {crop_width}x{crop_height}")]
    Crop {
        /// Decoded width.
        width: u32,
        /// Decoded height.
        height: u32,
        /// Requested width.
        crop_width: u32,
        /// Requested height.
        crop_height: u32,
    },

    /// A scene vector was not a finite 1-, 2-, or 3-component value.
    #[error("{context}: {detail}")]
    BadVector {
        /// Which vector was being read.
        context: &'static str,
        /// Why it was rejected.
        detail: String,
    },

    /// Scene or project JSON did not match the schema.
    #[error("{context}: {source}")]
    Json {
        /// `scene.json` or `project.json`.
        context: &'static str,
        /// Serde detail.
        #[source]
        source: serde_json::Error,
    },

    /// A check that only fails if this crate's own arithmetic is wrong.
    #[error("internal invariant failed: {0}")]
    Invariant(&'static str),
}
