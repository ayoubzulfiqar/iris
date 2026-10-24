//! Wallpaper Engine `TEXV0005` textures.
//!
//! Layout, after the two NUL-terminated magics `TEXV0005` and `TEXI0001`:
//!
//! * `u32` format, `u32` flags, full width, full height, image width, image height, `u32` unknown
//! * NUL-terminated container magic `TEXB0001` .. `TEXB0004`
//! * `u32` image count
//! * `TEXB0003` then reads a signed FreeImage format (`-1` means raw pixels)
//! * `TEXB0004` reads that format plus an `i32` video flag. The mip header stays
//!   the version-4 form only for MP4; every other payload uses the version-3 header
//! * each image is `u32` mip count, then mip headers
//!
//! Mip bytes are LZ4 block data when the compression flag is `1`. [`Texture::decode_mip`]
//! expands the formats a GPU upload needs as RGBA8 and leaves PNG/JPEG/MP4 payloads
//! as the embedded file bytes.
//!
//! Format id `0` is stored in memory as R, G, B, A. Older notes call it ARGB8888;
//! the engine uploads it with an RGBA layout. Flag bit 19
//! ([`TexFlags::ALPHA_CHANNEL_PRIORITY`]) means an R8 texel is opacity and an RG88
//! texel is `(color, opacity)`.

use crate::cursor::Cursor;
use crate::dxt::{self, BlockFormat};
use crate::error::Error;
use crate::limits;
use crate::size;

const MAGIC_LIMIT: usize = 16;
const FREEIMAGE_UNKNOWN: i32 = -1;
const FREEIMAGE_MP4: i32 = 35;

/// Pixel format stored in the texture header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    /// Four bytes per pixel, R, G, B, A. Format id 0.
    Rgba8888,
    /// Three bytes per pixel, R, G, B. Format id 1.
    Rgb888,
    /// Little-endian RGB565. Format id 2.
    Rgb565,
    /// BC3. Format id 4.
    Dxt5,
    /// BC2. Format id 6.
    Dxt3,
    /// BC1. Format id 7.
    Dxt1,
    /// Two bytes per pixel, R then G. Format id 8.
    Rg88,
    /// One byte per pixel. Format id 9.
    R8,
    /// Two half-precision channels. Format id 10. Parsed, not expanded.
    Rg1616f,
    /// One half-precision channel. Format id 11. Parsed, not expanded.
    R16f,
    /// BC7. Format id 12. Parsed, not expanded.
    Bc7,
    /// Packed 10-bit color. Format id 13. Parsed, not expanded.
    Rgba1010102,
    /// Four half-precision channels. Format id 14. Parsed, not expanded.
    Rgba16161616f,
    /// Three half-precision channels. Format id 15. Parsed, not expanded.
    Rgb161616f,
    /// A format id this crate does not name.
    Unknown(u32),
}

impl TexFormat {
    /// Map a header format id onto a known layout.
    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Rgba8888,
            1 => Self::Rgb888,
            2 => Self::Rgb565,
            4 => Self::Dxt5,
            6 => Self::Dxt3,
            7 => Self::Dxt1,
            8 => Self::Rg88,
            9 => Self::R8,
            10 => Self::Rg1616f,
            11 => Self::R16f,
            12 => Self::Bc7,
            13 => Self::Rgba1010102,
            14 => Self::Rgba16161616f,
            15 => Self::Rgb161616f,
            other => Self::Unknown(other),
        }
    }

    /// Stable short name used in errors.
    pub fn label(self) -> String {
        match self {
            Self::Rgba8888 => "rgba8888".to_owned(),
            Self::Rgb888 => "rgb888".to_owned(),
            Self::Rgb565 => "rgb565".to_owned(),
            Self::Dxt5 => "dxt5".to_owned(),
            Self::Dxt3 => "dxt3".to_owned(),
            Self::Dxt1 => "dxt1".to_owned(),
            Self::Rg88 => "rg88".to_owned(),
            Self::R8 => "r8".to_owned(),
            Self::Rg1616f => "rg1616f".to_owned(),
            Self::R16f => "r16f".to_owned(),
            Self::Bc7 => "bc7".to_owned(),
            Self::Rgba1010102 => "rgba1010102".to_owned(),
            Self::Rgba16161616f => "rgba16161616f".to_owned(),
            Self::Rgb161616f => "rgb161616f".to_owned(),
            Self::Unknown(id) => format!("unknown({id})"),
        }
    }
}

/// Bitfield stored next to the format id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexFlags(pub u32);

impl TexFlags {
    /// Sampling should not interpolate.
    pub const NO_INTERPOLATION: u32 = 1;
    /// Clamp texture coordinates.
    pub const CLAMP_UVS: u32 = 2;
    /// A `TEXS` frame table follows the images.
    pub const IS_GIF: u32 = 4;
    /// Clamp to the border color.
    pub const CLAMP_UVS_BORDER: u32 = 8;
    /// The mip payload is a video elementary stream.
    pub const VIDEO: u32 = 32;
    /// R8 stores opacity. RG88 stores color in R and opacity in G.
    pub const ALPHA_CHANNEL_PRIORITY: u32 = 524_288;

    /// Whether `bit` is set.
    pub const fn contains(self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    /// [`Self::IS_GIF`].
    pub const fn is_gif(self) -> bool {
        self.contains(Self::IS_GIF)
    }

    /// [`Self::VIDEO`].
    pub const fn is_video(self) -> bool {
        self.contains(Self::VIDEO)
    }

    /// [`Self::ALPHA_CHANNEL_PRIORITY`].
    pub const fn alpha_priority(self) -> bool {
        self.contains(Self::ALPHA_CHANNEL_PRIORITY)
    }
}

/// Container revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerVersion {
    /// No compression flag on mips.
    TexB0001,
    /// Compression flag, no FreeImage type.
    TexB0002,
    /// Compression flag plus a FreeImage type.
    TexB0003,
    /// FreeImage type plus a video flag. Mip layout depends on the payload.
    TexB0004,
}

/// How each mip header is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MipLayout {
    /// Width, height, byte count, bytes.
    V1,
    /// Width, height, compression, uncompressed size, byte count, bytes.
    V2,
    /// Editor integers and a JSON string, then a [`MipLayout::V2`] header.
    V4,
}

/// FreeImage type, for the values this crate branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// Header value `-1`: pixels are described by [`TexFormat`].
    Unknown,
    /// FreeImage BMP.
    Bmp,
    /// FreeImage JPEG.
    Jpeg,
    /// FreeImage PNG.
    Png,
    /// FreeImage GIF.
    Gif,
    /// FreeImage TIFF.
    Tiff,
    /// MP4 payload. Value 35.
    Mp4,
    /// Some other FreeImage id. The bytes are returned unchanged.
    Other(i32),
}

impl ImageKind {
    /// Classify a FreeImage id.
    pub fn from_freeimage(id: i32) -> Self {
        match id {
            FREEIMAGE_UNKNOWN => Self::Unknown,
            0 => Self::Bmp,
            2 => Self::Jpeg,
            13 => Self::Png,
            18 => Self::Tiff,
            25 => Self::Gif,
            FREEIMAGE_MP4 => Self::Mp4,
            other => Self::Other(other),
        }
    }

    /// True when the mip bytes are a file, not raw pixels.
    pub fn is_embedded_file(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// `TEXS` revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameVersion {
    /// Frame rectangles are integers.
    V1,
    /// Frame rectangles are floats.
    V2,
    /// Like V2, and the container stores the GIF size before the frames.
    V3,
}

/// Fixed header in front of the images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexHeader {
    /// Raw format id.
    pub format_id: u32,
    /// Decoded format.
    pub format: TexFormat,
    /// Flag bitfield.
    pub flags: TexFlags,
    /// Full texture width, including padding the editor may care about.
    pub texture_width: u32,
    /// Full texture height.
    pub texture_height: u32,
    /// Logical image width. [`Texture::decode_mip`] crops to this when it is non-zero.
    pub image_width: u32,
    /// Logical image height.
    pub image_height: u32,
    /// Header field that is preserved and otherwise unused.
    pub unknown: u32,
}

/// Bytes the editor stores in front of a version-4 mip. Safe to ignore for playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorMeta {
    /// First editor integer.
    pub a: i32,
    /// Second editor integer.
    pub b: i32,
    /// JSON text stored in the mip header.
    pub json: String,
    /// Third editor integer.
    pub c: i32,
}

/// One mip level. `bytes` are LZ4-decoded when the file said they were compressed.
pub struct Mipmap {
    /// Mip width in pixels.
    pub width: u32,
    /// Mip height in pixels.
    pub height: u32,
    /// Whether the on-disk block was LZ4. `bytes` is always the decompressed payload.
    pub was_lz4: bool,
    /// Present for MP4 mips stored in `TEXB0004`.
    pub editor: Option<EditorMeta>,
    /// Decompressed payload: raw pixels, a DXT block list, or an embedded file.
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for Mipmap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Mipmap")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("was_lz4", &self.was_lz4)
            .field("editor", &self.editor)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Mips that belong to one image or GIF frame.
#[derive(Debug)]
pub struct Image {
    /// Mip chain, index 0 is the largest.
    pub mipmaps: Vec<Mipmap>,
}

/// Image container.
#[derive(Debug)]
pub struct ImageContainer {
    /// `TEXB0001` .. `TEXB0004`.
    pub magic: String,
    /// Container revision from the magic.
    pub version: ContainerVersion,
    /// Mip header layout actually used. `TEXB0004` downgrades to [`MipLayout::V2`] unless the payload is MP4.
    pub mip_layout: MipLayout,
    /// FreeImage format, or `-1` when the container has none.
    pub image_format: i32,
    /// Images in container order.
    pub images: Vec<Image>,
}

/// One GIF frame rectangle.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Image index this frame displays.
    pub image_id: i32,
    /// Seconds this frame stays up.
    pub frametime: f32,
    /// Horizontal origin.
    pub x: f32,
    /// Vertical origin.
    pub y: f32,
    /// Width. Zero means the frame is stored rotated and [`Self::height_x`] carries it.
    pub width: f32,
    /// Second width component used by rotated frames.
    pub width_y: f32,
    /// Second height component used by rotated frames.
    pub height_x: f32,
    /// Height.
    pub height: f32,
}

/// Frame table present when [`TexFlags::IS_GIF`] is set.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSet {
    /// `TEXS0001` .. `TEXS0003`.
    pub magic: String,
    /// How the rectangles were stored.
    pub version: FrameVersion,
    /// GIF canvas width. Taken from the first frame when the container leaves it at zero.
    pub gif_width: u32,
    /// GIF canvas height.
    pub gif_height: u32,
    /// Frames in playback order.
    pub frames: Vec<Frame>,
}

/// A parsed texture.
#[derive(Debug)]
pub struct Texture {
    /// `TEXV0005`.
    pub version: String,
    /// `TEXI0001`.
    pub info: String,
    /// Fixed header.
    pub header: TexHeader,
    /// Images.
    pub container: ImageContainer,
    /// Animation frames, when the GIF flag is set.
    pub frames: Option<FrameSet>,
}

impl Texture {
    /// Parse a `.tex` byte string.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let mut cursor = Cursor::new(data);
        let version = cursor.cstring("tex version", MAGIC_LIMIT)?;
        if version != "TEXV0005" {
            return Err(Error::BadMagic {
                context: "tex version",
                found: version,
            });
        }
        let info = cursor.cstring("tex info", MAGIC_LIMIT)?;
        if info != "TEXI0001" {
            return Err(Error::BadMagic {
                context: "tex info",
                found: info,
            });
        }
        let format_id = cursor.u32("tex format")?;
        let flags = TexFlags(cursor.u32("tex flags")?);
        let texture_width = cursor.u32("tex width")?;
        let texture_height = cursor.u32("tex height")?;
        let image_width = cursor.u32("tex image width")?;
        let image_height = cursor.u32("tex image height")?;
        check_dim("tex width", texture_width)?;
        check_dim("tex height", texture_height)?;
        check_dim("tex image width", image_width)?;
        check_dim("tex image height", image_height)?;
        let unknown = cursor.u32("tex header unknown")?;
        let header = TexHeader {
            format_id,
            format: TexFormat::from_u32(format_id),
            flags,
            texture_width,
            texture_height,
            image_width,
            image_height,
            unknown,
        };
        let container = parse_container(&mut cursor)?;
        let frames = if flags.is_gif() {
            Some(parse_frames(&mut cursor)?)
        } else {
            None
        };
        Ok(Self {
            version,
            info,
            header,
            container,
            frames,
        })
    }

    /// Borrow one mip.
    pub fn mip(&self, image: usize, mip: usize) -> Result<&Mipmap, Error> {
        self.container
            .images
            .get(image)
            .and_then(|item| item.mipmaps.get(mip))
            .ok_or(Error::MissingMip { image, mip })
    }

    /// Decode one mip.
    ///
    /// Raw and block formats become [`MipContent::Image`], cropped to the logical
    /// image size when that size fits inside the mip. Embedded files and video
    /// are returned as the decompressed bytes.
    pub fn decode_mip(&self, image: usize, mip_index: usize) -> Result<MipContent<'_>, Error> {
        let format = self.header.format;
        let flags = self.header.flags;
        let image_width = self.header.image_width;
        let image_height = self.header.image_height;
        let image_format = self.container.image_format;
        let mip = self.mip(image, mip_index)?;
        if flags.is_video() || image_format == FREEIMAGE_MP4 {
            return Ok(MipContent::Video { bytes: &mip.bytes });
        }
        let kind = ImageKind::from_freeimage(image_format);
        if kind.is_embedded_file() {
            return Ok(MipContent::Encoded {
                kind,
                bytes: &mip.bytes,
            });
        }
        let rgba = expand_pixels(format, flags, mip)?;
        let cropped = crop_logical(image_width, image_height, rgba)?;
        Ok(MipContent::Image(cropped))
    }

    /// [`Self::decode_mip`] for image 0, mip 0.
    pub fn primary(&self) -> Result<MipContent<'_>, Error> {
        self.decode_mip(0, 0)
    }
}

/// What [`Texture::decode_mip`] returns.
#[derive(Debug)]
pub enum MipContent<'a> {
    /// Expanded RGBA8 pixels.
    Image(RgbaImage),
    /// An embedded image file (PNG, JPEG, and the other FreeImage kinds).
    Encoded {
        /// Classified FreeImage type.
        kind: ImageKind,
        /// File bytes.
        bytes: &'a [u8],
    },
    /// An embedded video.
    Video {
        /// Elementary stream bytes.
        bytes: &'a [u8],
    },
}

/// Tightly packed RGBA8, row-major, origin at the top left.
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for RgbaImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

impl RgbaImage {
    /// One pixel, or `None` when `x`/`y` is outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let row = usize::try_from(y)
            .ok()?
            .checked_mul(usize::try_from(self.width).ok()?)?;
        let index = row.checked_add(usize::try_from(x).ok()?)?.checked_mul(4)?;
        let bytes = self.pixels.get(index..index.checked_add(4)?)?;
        let mut pixel = [0u8; 4];
        pixel.copy_from_slice(bytes);
        Some(pixel)
    }

    /// Copy the top-left `width` by `height` region.
    pub fn crop(&self, width: u32, height: u32) -> Result<Self, Error> {
        if width > self.width || height > self.height {
            return Err(Error::Crop {
                width: self.width,
                height: self.height,
                crop_width: width,
                crop_height: height,
            });
        }
        if width == self.width && height == self.height {
            return Ok(self.clone());
        }
        let row_bytes = usize::try_from(width)
            .ok()
            .and_then(|value| value.checked_mul(4))
            .ok_or(Error::Invariant("crop row"))?;
        let mut pixels = Vec::new();
        for y in 0..height {
            let start = usize::try_from(y)
                .ok()
                .and_then(|row| {
                    usize::try_from(self.width)
                        .ok()
                        .and_then(|w| row.checked_mul(w))
                })
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or(Error::Invariant("crop start"))?;
            let end = start
                .checked_add(row_bytes)
                .ok_or(Error::Invariant("crop end"))?;
            let row = self
                .pixels
                .get(start..end)
                .ok_or(Error::Invariant("crop row bytes"))?;
            pixels.extend_from_slice(row);
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }
}

fn parse_container(cursor: &mut Cursor<'_>) -> Result<ImageContainer, Error> {
    let magic = cursor.cstring("tex container", MAGIC_LIMIT)?;
    let image_count = cursor.u32("tex image count")?;
    if image_count > limits::MAX_IMAGE_COUNT {
        return Err(Error::Limit {
            context: "tex image count",
            value: u64::from(image_count),
            limit: u64::from(limits::MAX_IMAGE_COUNT),
        });
    }
    let (version, image_format, mip_layout) = match magic.as_str() {
        "TEXB0001" => (ContainerVersion::TexB0001, FREEIMAGE_UNKNOWN, MipLayout::V1),
        "TEXB0002" => (ContainerVersion::TexB0002, FREEIMAGE_UNKNOWN, MipLayout::V2),
        "TEXB0003" => {
            let format = cursor.i32("tex freeimage format")?;
            (ContainerVersion::TexB0003, format, MipLayout::V2)
        }
        "TEXB0004" => {
            let mut format = cursor.i32("tex freeimage format")?;
            let video_flag = cursor.i32("tex video flag")?;
            if format == FREEIMAGE_UNKNOWN && video_flag == 1 {
                format = FREEIMAGE_MP4;
            }
            let layout = if format == FREEIMAGE_MP4 {
                MipLayout::V4
            } else {
                MipLayout::V2
            };
            (ContainerVersion::TexB0004, format, layout)
        }
        other => {
            return Err(Error::BadMagic {
                context: "tex container",
                found: other.to_owned(),
            });
        }
    };
    let mut images = Vec::with_capacity(usize::try_from(image_count).unwrap_or(0));
    for _ in 0..image_count {
        images.push(parse_image(cursor, mip_layout)?);
    }
    Ok(ImageContainer {
        magic,
        version,
        mip_layout,
        image_format,
        images,
    })
}

fn parse_image(cursor: &mut Cursor<'_>, layout: MipLayout) -> Result<Image, Error> {
    let mip_count = cursor.u32("tex mip count")?;
    if mip_count > limits::MAX_MIPMAP_COUNT {
        return Err(Error::Limit {
            context: "tex mip count",
            value: u64::from(mip_count),
            limit: u64::from(limits::MAX_MIPMAP_COUNT),
        });
    }
    let mut mipmaps = Vec::with_capacity(usize::try_from(mip_count).unwrap_or(0));
    for _ in 0..mip_count {
        mipmaps.push(parse_mip(cursor, layout)?);
    }
    Ok(Image { mipmaps })
}

fn parse_mip(cursor: &mut Cursor<'_>, layout: MipLayout) -> Result<Mipmap, Error> {
    let editor = if layout == MipLayout::V4 {
        Some(EditorMeta {
            a: cursor.i32("tex v4 a")?,
            b: cursor.i32("tex v4 b")?,
            json: cursor.cstring("tex v4 json", limits::MAX_STRING_LEN)?,
            c: cursor.i32("tex v4 c")?,
        })
    } else {
        None
    };
    let width = cursor.u32("tex mip width")?;
    let height = cursor.u32("tex mip height")?;
    check_dim("tex mip width", width)?;
    check_dim("tex mip height", height)?;
    let (was_lz4, uncompressed, stored) = match layout {
        MipLayout::V1 => (false, 0, cursor.u32("tex mip bytes")?),
        MipLayout::V2 | MipLayout::V4 => {
            let flag = cursor.u32("tex mip compression")?;
            let uncompressed = cursor.u32("tex mip uncompressed")?;
            let stored = cursor.u32("tex mip bytes")?;
            let lz4 = match flag {
                0 => false,
                1 => true,
                other => {
                    return Err(Error::BadCompression {
                        context: "tex mip",
                        flag: other,
                    });
                }
            };
            (lz4, uncompressed, stored)
        }
    };
    let raw = cursor.bytes_vec(stored, "tex mip bytes")?;
    let bytes = if was_lz4 {
        let expected = usize::try_from(uncompressed).map_err(|_| Error::Limit {
            context: "tex mip uncompressed",
            value: u64::from(uncompressed),
            limit: limits::MAX_MIP_BYTES as u64,
        })?;
        if expected > limits::MAX_MIP_BYTES {
            return Err(Error::Limit {
                context: "tex mip uncompressed",
                value: u64::from(uncompressed),
                limit: limits::MAX_MIP_BYTES as u64,
            });
        }
        lz4_flex::block::decompress(&raw, expected).map_err(|err| Error::Lz4 {
            context: "tex mip",
            detail: err.to_string(),
        })?
    } else {
        raw
    };
    Ok(Mipmap {
        width,
        height,
        was_lz4,
        editor,
        bytes,
    })
}

fn parse_frames(cursor: &mut Cursor<'_>) -> Result<FrameSet, Error> {
    let magic = cursor.cstring("tex frames", MAGIC_LIMIT)?;
    let count = cursor.u32("tex frame count")?;
    if count > limits::MAX_FRAME_COUNT {
        return Err(Error::Limit {
            context: "tex frame count",
            value: u64::from(count),
            limit: u64::from(limits::MAX_FRAME_COUNT),
        });
    }
    let (version, mut gif_width, mut gif_height) = match magic.as_str() {
        "TEXS0001" => (FrameVersion::V1, 0, 0),
        "TEXS0002" => (FrameVersion::V2, 0, 0),
        "TEXS0003" => {
            let width = cursor.u32("tex gif width")?;
            let height = cursor.u32("tex gif height")?;
            (FrameVersion::V3, width, height)
        }
        other => {
            return Err(Error::BadMagic {
                context: "tex frames",
                found: other.to_owned(),
            });
        }
    };
    let mut frames = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for _ in 0..count {
        let image_id = cursor.i32("tex frame image")?;
        let frametime = cursor.f32("tex frame time")?;
        let (x, y, width, width_y, height_x, height) = match version {
            FrameVersion::V1 => (
                cursor.i32("tex frame x")? as f32,
                cursor.i32("tex frame y")? as f32,
                cursor.i32("tex frame width")? as f32,
                cursor.i32("tex frame width_y")? as f32,
                cursor.i32("tex frame height_x")? as f32,
                cursor.i32("tex frame height")? as f32,
            ),
            FrameVersion::V2 | FrameVersion::V3 => (
                cursor.f32("tex frame x")?,
                cursor.f32("tex frame y")?,
                cursor.f32("tex frame width")?,
                cursor.f32("tex frame width_y")?,
                cursor.f32("tex frame height_x")?,
                cursor.f32("tex frame height")?,
            ),
        };
        frames.push(Frame {
            image_id,
            frametime,
            x,
            y,
            width,
            width_y,
            height_x,
            height,
        });
    }
    if gif_width == 0 || gif_height == 0 {
        if let Some(frame) = frames.first() {
            let width = if frame.width != 0.0 {
                frame.width
            } else {
                frame.height_x
            };
            let height = if frame.height != 0.0 {
                frame.height
            } else {
                frame.width_y
            };
            gif_width = positive_dim(width);
            gif_height = positive_dim(height);
        }
    }
    Ok(FrameSet {
        magic,
        version,
        gif_width,
        gif_height,
        frames,
    })
}

fn expand_pixels(format: TexFormat, flags: TexFlags, mip: &Mipmap) -> Result<RgbaImage, Error> {
    match format {
        TexFormat::Rgba8888 => expand_groups(&mip.bytes, mip.width, mip.height, 4, |group| {
            Ok([group[0], group[1], group[2], group[3]])
        }),
        TexFormat::Rgb888 => expand_groups(&mip.bytes, mip.width, mip.height, 3, |group| {
            Ok([group[0], group[1], group[2], 255])
        }),
        TexFormat::Rgb565 => expand_groups(&mip.bytes, mip.width, mip.height, 2, |group| {
            let packed = u16::from_le_bytes([group[0], group[1]]);
            Ok(rgb565(packed))
        }),
        TexFormat::Rg88 => expand_groups(&mip.bytes, mip.width, mip.height, 2, |group| {
            if flags.alpha_priority() {
                Ok([group[0], group[0], group[0], group[1]])
            } else {
                Ok([group[0], group[1], 0, 255])
            }
        }),
        TexFormat::R8 => expand_groups(&mip.bytes, mip.width, mip.height, 1, |group| {
            if flags.alpha_priority() {
                Ok([255, 255, 255, group[0]])
            } else {
                Ok([group[0], group[0], group[0], 255])
            }
        }),
        TexFormat::Dxt1 => dxt::decode(BlockFormat::Dxt1, mip.width, mip.height, &mip.bytes),
        TexFormat::Dxt3 => dxt::decode(BlockFormat::Dxt3, mip.width, mip.height, &mip.bytes),
        TexFormat::Dxt5 => dxt::decode(BlockFormat::Dxt5, mip.width, mip.height, &mip.bytes),
        other => Err(Error::UnsupportedPixels {
            format: other.label(),
        }),
    }
}

fn expand_groups<F>(
    bytes: &[u8],
    width: u32,
    height: u32,
    group: usize,
    mut map: F,
) -> Result<RgbaImage, Error>
where
    F: FnMut(&[u8]) -> Result<[u8; 4], Error>,
{
    let pixel_count = size::byte_len(width, height, 1)?;
    let needed = pixel_count
        .checked_mul(group)
        .ok_or(Error::Invariant("texel stride"))?;
    if bytes.len() < needed {
        return Err(Error::SizeMismatch {
            context: "tex pixels",
            expected: needed,
            actual: bytes.len(),
        });
    }
    let source = bytes
        .get(..needed)
        .ok_or(Error::Invariant("texel prefix"))?;
    let mut pixels = Vec::with_capacity(pixel_count.saturating_mul(4));
    for chunk in source.chunks(group) {
        let pixel = map(chunk)?;
        pixels.extend_from_slice(&pixel);
    }
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

fn crop_logical(image_width: u32, image_height: u32, image: RgbaImage) -> Result<RgbaImage, Error> {
    if image_width == 0
        || image_height == 0
        || (image_width == image.width && image_height == image.height)
    {
        return Ok(image);
    }
    image.crop(image_width, image_height)
}

fn check_dim(context: &'static str, value: u32) -> Result<(), Error> {
    if value > limits::MAX_DIMENSION {
        Err(Error::Limit {
            context,
            value: u64::from(value),
            limit: u64::from(limits::MAX_DIMENSION),
        })
    } else {
        Ok(())
    }
}

fn positive_dim(value: f32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let rounded = value.round();
    if rounded >= u32::MAX as f32 {
        u32::MAX
    } else {
        rounded as u32
    }
}

fn rgb565(packed: u16) -> [u8; 4] {
    let r5 = ((packed >> 11) & 0x1f) as u8;
    let g6 = ((packed >> 5) & 0x3f) as u8;
    let b5 = (packed & 0x1f) as u8;
    [
        (r5 << 3) | (r5 >> 2),
        (g6 << 2) | (g6 >> 4),
        (b5 << 3) | (b5 >> 2),
        255,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{must, push_cstr, push_i32, push_u32};

    struct Built {
        bytes: Vec<u8>,
    }

    fn raw_tex(format: u32, flags: u32, width: u32, height: u32, container: &str) -> Built {
        let mut bytes = Vec::new();
        push_cstr(&mut bytes, "TEXV0005");
        push_cstr(&mut bytes, "TEXI0001");
        push_u32(&mut bytes, format);
        push_u32(&mut bytes, flags);
        push_u32(&mut bytes, width);
        push_u32(&mut bytes, height);
        push_u32(&mut bytes, width);
        push_u32(&mut bytes, height);
        push_u32(&mut bytes, 0);
        push_cstr(&mut bytes, container);
        Built { bytes }
    }

    #[test]
    fn r8_expands_to_opaque_gray() {
        let mut tex = raw_tex(9, 0, 2, 2, "TEXB0003");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, FREEIMAGE_UNKNOWN);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 2);
        push_u32(&mut tex.bytes, 2);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 4);
        push_u32(&mut tex.bytes, 4);
        tex.bytes.extend_from_slice(&[1, 2, 3, 4]);
        let texture = must(Texture::parse(&tex.bytes));
        let MipContent::Image(image) = must(texture.primary()) else {
            panic!("expected pixels");
        };
        assert_eq!(image.pixel(0, 0), Some([1, 1, 1, 255]));
        assert_eq!(image.pixel(1, 0), Some([2, 2, 2, 255]));
        assert_eq!(image.pixel(0, 1), Some([3, 3, 3, 255]));
        assert_eq!(image.pixel(1, 1), Some([4, 4, 4, 255]));
    }

    #[test]
    fn r8_alpha_priority_puts_the_sample_in_alpha() {
        let mut tex = raw_tex(9, TexFlags::ALPHA_CHANNEL_PRIORITY, 1, 1, "TEXB0002");
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        tex.bytes.push(10);
        let texture = must(Texture::parse(&tex.bytes));
        let MipContent::Image(image) = must(texture.primary()) else {
            panic!("expected pixels");
        };
        assert_eq!(image.pixel(0, 0), Some([255, 255, 255, 10]));
    }

    #[test]
    fn lz4_r8_roundtrips() {
        let samples = [9u8, 8, 7, 6];
        let compressed = lz4_flex::block::compress(&samples);
        let mut tex = raw_tex(9, 0, 2, 2, "TEXB0003");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, -1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 2);
        push_u32(&mut tex.bytes, 2);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 4);
        push_u32(
            &mut tex.bytes,
            u32::try_from(compressed.len()).expect("compressed len"),
        );
        tex.bytes.extend_from_slice(&compressed);
        let texture = must(Texture::parse(&tex.bytes));
        assert!(texture.mip(0, 0).expect("mip").was_lz4);
        let MipContent::Image(image) = must(texture.primary()) else {
            panic!("expected pixels");
        };
        assert_eq!(image.pixel(0, 0), Some([9, 9, 9, 255]));
        assert_eq!(image.pixel(1, 1), Some([6, 6, 6, 255]));
    }

    #[test]
    fn rgba8888_keeps_channel_order_and_crops_to_the_logical_size() {
        let mut tex = raw_tex(0, 0, 2, 1, "TEXB0001");
        // Logical size is smaller than the mip. Rewrite image width after the shared header.
        // The helper writes image size equal to the mip; patch bytes 4+4+4 of the sizes.
        // Header after magics: format, flags, tex_w, tex_h, image_w, image_h, unknown.
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 2);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 8);
        tex.bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut texture = must(Texture::parse(&tex.bytes));
        texture.header.image_width = 1;
        texture.header.image_height = 1;
        let MipContent::Image(image) = must(texture.primary()) else {
            panic!("expected pixels");
        };
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixel(0, 0), Some([1, 2, 3, 4]));
    }

    #[test]
    fn rg88_without_the_priority_flag_keeps_both_channels() {
        let mut tex = raw_tex(8, 0, 1, 1, "TEXB0003");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, -1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 2);
        push_u32(&mut tex.bytes, 2);
        tex.bytes.extend_from_slice(&[10, 20]);
        let texture = must(Texture::parse(&tex.bytes));
        let MipContent::Image(image) = must(texture.primary()) else {
            panic!("expected pixels");
        };
        assert_eq!(image.pixel(0, 0), Some([10, 20, 0, 255]));
    }

    #[test]
    fn rg88_alpha_priority_uses_the_second_channel_as_opacity() {
        let mut tex = raw_tex(8, TexFlags::ALPHA_CHANNEL_PRIORITY, 1, 1, "TEXB0003");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, -1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 2);
        push_u32(&mut tex.bytes, 2);
        tex.bytes.extend_from_slice(&[10, 20]);
        let texture = must(Texture::parse(&tex.bytes));
        let MipContent::Image(image) = must(texture.primary()) else {
            panic!("expected pixels");
        };
        assert_eq!(image.pixel(0, 0), Some([10, 10, 10, 20]));
    }

    #[test]
    fn rgb565_expands_pure_red() {
        let mut tex = raw_tex(2, 0, 1, 1, "TEXB0003");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, -1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 2);
        push_u32(&mut tex.bytes, 2);
        tex.bytes.extend_from_slice(&0xF800_u16.to_le_bytes());
        let texture = must(Texture::parse(&tex.bytes));
        let MipContent::Image(image) = must(texture.primary()) else {
            panic!("expected pixels");
        };
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
    }

    #[test]
    fn dxt1_mip_decodes_through_the_container() {
        let mut tex = raw_tex(7, 0, 4, 1, "TEXB0003");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, -1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 4);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 8);
        push_u32(&mut tex.bytes, 8);
        tex.bytes
            .extend_from_slice(&[0x00, 0xF8, 0x00, 0x00, 0xE4, 0x00, 0x00, 0x00]);
        let texture = must(Texture::parse(&tex.bytes));
        let MipContent::Image(image) = must(texture.primary()) else {
            panic!("expected pixels");
        };
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(2, 0), Some([170, 0, 0, 255]));
    }

    #[test]
    fn embedded_png_is_not_read_as_pixels() {
        let mut tex = raw_tex(0, 0, 1, 1, "TEXB0003");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, 13);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 8);
        push_u32(&mut tex.bytes, 8);
        let png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        tex.bytes.extend_from_slice(&png);
        let texture = must(Texture::parse(&tex.bytes));
        match texture.primary() {
            Ok(MipContent::Encoded {
                kind: ImageKind::Png,
                bytes,
            }) => assert_eq!(bytes, &png),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn texb0004_png_uses_the_version3_mip_header() {
        let mut tex = raw_tex(0, 0, 1, 1, "TEXB0004");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, 13);
        push_i32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 4);
        push_u32(&mut tex.bytes, 4);
        tex.bytes.extend_from_slice(b"png!");
        let texture = must(Texture::parse(&tex.bytes));
        assert_eq!(texture.container.version, ContainerVersion::TexB0004);
        assert_eq!(texture.container.mip_layout, MipLayout::V2);
        match texture.primary() {
            Ok(MipContent::Encoded {
                kind: ImageKind::Png,
                bytes,
            }) => assert_eq!(bytes, b"png!"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn texb0004_mp4_keeps_the_editor_preamble() {
        let mut tex = raw_tex(0, TexFlags::VIDEO, 1, 1, "TEXB0004");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, -1);
        push_i32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, 2);
        push_cstr(&mut tex.bytes, "{}");
        push_i32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 0);
        push_u32(&mut tex.bytes, 4);
        push_u32(&mut tex.bytes, 4);
        tex.bytes.extend_from_slice(b"ftyp");
        let texture = must(Texture::parse(&tex.bytes));
        assert_eq!(texture.container.image_format, FREEIMAGE_MP4);
        assert_eq!(texture.container.mip_layout, MipLayout::V4);
        let mip = must(texture.mip(0, 0));
        assert_eq!(
            mip.editor.as_ref().map(|meta| meta.json.as_str()),
            Some("{}")
        );
        match texture.primary() {
            Ok(MipContent::Video { bytes }) => assert_eq!(bytes, b"ftyp"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn gif_frame_table_is_parsed_and_supplies_the_canvas_size() {
        let mut tex = raw_tex(9, TexFlags::IS_GIF, 1, 1, "TEXB0001");
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        tex.bytes.push(0);
        push_cstr(&mut tex.bytes, "TEXS0001");
        push_u32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, 0);
        crate::test_support::push_f32(&mut tex.bytes, 0.1);
        push_i32(&mut tex.bytes, 1);
        push_i32(&mut tex.bytes, 2);
        push_i32(&mut tex.bytes, 3);
        push_i32(&mut tex.bytes, 0);
        push_i32(&mut tex.bytes, 0);
        push_i32(&mut tex.bytes, 4);
        let texture = must(Texture::parse(&tex.bytes));
        let frames = texture.frames.expect("frames");
        assert_eq!(frames.version, FrameVersion::V1);
        assert_eq!((frames.gif_width, frames.gif_height), (3, 4));
        assert_eq!(frames.frames[0].image_id, 0);
        assert_eq!(frames.frames[0].x, 1.0);
    }

    #[test]
    fn a_short_magic_is_rejected() {
        assert!(matches!(
            Texture::parse(b"TEXV0005"),
            Err(Error::UnexpectedEof { .. } | Error::Limit { .. })
        ));
    }

    #[test]
    fn bc7_parses_and_refuses_to_pretend_it_is_rgba() {
        let mut tex = raw_tex(12, 0, 4, 4, "TEXB0001");
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 1);
        push_u32(&mut tex.bytes, 4);
        push_u32(&mut tex.bytes, 4);
        push_u32(&mut tex.bytes, 16);
        tex.bytes.extend_from_slice(&[0u8; 16]);
        let texture = must(Texture::parse(&tex.bytes));
        assert!(matches!(texture.header.format, TexFormat::Bc7));
        assert_eq!(must(texture.mip(0, 0)).bytes.len(), 16);
        assert!(matches!(
            texture.primary(),
            Err(Error::UnsupportedPixels { .. })
        ));
    }
}
