//! Checked pixel-buffer sizes.

use crate::error::Error;
use crate::limits;

pub(crate) fn byte_len(width: u32, height: u32, bytes_per_pixel: usize) -> Result<usize, Error> {
    if width == 0 || height == 0 {
        return Err(Error::EmptyDimension { context: "tex mip" });
    }
    if width > limits::MAX_DIMENSION || height > limits::MAX_DIMENSION {
        return Err(Error::Limit {
            context: "tex mip dimension",
            value: u64::from(width.max(height)),
            limit: u64::from(limits::MAX_DIMENSION),
        });
    }
    let width = usize::try_from(width).map_err(|_| Error::Invariant("width"))?;
    let height = usize::try_from(height).map_err(|_| Error::Invariant("height"))?;
    let pixels = width
        .checked_mul(height)
        .ok_or(Error::Invariant("pixel count"))?;
    let bytes = pixels
        .checked_mul(bytes_per_pixel)
        .ok_or(Error::Invariant("pixel bytes"))?;
    if bytes > limits::MAX_MIP_BYTES {
        return Err(Error::Limit {
            context: "tex mip bytes",
            value: bytes as u64,
            limit: limits::MAX_MIP_BYTES as u64,
        });
    }
    Ok(bytes)
}
