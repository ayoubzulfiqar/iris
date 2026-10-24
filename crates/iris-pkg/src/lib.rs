//! Wallpaper Engine package reader.
//!
//! * [`pkg`] reads `PKGV` archives into owned entry bytes.
//! * [`tex`] reads `TEXV0005` textures, LZ4-decompresses mip blocks, and expands
//!   DXT1, DXT3, DXT5, RGBA8, RGB8, RGB565, RG88, and R8 into RGBA8.
//! * [`scene`] reads `scene.json` and `project.json`.
//! * [`decoded`] is the RGBA8 image the renderer uploads.
//!
//! The crate does not render, and it does not write extracted files to disk.

#![forbid(unsafe_code)]

mod cursor;
mod decoded;
mod dxt;
mod error;
mod limits;
mod pkg;
mod scene;
mod size;
mod tex;

pub use decoded::DecodedTexture;
pub use error::Error;
pub use pkg::{Entry, Package};
pub use scene::{
    Camera, ElementKind, General, OrthoProjection, Project, Scene, SceneEffect, SceneObject,
    SceneVec, UserValue, WorkshopId,
};
pub use tex::{
    ContainerVersion, EditorMeta, Frame, FrameSet, FrameVersion, Image, ImageContainer, ImageKind,
    MipContent, MipLayout, Mipmap, RgbaImage, TexFlags, TexFormat, TexHeader, Texture,
};

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!("expected Ok, got {err:?}"),
        }
    }

    pub(crate) fn push_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn push_i32(buf: &mut Vec<u8>, value: i32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn push_f32(buf: &mut Vec<u8>, value: f32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn push_sized(buf: &mut Vec<u8>, text: &str) {
        push_u32(
            buf,
            u32::try_from(text.len()).expect("test string fits in u32"),
        );
        buf.extend_from_slice(text.as_bytes());
    }

    pub(crate) fn push_cstr(buf: &mut Vec<u8>, text: &str) {
        buf.extend_from_slice(text.as_bytes());
        buf.push(0);
    }
}
