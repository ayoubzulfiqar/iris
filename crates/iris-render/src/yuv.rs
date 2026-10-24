//! Limited-range YUV to RGB. The same coefficients are inlined in the fragment shader.

/// Studio-swing matrix selected from the stream's color space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvMatrix {
    /// BT.601.
    Bt601,
    /// BT.709.
    Bt709,
}

/// One plane borrowed for a single upload.
pub struct YuvPlane<'a> {
    /// Sample width. NV12 chroma is half the luma width.
    pub width: u32,
    /// Sample height.
    pub height: u32,
    /// Bytes between rows.
    pub stride: usize,
    /// Row storage. The length is at least `stride * height`.
    pub data: &'a [u8],
}

/// A picture the fragment shader can sample.
pub enum YuvFrame<'a> {
    /// Three-plane 4:2:0.
    Yuv420 {
        /// Luma.
        y: YuvPlane<'a>,
        /// Cb.
        u: YuvPlane<'a>,
        /// Cr.
        v: YuvPlane<'a>,
        /// Matrix.
        matrix: YuvMatrix,
    },
    /// Luma plus interleaved chroma.
    Nv12 {
        /// Luma.
        y: YuvPlane<'a>,
        /// CbCr pairs.
        uv: YuvPlane<'a>,
        /// Matrix.
        matrix: YuvMatrix,
    },
}

/// Uniform `padding.x` values. `0` is an RGBA texture.
pub const MODE_RGBA: f32 = 0.0;
/// Planar 4:2:0, BT.709.
pub const MODE_YUV420_709: f32 = 1.0;
/// NV12, BT.709.
pub const MODE_NV12_709: f32 = 2.0;
/// Planar 4:2:0, BT.601.
pub const MODE_YUV420_601: f32 = 3.0;
/// NV12, BT.601.
pub const MODE_NV12_601: f32 = 4.0;

/// Map a frame onto the uniform mode consumed by the shader.
pub fn sample_mode(frame: &YuvFrame<'_>) -> f32 {
    match frame {
        YuvFrame::Yuv420 {
            matrix: YuvMatrix::Bt709,
            ..
        } => MODE_YUV420_709,
        YuvFrame::Nv12 {
            matrix: YuvMatrix::Bt709,
            ..
        } => MODE_NV12_709,
        YuvFrame::Yuv420 {
            matrix: YuvMatrix::Bt601,
            ..
        } => MODE_YUV420_601,
        YuvFrame::Nv12 {
            matrix: YuvMatrix::Bt601,
            ..
        } => MODE_NV12_601,
    }
}

/// Limited-range (16–235) conversion. `y`, `u`, and `v` are normalized to `0.0..=1.0`.
pub fn limited_rgb(matrix: YuvMatrix, y: f32, u: f32, v: f32) -> [f32; 3] {
    let yy = 1.164_383 * (y - 16.0 / 255.0);
    let uu = u - 128.0 / 255.0;
    let vv = v - 128.0 / 255.0;
    let (r, g, b) = match matrix {
        YuvMatrix::Bt709 => (
            yy + 1.792_741 * vv,
            yy - 0.213_249 * uu - 0.532_909 * vv,
            yy + 2.112_402 * uu,
        ),
        YuvMatrix::Bt601 => (
            yy + 1.596_027 * vv,
            yy - 0.391_762 * uu - 0.812_968 * vv,
            yy + 2.017_232 * uu,
        ),
    };
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_black_and_white_stay_neutral() {
        for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709] {
            let black = limited_rgb(matrix, 16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
            assert!(black.iter().all(|channel| *channel < 0.02), "{black:?}");
            let white = limited_rgb(matrix, 235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
            assert!(
                white.iter().all(|channel| (channel - 1.0).abs() < 0.02),
                "{white:?}"
            );
        }
    }
}
