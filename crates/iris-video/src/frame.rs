//! Owned YUV planes. The GPU converts them; this crate does not build RGB.

/// Which matrix the renderer should apply. Limited-range studio swing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    /// SD / BT.601.
    Bt601,
    /// HD / BT.709.
    Bt709,
}

/// One plane. `stride` is the byte distance between rows and may be larger
/// than the tightly packed width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    /// Width in samples. An NV12 chroma plane is half the luma width.
    pub width: u32,
    /// Height in samples.
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: usize,
    /// `stride * height` bytes.
    pub bytes: Vec<u8>,
}

/// A decoded picture plus the media time it should appear.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedVideoFrame {
    /// Seconds from the start of playback, including completed loops.
    pub pts: f64,
    /// Coded width in pixels.
    pub width: u32,
    /// Coded height in pixels.
    pub height: u32,
    /// Plane layout.
    pub picture: Picture,
}

/// YUV 4:2:0 as three planes, or NV12 as luma plus interleaved chroma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picture {
    /// Planar 4:2:0.
    Yuv420 {
        /// Luma.
        y: Plane,
        /// Cb, half resolution.
        u: Plane,
        /// Cr, half resolution.
        v: Plane,
        /// Color matrix.
        matrix: Matrix,
    },
    /// Semi-planar 4:2:0. `uv` samples are pairs of bytes.
    Nv12 {
        /// Luma.
        y: Plane,
        /// Interleaved CbCr.
        uv: Plane,
        /// Color matrix.
        matrix: Matrix,
    },
}
