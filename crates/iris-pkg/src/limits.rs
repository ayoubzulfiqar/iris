//! Caps that keep a corrupt archive from forcing a huge allocation.

/// Longest length-prefixed or NUL-terminated string we will copy.
pub(crate) const MAX_STRING_LEN: usize = 4_096;

/// Most files one package may declare.
pub(crate) const MAX_ENTRY_COUNT: u32 = 100_000;

/// Most bytes one mip payload may occupy after LZ4 expansion.
pub(crate) const MAX_MIP_BYTES: usize = 256 * 1024 * 1024;

/// Largest width or height we will decode.
pub(crate) const MAX_DIMENSION: u32 = 16_384;

/// Most images (GIF frames, layers) in one texture.
pub(crate) const MAX_IMAGE_COUNT: u32 = 100;

/// Most mips in one image.
pub(crate) const MAX_MIPMAP_COUNT: u32 = 32;

/// Most animation frames in a `TEXS` block.
pub(crate) const MAX_FRAME_COUNT: u32 = 10_000;
