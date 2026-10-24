//! Owned RGBA8 image ready for a GPU upload.

use crate::error::Error;
use crate::tex::RgbaImage;

/// Tightly packed RGBA8 pixels, row-major, origin at the top left.
pub struct DecodedTexture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for DecodedTexture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecodedTexture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

impl DecodedTexture {
    /// Check dimensions and the byte length, then keep the buffer.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, Error> {
        if width == 0 || height == 0 {
            return Err(Error::EmptyDimension {
                context: "decoded texture",
            });
        }
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or(Error::Limit {
                context: "decoded texture pixels",
                value: u64::from(width),
                limit: u64::MAX,
            })?;
        let expected = pixels.checked_mul(4).ok_or(Error::Limit {
            context: "decoded texture bytes",
            value: pixels,
            limit: u64::MAX,
        })?;
        let expected_len = usize::try_from(expected).map_err(|_| Error::Limit {
            context: "decoded texture bytes",
            value: expected,
            limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        })?;
        if rgba.len() != expected_len {
            return Err(Error::SizeMismatch {
                context: "decoded texture",
                expected: expected_len,
                actual: rgba.len(),
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Move a decoded mip into an upload buffer.
    pub fn from_rgba(image: RgbaImage) -> Result<Self, Error> {
        Self::new(image.width, image.height, image.pixels)
    }
}

impl TryFrom<RgbaImage> for DecodedTexture {
    type Error = Error;

    fn try_from(image: RgbaImage) -> Result<Self, Self::Error> {
        Self::from_rgba(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_empty_edge_and_a_short_buffer() {
        let empty = DecodedTexture::new(0, 1, Vec::new());
        assert!(matches!(
            empty,
            Err(Error::EmptyDimension {
                context: "decoded texture"
            })
        ));
        let short = DecodedTexture::new(1, 1, vec![1, 2, 3]);
        assert!(matches!(short, Err(Error::SizeMismatch { .. })));
    }

    #[test]
    fn keeps_a_tight_rgba_buffer() {
        let image = RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![9, 8, 7, 6],
        };
        let texture = DecodedTexture::from_rgba(image).expect("rgba");
        assert_eq!(texture.width, 1);
        assert_eq!(texture.rgba, vec![9, 8, 7, 6]);
    }
}
