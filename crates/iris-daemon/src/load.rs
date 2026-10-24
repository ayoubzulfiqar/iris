//! Load a still image from a file, a `.pkg`, or an unpacked scene directory.
//!
//! Video files are opened by [`load_wallpaper`], which starts the decode thread.

use std::fs::{self, FileType};
use std::io::ErrorKind;
use std::path::Path;

use iris_core::WallpaperSource;
use iris_pkg::{DecodedTexture, ImageKind, MipContent, Package, Project, Scene, Texture};
use iris_scene::ScenePackage;
#[cfg(target_os = "linux")]
use iris_video::{PlaybackOptions, VideoPlayer};

use crate::error::Error;

/// A wallpaper that is either a still image or a running video decoder.
pub(crate) enum Loaded {
    /// RGBA pixels.
    Still(DecodedTexture),
    /// Decode thread and ring buffer.
    #[cfg(target_os = "linux")]
    Video(VideoPlayer),
}

/// Open `source`. Video starts a decoder; every other kind is a still image.
#[cfg(target_os = "linux")]
pub(crate) fn load_wallpaper(
    source: &WallpaperSource,
    options: PlaybackOptions,
) -> Result<Loaded, Error> {
    match source {
        WallpaperSource::Video { path } => Ok(Loaded::Video(VideoPlayer::open(path, options)?)),
        other => Ok(Loaded::Still(load_source(other)?)),
    }
}

/// Open `source`. Video is unsupported on this target and names the path.
#[cfg(not(target_os = "linux"))]
pub(crate) fn load_wallpaper(source: &WallpaperSource) -> Result<Loaded, Error> {
    match source {
        WallpaperSource::Video { path } => Err(Error::Unsupported(format!(
            "video is not supported: {}",
            path.display()
        ))),
        other => Ok(Loaded::Still(load_source(other)?)),
    }
}

const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DIMENSION: u32 = 16_384;
const MAX_VISITED: usize = 4096;

/// Compile a scene when the source has a `scene.json` with objects.
///
/// An absent scene returns `Ok(None)` so the caller can fall back to a still image.
pub(crate) fn prepare_scene(source: &WallpaperSource) -> Result<Option<ScenePackage>, Error> {
    match source {
        WallpaperSource::Package { path } => prepare_package_scene(path),
        WallpaperSource::SceneDirectory { path } => prepare_directory_scene(path),
        _ => Ok(None),
    }
}

fn prepare_package_scene(path: &Path) -> Result<Option<ScenePackage>, Error> {
    ensure_regular_file(path)?;
    let package = Package::open(path)?;
    let Some(scene) = Scene::from_package(&package)? else {
        return Ok(None);
    };
    if scene.objects.is_empty() {
        return Ok(None);
    }
    let mut images = Vec::new();
    let mut particles = Vec::new();
    for object in &scene.objects {
        if images.len() >= 64 && particles.len() >= 32 {
            break;
        }
        if let Some(image) = &object.image {
            if images.len() < 64 {
                if let Some(bytes) = package.get(image) {
                    if let Some(texture) = try_bytes(image, bytes) {
                        images.push((image.clone(), texture));
                    }
                }
            }
        }
        if let Some(particle) = &object.particle {
            if particles.len() < 32 {
                if let Some(bytes) = package.get(particle) {
                    particles.push((particle.clone(), bytes.to_vec()));
                }
            }
        }
    }
    Ok(Some(ScenePackage::compile(&scene, images, particles)))
}

fn prepare_directory_scene(dir: &Path) -> Result<Option<ScenePackage>, Error> {
    let Some(bytes) = read_doc(dir, "scene.json")? else {
        return Ok(None);
    };
    let scene = Scene::parse(&bytes)?;
    if scene.objects.is_empty() {
        return Ok(None);
    }
    let mut images = Vec::new();
    let mut particles = Vec::new();
    for object in &scene.objects {
        if let Some(image) = &object.image {
            if images.len() < 64 {
                if let Some(texture) = try_scene_file(dir, image)? {
                    images.push((image.clone(), texture));
                }
            }
        }
        if let Some(particle) = &object.particle {
            if particles.len() < 32 {
                if let Some(relative) = safe_relative(particle) {
                    if let Some(bytes) = read_regular(&dir.join(relative))? {
                        particles.push((particle.clone(), bytes));
                    }
                }
            }
        }
    }
    Ok(Some(ScenePackage::compile(&scene, images, particles)))
}

/// Decode a still `source` into RGBA8.
///
/// Video sources are not still images. [`load_wallpaper`] opens those.
pub(crate) fn load_source(source: &WallpaperSource) -> Result<DecodedTexture, Error> {
    match source {
        WallpaperSource::Image { path } => load_image_file(path),
        WallpaperSource::Package { path } => load_package(path),
        WallpaperSource::SceneDirectory { path } => load_scene_dir(path),
        WallpaperSource::Video { path } => Err(Error::Load(format!(
            "video is opened on the frame clock: {}",
            path.display()
        ))),
        _ => Err(Error::Unsupported(
            "unsupported wallpaper source".to_owned(),
        )),
    }
}

fn load_image_file(path: &Path) -> Result<DecodedTexture, Error> {
    let meta = fs::symlink_metadata(path)
        .map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
    if !meta.file_type().is_file() {
        return Err(Error::Load(format!("{} is not a file", path.display())));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(Error::Load(format!("{} exceeds 256 MiB", path.display())));
    }
    let image = image::ImageReader::open(path)
        .map_err(|err| Error::Image(format!("{}: {err}", path.display())))?
        .with_guessed_format()
        .map_err(|err| Error::Image(err.to_string()))?
        .decode()
        .map_err(|err| Error::Image(err.to_string()))?;
    decoded_from_dynamic(image)
}

fn load_package(path: &Path) -> Result<DecodedTexture, Error> {
    ensure_regular_file(path)?;
    let package = Package::open(path)?;
    if let Some(project) = Project::from_package(&package)? {
        if let Some(preview) = project.preview.as_deref() {
            if let Some(texture) = try_package_path(&package, preview) {
                return Ok(texture);
            }
        }
    }
    if let Some(scene) = Scene::from_package(&package)? {
        for object in &scene.objects {
            if let Some(image) = object.image.as_deref() {
                if let Some(texture) = try_package_path(&package, image) {
                    return Ok(texture);
                }
            }
        }
    }
    for entry in package.entries() {
        if u64::from(entry.length) > MAX_FILE_BYTES {
            continue;
        }
        if let Some(texture) = try_bytes(&entry.path, &entry.bytes) {
            return Ok(texture);
        }
    }
    Err(Error::Load("no decodable image".to_owned()))
}

fn try_package_path(package: &Package, path: &str) -> Option<DecodedTexture> {
    let bytes = package.get(path)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    try_bytes(path, bytes)
}

fn load_scene_dir(dir: &Path) -> Result<DecodedTexture, Error> {
    let meta =
        fs::symlink_metadata(dir).map_err(|err| Error::Io(format!("{}: {err}", dir.display())))?;
    if !meta.file_type().is_dir() {
        return Err(Error::Load(format!("{} is not a directory", dir.display())));
    }
    if let Some(bytes) = read_doc(dir, "project.json")? {
        let project = Project::parse(&bytes)?;
        if let Some(preview) = project.preview.as_deref() {
            if let Some(texture) = try_scene_file(dir, preview)? {
                return Ok(texture);
            }
        }
    }
    if let Some(bytes) = read_doc(dir, "scene.json")? {
        let scene = Scene::parse(&bytes)?;
        for object in &scene.objects {
            if let Some(image) = object.image.as_deref() {
                if let Some(texture) = try_scene_file(dir, image)? {
                    return Ok(texture);
                }
            }
        }
    }
    let mut visited = 0usize;
    if let Some(texture) = walk_dir(dir, 0, &mut visited)? {
        return Ok(texture);
    }
    Err(Error::Load("no decodable image".to_owned()))
}

fn try_scene_file(root: &Path, raw: &str) -> Result<Option<DecodedTexture>, Error> {
    let Some(relative) = safe_relative(raw) else {
        return Ok(None);
    };
    let path = root.join(&relative);
    let Some(bytes) = read_regular(&path)? else {
        return Ok(None);
    };
    Ok(try_bytes(&relative, &bytes))
}

fn read_doc(dir: &Path, name: &str) -> Result<Option<Vec<u8>>, Error> {
    if let Some(bytes) = read_regular(&dir.join(name))? {
        return Ok(Some(bytes));
    }
    let entries =
        fs::read_dir(dir).map_err(|err| Error::Io(format!("{}: {err}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| Error::Io(err.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|err| Error::Io(err.to_string()))?;
        if !is_real_dir(&file_type) {
            continue;
        }
        if let Some(bytes) = read_regular(&entry.path().join(name))? {
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

fn walk_dir(
    dir: &Path,
    depth: usize,
    visited: &mut usize,
) -> Result<Option<DecodedTexture>, Error> {
    if *visited >= MAX_VISITED {
        return Ok(None);
    }
    let entries =
        fs::read_dir(dir).map_err(|err| Error::Io(format!("{}: {err}", dir.display())))?;
    for entry in entries {
        if *visited >= MAX_VISITED {
            return Ok(None);
        }
        *visited += 1;
        let entry = entry.map_err(|err| Error::Io(err.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|err| Error::Io(err.to_string()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if depth < 2 {
                if let Some(texture) = walk_dir(&entry.path(), depth + 1, visited)? {
                    return Ok(Some(texture));
                }
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let meta = entry.metadata().map_err(|err| Error::Io(err.to_string()))?;
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::debug!("skip {}: {err}", path.display());
                continue;
            }
        };
        if let Some(texture) = try_bytes(name, &bytes) {
            return Ok(Some(texture));
        }
    }
    Ok(None)
}

fn try_bytes(path: &str, bytes: &[u8]) -> Option<DecodedTexture> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let extension = extension_lower(path);
    if extension == Some("tex") || bytes.starts_with(b"TEXV") {
        return decode_tex(bytes).ok().flatten();
    }
    match extension {
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp") => decode_memory(bytes).ok(),
        _ => None,
    }
}

fn decode_tex(bytes: &[u8]) -> Result<Option<DecodedTexture>, Error> {
    let texture = Texture::parse(bytes)?;
    match texture.primary()? {
        MipContent::Image(rgba) => {
            if rgba.width == 0
                || rgba.height == 0
                || rgba.width > MAX_DIMENSION
                || rgba.height > MAX_DIMENSION
            {
                return Ok(None);
            }
            Ok(Some(DecodedTexture::from_rgba(rgba)?))
        }
        MipContent::Encoded { kind, bytes } => match kind {
            ImageKind::Png | ImageKind::Jpeg | ImageKind::Gif | ImageKind::Bmp => {
                decode_memory(bytes).map(Some)
            }
            ImageKind::Tiff | ImageKind::Mp4 | ImageKind::Unknown | ImageKind::Other(_) => Ok(None),
        },
        MipContent::Video { .. } => Ok(None),
    }
}

fn decode_memory(bytes: &[u8]) -> Result<DecodedTexture, Error> {
    let image = image::load_from_memory(bytes).map_err(|err| Error::Image(err.to_string()))?;
    decoded_from_dynamic(image)
}

fn decoded_from_dynamic(image: image::DynamicImage) -> Result<DecodedTexture, Error> {
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(Error::Load(format!(
            "image dimensions {width}x{height} are outside 1..={MAX_DIMENSION}"
        )));
    }
    DecodedTexture::new(width, height, rgba.into_raw()).map_err(Error::from)
}

fn ensure_regular_file(path: &Path) -> Result<(), Error> {
    let meta = fs::symlink_metadata(path)
        .map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
    if meta.file_type().is_file() {
        Ok(())
    } else {
        Err(Error::Load(format!("{} is not a file", path.display())))
    }
}

fn read_regular(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::Io(format!("{}: {err}", path.display()))),
    };
    if !meta.file_type().is_file() || meta.len() > MAX_FILE_BYTES {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
    Ok(Some(bytes))
}

fn is_real_dir(file_type: &FileType) -> bool {
    file_type.is_dir() && !file_type.is_symlink()
}

fn extension_lower(path: &str) -> Option<&str> {
    let slash = path.rfind(['/', '\\']).map(|index| index + 1).unwrap_or(0);
    let name = path.get(slash..)?;
    let dot = name.rfind('.')?;
    let ext = name.get(dot + 1..)?;
    if ext.eq_ignore_ascii_case("png") {
        Some("png")
    } else if ext.eq_ignore_ascii_case("jpg") {
        Some("jpg")
    } else if ext.eq_ignore_ascii_case("jpeg") {
        Some("jpeg")
    } else if ext.eq_ignore_ascii_case("gif") {
        Some("gif")
    } else if ext.eq_ignore_ascii_case("bmp") {
        Some("bmp")
    } else if ext.eq_ignore_ascii_case("tex") {
        Some("tex")
    } else {
        None
    }
}

/// Reject absolute paths, drive prefixes, `.`, `..`, and empty components.
fn safe_relative(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains(':') {
        return None;
    }
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use image::ImageEncoder;

    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("iris-load-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn png_1x1() -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::codecs::png::PngEncoder::new(&mut cursor)
            .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .expect("encode png");
        cursor.into_inner()
    }

    fn push_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn push_cstr(buf: &mut Vec<u8>, text: &str) {
        buf.extend_from_slice(text.as_bytes());
        buf.push(0);
    }

    fn tex_1x1() -> Vec<u8> {
        let mut buf = Vec::new();
        push_cstr(&mut buf, "TEXV0005");
        push_cstr(&mut buf, "TEXI0001");
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 0);
        push_cstr(&mut buf, "TEXB0001");
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 4);
        buf.extend_from_slice(&[1, 2, 3, 255]);
        buf
    }

    fn pkg(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut directory = Vec::new();
        let mut payload = Vec::new();
        for (path, bytes) in entries {
            push_u32(&mut directory, u32::try_from(path.len()).expect("path"));
            directory.extend_from_slice(path.as_bytes());
            push_u32(
                &mut directory,
                u32::try_from(payload.len()).expect("offset"),
            );
            push_u32(&mut directory, u32::try_from(bytes.len()).expect("length"));
            payload.extend_from_slice(bytes);
        }
        let magic = b"PKGV0001";
        let mut out = Vec::new();
        push_u32(&mut out, u32::try_from(magic.len()).expect("magic"));
        out.extend_from_slice(magic);
        push_u32(&mut out, u32::try_from(entries.len()).expect("count"));
        out.extend_from_slice(&directory);
        out.extend_from_slice(&payload);
        out
    }

    #[test]
    fn package_preview_png_decodes() {
        let dir = scratch("pkg");
        let png = png_1x1();
        let project = br#"{"preview":"preview.png"}"#;
        let bytes = pkg(&[("project.json", project), ("preview.png", &png)]);
        let path = dir.join("wall.pkg");
        fs::write(&path, bytes).expect("write pkg");
        let texture = load_source(&WallpaperSource::Package { path }).expect("load");
        assert_eq!(texture.width, 1);
        assert_eq!(texture.height, 1);
        assert_eq!(&texture.rgba[..4], &[255, 0, 0, 255]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scene_directory_tex_decodes() {
        let dir = scratch("scene");
        let materials = dir.join("materials");
        fs::create_dir_all(&materials).expect("materials");
        fs::write(materials.join("color.tex"), tex_1x1()).expect("tex");
        let texture =
            load_source(&WallpaperSource::SceneDirectory { path: dir.clone() }).expect("load");
        assert_eq!((texture.width, texture.height), (1, 1));
        assert_eq!(&texture.rgba[..4], &[1, 2, 3, 255]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_scene_directory_compiles_layers() {
        let dir = scratch("scene-graph");
        let scene = r#"{
            "camera": {"eye": "0 0 1", "center": "0 0 0", "up": "0 1 0"},
            "general": {},
            "objects": [
                {"id": 1, "name": "sprite", "image": "dot.png", "origin": "0 0 0"},
                {"id": 2, "name": "motes", "particle": "motes.json"}
            ]
        }"#;
        fs::write(dir.join("scene.json"), scene).expect("scene");
        fs::write(dir.join("dot.png"), png_1x1()).expect("png");
        fs::write(dir.join("motes.json"), br#"{"rate": 5, "lifetime": 1}"#).expect("particle");
        let package = prepare_scene(&WallpaperSource::SceneDirectory { path: dir.clone() })
            .expect("prepare")
            .expect("scene");
        assert_eq!(package.layers.len(), 2);
        assert_eq!(package.images.len(), 1);
        assert_eq!(package.emitters.len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_video_names_the_path() {
        let path = PathBuf::from("/tmp/iris-clip-missing-phase4.mp4");
        let err = match load_wallpaper(
            &WallpaperSource::Video { path: path.clone() },
            iris_video::PlaybackOptions {
                muted: true,
                ..iris_video::PlaybackOptions::default()
            },
        ) {
            Err(err) => err,
            Ok(_) => panic!("missing video opened"),
        };
        let text = err.to_string();
        assert!(text.contains(&path.display().to_string()), "{text}");
        assert!(!matches!(err, Error::Unsupported(_)));
    }
}
