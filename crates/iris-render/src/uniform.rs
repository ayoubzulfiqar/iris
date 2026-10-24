//! 112-byte wallpaper uniform. Field offsets match the WGSL struct because the
//! prefix sums land on each member's alignment, so no extra padding bytes exist.
//!
//! The bytes are written with [`f32::to_le_bytes`]. A `bytemuck` derive would
//! emit `unsafe impl` in this crate, which `forbid(unsafe_code)` rejects.

use crate::error::Error;

/// Byte size of [`Uniforms`]. Also the uniform buffer allocation.
pub const UNIFORM_SIZE: usize = 112;

/// How the image meets the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Cover the output, cropping one axis.
    Fill,
    /// Fit inside the output, letterboxing the other axis.
    Fit,
    /// Stretch both axes to the output.
    Stretch,
    /// Draw at native pixel size, centered.
    Center,
}

/// CPU copy of the uniform block.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uniforms {
    /// Seconds since the wallpaper started. WGSL name `g_Time` is a let of this.
    pub time: f32,
    /// Seconds since the previous presented frame, after the playback rate.
    pub delta_time: f32,
    /// Pointer in the output, in pixels. Wallpaper Engine's `g_PointerPosition`.
    pub mouse: [f32; 2],
    /// Swapchain size in physical pixels.
    pub resolution: [f32; 2],
    /// `x` selects the sampler: 0 is RGBA, 1–4 are YUV matrices. `y` stays 0.
    pub padding: [f32; 2],
    /// Column-major 4x4. WGSL field `g_ModelViewProjectionMatrix`.
    pub mvp: [[f32; 4]; 4],
    /// Multiplied with the corner UV.
    pub uv_scale: [f32; 2],
    /// Added after [`Self::uv_scale`].
    pub uv_offset: [f32; 2],
}

/// Matrix and UV transform for one fit mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Column-major model-view-projection.
    pub mvp: [[f32; 4]; 4],
    /// UV scale applied to the unstretched corner.
    pub uv_scale: [f32; 2],
    /// UV offset applied after the scale.
    pub uv_offset: [f32; 2],
}

impl Uniforms {
    /// Pack the block in little-endian order.
    pub fn to_bytes(&self) -> Result<[u8; UNIFORM_SIZE], Error> {
        let mut bytes = [0u8; UNIFORM_SIZE];
        put_f32(&mut bytes, std::mem::offset_of!(Uniforms, time), self.time)?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, delta_time),
            self.delta_time,
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, mouse),
            self.mouse[0],
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, mouse) + 4,
            self.mouse[1],
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, resolution),
            self.resolution[0],
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, resolution) + 4,
            self.resolution[1],
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, padding),
            self.padding[0],
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, padding) + 4,
            self.padding[1],
        )?;
        let mvp_base = std::mem::offset_of!(Uniforms, mvp);
        for (column_index, column) in self.mvp.iter().enumerate() {
            for (row_index, value) in column.iter().enumerate() {
                let offset = mvp_base + (column_index * 4 + row_index) * 4;
                put_f32(&mut bytes, offset, *value)?;
            }
        }
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, uv_scale),
            self.uv_scale[0],
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, uv_scale) + 4,
            self.uv_scale[1],
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, uv_offset),
            self.uv_offset[0],
        )?;
        put_f32(
            &mut bytes,
            std::mem::offset_of!(Uniforms, uv_offset) + 4,
            self.uv_offset[1],
        )?;
        Ok(bytes)
    }
}

fn put_f32(bytes: &mut [u8], offset: usize, value: f32) -> Result<(), Error> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| Error::Gpu("uniform offset overflow".to_owned()))?;
    let slot = bytes.get_mut(offset..end).ok_or_else(|| {
        Error::Gpu(format!(
            "uniform write at {offset} does not fit in {UNIFORM_SIZE} bytes"
        ))
    })?;
    slot.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Identity matrix, column-major.
pub fn identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn scale_matrix(x: f32, y: f32) -> [[f32; 4]; 4] {
    [
        [x, 0.0, 0.0, 0.0],
        [0.0, y, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn edge(value: u32) -> f32 {
    if value == 0 {
        1.0
    } else {
        value as f32
    }
}

/// Placement for `fit` given the image and the swapchain size.
///
/// A zero edge is treated as 1 so the ratios stay finite.
pub fn placement(fit: Fit, image_w: u32, image_h: u32, out_w: u32, out_h: u32) -> Placement {
    let image_w = edge(image_w);
    let image_h = edge(image_h);
    let out_w = edge(out_w);
    let out_h = edge(out_h);
    let image_aspect = image_w / image_h;
    let output_aspect = out_w / out_h;
    match fit {
        Fit::Stretch => Placement {
            mvp: identity(),
            uv_scale: [1.0, 1.0],
            uv_offset: [0.0, 0.0],
        },
        Fit::Fill => {
            let (uv_scale, uv_offset) = if image_aspect > output_aspect {
                let scale_x = output_aspect / image_aspect;
                ([scale_x, 1.0], [(1.0 - scale_x) * 0.5, 0.0])
            } else {
                let scale_y = image_aspect / output_aspect;
                ([1.0, scale_y], [0.0, (1.0 - scale_y) * 0.5])
            };
            Placement {
                mvp: identity(),
                uv_scale,
                uv_offset,
            }
        }
        Fit::Fit => {
            let (scale_x, scale_y) = if image_aspect > output_aspect {
                (1.0, output_aspect / image_aspect)
            } else {
                (image_aspect / output_aspect, 1.0)
            };
            Placement {
                mvp: scale_matrix(scale_x, scale_y),
                uv_scale: [1.0, 1.0],
                uv_offset: [0.0, 0.0],
            }
        }
        Fit::Center => Placement {
            mvp: scale_matrix(image_w / out_w, image_h / out_h),
            uv_scale: [1.0, 1.0],
            uv_offset: [0.0, 0.0],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layout_matches_the_wgsl_block() {
        assert_eq!(std::mem::size_of::<Uniforms>(), UNIFORM_SIZE);
        assert_eq!(std::mem::offset_of!(Uniforms, mouse), 8);
        assert_eq!(std::mem::offset_of!(Uniforms, resolution), 16);
        assert_eq!(std::mem::offset_of!(Uniforms, padding), 24);
        assert_eq!(std::mem::offset_of!(Uniforms, mvp), 32);
        assert_eq!(std::mem::offset_of!(Uniforms, uv_scale), 96);
        assert_eq!(std::mem::offset_of!(Uniforms, uv_offset), 104);
    }

    #[test]
    fn time_is_the_first_four_bytes() {
        let uniforms = Uniforms {
            time: 1.0,
            delta_time: 0.0,
            mouse: [2.0, 3.0],
            resolution: [0.0, 0.0],
            padding: [0.0, 0.0],
            mvp: identity(),
            uv_scale: [1.0, 1.0],
            uv_offset: [0.0, 0.0],
        };
        let bytes = uniforms.to_bytes().expect("bytes");
        assert_eq!(&bytes[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[8..12], &2.0f32.to_le_bytes());
        assert_eq!(&bytes[12..16], &3.0f32.to_le_bytes());
    }

    #[test]
    fn fill_fit_and_center_use_exact_halves() {
        let fill = placement(Fit::Fill, 200, 100, 100, 100);
        assert_eq!(fill.uv_scale, [0.5, 1.0]);
        assert_eq!(fill.uv_offset, [0.25, 0.0]);
        assert_eq!(fill.mvp[0][0], 1.0);
        assert_eq!(fill.mvp[1][1], 1.0);

        let fitted = placement(Fit::Fit, 200, 100, 100, 100);
        assert_eq!(fitted.mvp[0][0], 1.0);
        assert_eq!(fitted.mvp[1][1], 0.5);
        assert_eq!(fitted.uv_scale, [1.0, 1.0]);
        assert_eq!(fitted.uv_offset, [0.0, 0.0]);

        let center = placement(Fit::Center, 50, 40, 100, 80);
        assert_eq!(center.mvp[0][0], 0.5);
        assert_eq!(center.mvp[1][1], 0.5);

        let stretch = placement(Fit::Stretch, 20, 10, 30, 40);
        assert_eq!(stretch.mvp, identity());
        assert_eq!(stretch.uv_scale, [1.0, 1.0]);
        assert_eq!(stretch.uv_offset, [0.0, 0.0]);
    }
}
