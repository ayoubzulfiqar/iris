//! S3TC / DXT block decode to straight RGBA8.
//!
//! Blocks are 4×4. Dimensions that are not multiples of 4 still occupy a full
//! block on disk; only texels inside the mip are written. Color endpoints are
//! RGB565 expanded the usual way (`(c << 3) | (c >> 2)` for 5-bit channels).
//! DXT1 uses the 3-color transparent codebook when `color0 <= color1`. DXT3 and
//! DXT5 always use the 4-color codebook and then replace alpha.

use crate::error::Error;
use crate::size;
use crate::tex::RgbaImage;

#[derive(Clone, Copy)]
pub(crate) enum BlockFormat {
    Dxt1,
    Dxt3,
    Dxt5,
}

pub(crate) fn decode(
    format: BlockFormat,
    width: u32,
    height: u32,
    data: &[u8],
) -> Result<RgbaImage, Error> {
    let stride = match format {
        BlockFormat::Dxt1 => 8usize,
        BlockFormat::Dxt3 | BlockFormat::Dxt5 => 16,
    };
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let block_count = usize::try_from(blocks_x)
        .ok()
        .and_then(|x| {
            usize::try_from(blocks_y)
                .ok()
                .and_then(|y| x.checked_mul(y))
        })
        .ok_or(Error::Invariant("dxt block count"))?;
    let needed = block_count
        .checked_mul(stride)
        .ok_or(Error::Invariant("dxt byte count"))?;
    if data.len() < needed {
        return Err(Error::SizeMismatch {
            context: "dxt blocks",
            expected: needed,
            actual: data.len(),
        });
    }
    let byte_len = size::byte_len(width, height, 4)?;
    let mut pixels = vec![0u8; byte_len];
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let index = usize::try_from(by)
                .ok()
                .and_then(|y| {
                    usize::try_from(blocks_x)
                        .ok()
                        .and_then(|row| y.checked_mul(row))
                })
                .and_then(|row| usize::try_from(bx).ok().and_then(|x| row.checked_add(x)))
                .ok_or(Error::Invariant("dxt block index"))?;
            let offset = index
                .checked_mul(stride)
                .ok_or(Error::Invariant("dxt block offset"))?;
            let end = offset.saturating_add(stride);
            let block = data
                .get(offset..end)
                .ok_or(Error::Invariant("dxt block slice"))?;
            let decoded = decode_block(format, block)?;
            blit(
                &mut pixels,
                width,
                height,
                bx.saturating_mul(4),
                by.saturating_mul(4),
                &decoded,
            )?;
        }
    }
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

fn decode_block(format: BlockFormat, block: &[u8]) -> Result<[[u8; 4]; 16], Error> {
    let (alpha, color, dxt1) = match format {
        BlockFormat::Dxt1 => (None, copy8(block)?, true),
        BlockFormat::Dxt3 | BlockFormat::Dxt5 => {
            if block.len() < 16 {
                return Err(Error::Invariant("dxt 16-byte block"));
            }
            let alpha = copy8(block.get(..8).ok_or(Error::Invariant("dxt alpha"))?)?;
            let color = copy8(block.get(8..16).ok_or(Error::Invariant("dxt color"))?)?;
            (Some(alpha), color, false)
        }
    };
    let mut texels = decode_color(&color, dxt1)?;
    if let Some(alpha) = alpha {
        match format {
            BlockFormat::Dxt3 => apply_dxt3_alpha(alpha, &mut texels)?,
            BlockFormat::Dxt5 => apply_dxt5_alpha(alpha, &mut texels)?,
            BlockFormat::Dxt1 => {}
        }
    }
    Ok(texels)
}

fn decode_color(block: &[u8; 8], dxt1: bool) -> Result<[[u8; 4]; 16], Error> {
    let color0 = u16::from_le_bytes([block[0], block[1]]);
    let color1 = u16::from_le_bytes([block[2], block[3]]);
    let mut palette = [rgb565(color0), rgb565(color1), [0; 4], [0; 4]];
    if !dxt1 || color0 > color1 {
        palette[2] = lerp(palette[0], palette[1], 1, 3);
        palette[3] = lerp(palette[0], palette[1], 2, 3);
    } else {
        palette[2] = lerp(palette[0], palette[1], 1, 2);
        palette[3] = [0, 0, 0, 0];
    }
    let mut out = [[0u8; 4]; 16];
    for row in 0..4 {
        let bits = block[4 + row];
        for col in 0..4 {
            let index = usize::from((bits >> (2 * col)) & 0b11);
            let texel = palette
                .get(index)
                .copied()
                .ok_or(Error::Invariant("dxt palette"))?;
            let dest = row * 4 + col;
            *out.get_mut(dest).ok_or(Error::Invariant("dxt texel"))? = texel;
        }
    }
    Ok(out)
}

fn apply_dxt3_alpha(block: [u8; 8], texels: &mut [[u8; 4]; 16]) -> Result<(), Error> {
    for (index, byte) in block.iter().copied().enumerate() {
        let lo = byte & 0x0f;
        let hi = byte >> 4;
        let left = index.saturating_mul(2);
        let right = left.saturating_add(1);
        texels.get_mut(left).ok_or(Error::Invariant("dxt3 left"))?[3] = expand_nibble(lo);
        texels
            .get_mut(right)
            .ok_or(Error::Invariant("dxt3 right"))?[3] = expand_nibble(hi);
    }
    Ok(())
}

fn apply_dxt5_alpha(block: [u8; 8], texels: &mut [[u8; 4]; 16]) -> Result<(), Error> {
    let alpha0 = block[0];
    let alpha1 = block[1];
    let mut codes = [0u8; 8];
    codes[0] = alpha0;
    codes[1] = alpha1;
    if alpha0 > alpha1 {
        for i in 1u16..7 {
            let mixed = ((7 - i) * u16::from(alpha0) + i * u16::from(alpha1)) / 7;
            let slot = usize::from(i + 1);
            *codes.get_mut(slot).ok_or(Error::Invariant("dxt5 code"))? =
                u8::try_from(mixed).unwrap_or(0);
        }
    } else {
        for i in 1u16..5 {
            let mixed = ((5 - i) * u16::from(alpha0) + i * u16::from(alpha1)) / 5;
            let slot = usize::from(i + 1);
            *codes.get_mut(slot).ok_or(Error::Invariant("dxt5 code"))? =
                u8::try_from(mixed).unwrap_or(0);
        }
        codes[6] = 0;
        codes[7] = 255;
    }
    let mut bits = 0u64;
    for (shift, byte) in block[2..8].iter().copied().enumerate() {
        bits |= u64::from(byte) << (8 * shift);
    }
    for index in 0..16 {
        let code = usize::try_from((bits >> (3 * index)) & 0x7).unwrap_or(0);
        let alpha = codes
            .get(code)
            .copied()
            .ok_or(Error::Invariant("dxt5 alpha"))?;
        texels
            .get_mut(index)
            .ok_or(Error::Invariant("dxt5 texel"))?[3] = alpha;
    }
    Ok(())
}

fn blit(
    pixels: &mut [u8],
    image_w: u32,
    image_h: u32,
    origin_x: u32,
    origin_y: u32,
    block: &[[u8; 4]; 16],
) -> Result<(), Error> {
    for row in 0..4u32 {
        for col in 0..4u32 {
            let x = origin_x.saturating_add(col);
            let y = origin_y.saturating_add(row);
            if x >= image_w || y >= image_h {
                continue;
            }
            let texel_index = usize::try_from(row * 4 + col).unwrap_or(0);
            let texel = block
                .get(texel_index)
                .ok_or(Error::Invariant("dxt blit texel"))?;
            let dest = (usize::try_from(y).unwrap_or(0) * usize::try_from(image_w).unwrap_or(0)
                + usize::try_from(x).unwrap_or(0))
                * 4;
            let slot = pixels
                .get_mut(dest..dest.saturating_add(4))
                .ok_or(Error::Invariant("dxt blit"))?;
            slot.copy_from_slice(texel);
        }
    }
    Ok(())
}

fn rgb565(packed: u16) -> [u8; 4] {
    let r5 = u8::try_from((packed >> 11) & 0x1f).unwrap_or(0);
    let g6 = u8::try_from((packed >> 5) & 0x3f).unwrap_or(0);
    let b5 = u8::try_from(packed & 0x1f).unwrap_or(0);
    [
        (r5 << 3) | (r5 >> 2),
        (g6 << 2) | (g6 >> 4),
        (b5 << 3) | (b5 >> 2),
        255,
    ]
}

fn lerp(a: [u8; 4], b: [u8; 4], num: u16, den: u16) -> [u8; 4] {
    let mix = |left: u8, right: u8| -> u8 {
        let value = (u16::from(left) * (den - num) + u16::from(right) * num) / den;
        u8::try_from(value).unwrap_or(0)
    };
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]), 255]
}

fn expand_nibble(nibble: u8) -> u8 {
    nibble | (nibble << 4)
}

fn copy8(bytes: &[u8]) -> Result<[u8; 8], Error> {
    let slice = bytes.get(..8).ok_or(Error::Invariant("8 byte block"))?;
    let mut out = [0u8; 8];
    out.copy_from_slice(slice);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxt1_four_color_row_interpolates_red_and_black() {
        // color0 = red, color1 = black, indices 0,1,2,3 on the first row.
        let block = [0x00, 0xF8, 0x00, 0x00, 0xE4, 0x00, 0x00, 0x00];
        let image = decode(BlockFormat::Dxt1, 4, 1, &block).expect("decode");
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([0, 0, 0, 255]));
        assert_eq!(image.pixel(2, 0), Some([170, 0, 0, 255]));
        assert_eq!(image.pixel(3, 0), Some([85, 0, 0, 255]));
        assert_eq!(image.pixels.len(), 16);
    }

    #[test]
    fn dxt1_three_color_mode_uses_transparent_black() {
        // color0 <= color1. Indices 0, 3, 2, 1.
        let block = [0x00, 0x00, 0x00, 0xF8, 0x6C, 0x00, 0x00, 0x00];
        let image = decode(BlockFormat::Dxt1, 4, 1, &block).expect("decode");
        assert_eq!(image.pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([0, 0, 0, 0]));
        assert_eq!(image.pixel(2, 0), Some([127, 0, 0, 255]));
        assert_eq!(image.pixel(3, 0), Some([255, 0, 0, 255]));
    }

    #[test]
    fn dxt1_one_by_one_drops_the_padding_texels() {
        let block = [0x00, 0xF8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let image = decode(BlockFormat::Dxt1, 1, 1, &block).expect("decode");
        assert_eq!(image.width, 1);
        assert_eq!(image.height, 1);
        assert_eq!(image.pixels, vec![255, 0, 0, 255]);
    }

    #[test]
    fn dxt3_nibbles_replace_alpha() {
        let mut block = [0u8; 16];
        block[0] = 0x0F;
        block[1..8].fill(0xFF);
        block[8..16].copy_from_slice(&[0x00, 0xF8, 0x00, 0x00, 0xE4, 0x00, 0x00, 0x00]);
        let image = decode(BlockFormat::Dxt3, 4, 1, &block).expect("decode");
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn dxt5_alpha_endpoints_are_index_0_and_1() {
        let mut block = [0u8; 16];
        block[0] = 255;
        block[1] = 0;
        block[2] = 0x08;
        block[8..16].copy_from_slice(&[0x00, 0xF8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let image = decode(BlockFormat::Dxt5, 4, 1, &block).expect("decode");
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([255, 0, 0, 0]));
    }

    #[test]
    fn a_short_block_is_a_size_mismatch() {
        let err = decode(BlockFormat::Dxt1, 4, 4, &[0; 7]).expect_err("short");
        assert!(matches!(err, Error::SizeMismatch { .. }));
    }
}
