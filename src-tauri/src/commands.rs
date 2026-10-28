//! Tauri commands. IPC calls stay async. Package inspection reads the filesystem
//! on Tauri's blocking pool and returns a small preview, not the wallpaper bytes.

use std::fs;
use std::path::{Path, PathBuf};

use iris_core::{
    MonitorInfo, MonitorTarget, TelemetryReport, WallpaperConfig, WallpaperSource, WallpaperStatus,
};
use iris_pkg::{ImageKind, MipContent, Package, Project, Scene, Texture};
use serde::Serialize;
use tauri::State;

use crate::client::DaemonClient;
use crate::supervisor;

const THUMB_EDGE: u32 = 256;
const THUMB_LIMIT: u64 = 32 * 1024 * 1024;
const DOC_LIMIT: u64 = 1024 * 1024;
const LIBRARY_CAP: usize = 400;

/// Daemon status row for the displays grid.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatus {
    /// False while the daemon is shutting down.
    pub running: bool,
    /// One row per assigned wallpaper.
    pub wallpapers: Vec<WallpaperStatus>,
}

/// Metadata and a small PNG the inspector can show.
#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    /// Path the user selected.
    pub path: String,
    /// `package`, `scene`, `image`, `video`, or `directory`.
    pub kind: String,
    /// Title from `project.json`, or the file name.
    pub title: String,
    /// Description from `project.json`.
    pub description: String,
    /// `data:image/png;base64,...` when a preview decoded.
    pub preview_data_url: Option<String>,
}

/// One row in the library browser.
#[derive(Debug, Clone, Serialize)]
pub struct LibraryEntry {
    /// Absolute or user-supplied path.
    pub path: String,
    /// File or folder name.
    pub name: String,
    /// `package`, `scene`, `image`, `video`, or `directory`.
    pub kind: String,
}

/// A directory listing for the library view.
#[derive(Debug, Clone, Serialize)]
pub struct LibraryListing {
    /// Directory that was read.
    pub path: String,
    /// Parent directory, when it is distinct.
    pub parent: Option<String>,
    /// Wallpaper-like children.
    pub entries: Vec<LibraryEntry>,
}

fn owned(state: &State<'_, DaemonClient>) -> DaemonClient {
    state.inner().clone()
}

/// Move the parallax pointer, in output pixels.
#[tauri::command]
pub async fn set_pointer_command(
    client: State<'_, DaemonClient>,
    x: f32,
    y: f32,
) -> Result<(), String> {
    owned(&client)
        .set_pointer(x, y)
        .await
        .map_err(|err| err.to_string())
}

/// CPU, memory, network, and battery, as the daemon last sampled them.
#[tauri::command]
pub async fn get_telemetry_command(
    client: State<'_, DaemonClient>,
) -> Result<TelemetryReport, String> {
    owned(&client)
        .get_telemetry()
        .await
        .map_err(|err| err.to_string())
}

/// Connected displays, as the daemon reports them.
#[tauri::command]
pub async fn get_system_monitors(
    client: State<'_, DaemonClient>,
) -> Result<Vec<MonitorInfo>, String> {
    owned(&client)
        .list_monitors()
        .await
        .map_err(|err| err.to_string())
}

/// Running flag and the wallpapers assigned to outputs.
#[tauri::command]
pub async fn get_wallpaper_status(client: State<'_, DaemonClient>) -> Result<DaemonStatus, String> {
    let (running, wallpapers) = owned(&client)
        .get_status()
        .await
        .map_err(|err| err.to_string())?;
    Ok(DaemonStatus {
        running,
        wallpapers,
    })
}

/// Send an apply request. `target` is written onto `config` before the call.
#[tauri::command]
pub async fn apply_wallpaper_command(
    client: State<'_, DaemonClient>,
    target: MonitorTarget,
    config: WallpaperConfig,
) -> Result<(), String> {
    let mut config = config;
    config.source = absolute_source(config.source);
    owned(&client)
        .apply_wallpaper(target, config)
        .await
        .map_err(|err| err.to_string())
}

/// Remove a wallpaper from one target or from every output.
#[tauri::command]
pub async fn clear_wallpaper_command(
    client: State<'_, DaemonClient>,
    target: MonitorTarget,
) -> Result<(), String> {
    owned(&client)
        .clear_wallpaper(target)
        .await
        .map_err(|err| err.to_string())
}

/// Pause or resume one target or every output.
#[tauri::command]
pub async fn set_paused_command(
    client: State<'_, DaemonClient>,
    target: MonitorTarget,
    paused: bool,
) -> Result<(), String> {
    let client = owned(&client);
    let result = if paused {
        client.pause(target).await
    } else {
        client.resume(target).await
    };
    result.map_err(|err| err.to_string())
}

/// Read a `.pkg`, scene folder, or image and return metadata plus a thumbnail.
///
/// Archive bytes stay in this process. `Package::open` is the package reader.
#[tauri::command]
pub fn inspect_wallpaper_package(path: String) -> Result<Inspection, String> {
    inspect_path(Path::new(&path))
}

/// List wallpaper files and scene folders in one directory.
#[tauri::command]
pub fn list_library(path: String) -> Result<LibraryListing, String> {
    list_path(Path::new(&path))
}

/// One small preview for a library card. Missing when the file is not an image or is too large.
#[derive(Debug, Clone, Serialize)]
pub struct LibraryThumb {
    /// Absolute path that was requested.
    pub path: String,
    /// PNG data URL at most 256 px on the long edge.
    pub preview_data_url: Option<String>,
}

/// Decode library images one at a time into small previews.
///
/// Full-size wallpapers are not sent to the window. Files over 24 MiB, and
/// decodes that would allocate more than 32 MiB, stay as a plain card.
#[tauri::command]
pub fn library_thumbnails(paths: Vec<String>) -> Vec<LibraryThumb> {
    const BATCH: usize = 48;
    const FILE_CAP: u64 = 24 * 1024 * 1024;
    paths
        .into_iter()
        .take(BATCH)
        .map(|path| {
            let preview = thumb_for(Path::new(&path), FILE_CAP);
            LibraryThumb {
                path,
                preview_data_url: preview,
            }
        })
        .collect()
}

fn thumb_for(path: &Path, file_cap: u64) -> Option<String> {
    if file_kind(path) != Some("image") {
        return None;
    }
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > file_cap {
        return None;
    }
    let image = decode_capped(path)?;
    encode_data_url(&image)
}

fn decode_capped(path: &Path) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::open(path).ok()?;
    reader = reader.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

/// Folder the library should open when the user has not chosen one.
#[tauri::command]
pub fn default_library_path() -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return String::new();
    };
    for candidate in [
        home.join("Wallpaper"),
        home.join("Pictures"),
        home.join("Pictures").join("Iris"),
        home,
    ] {
        if candidate.is_dir() {
            return candidate.display().to_string();
        }
    }
    String::new()
}

/// Ping the daemon and spawn it when the socket is down.
#[tauri::command]
pub async fn ensure_daemon(client: State<'_, DaemonClient>) -> Result<(), String> {
    supervisor::ensure_running(&owned(&client))
        .await
        .map_err(|err| err.to_string())
}

fn absolute_source(source: WallpaperSource) -> WallpaperSource {
    match source {
        WallpaperSource::Package { path } => WallpaperSource::Package {
            path: canonical(path),
        },
        WallpaperSource::Video { path } => WallpaperSource::Video {
            path: canonical(path),
        },
        WallpaperSource::Image { path } => WallpaperSource::Image {
            path: canonical(path),
        },
        WallpaperSource::SceneDirectory { path } => WallpaperSource::SceneDirectory {
            path: canonical(path),
        },
        other => other,
    }
}

fn canonical(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

pub(crate) fn inspect_path(path: &Path) -> Result<Inspection, String> {
    if path.as_os_str().is_empty() {
        return Err("path is empty".to_owned());
    }
    let meta = fs::symlink_metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{} is a symlink", path.display()));
    }
    if meta.is_dir() {
        return inspect_directory(path);
    }
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    match extension(path).as_deref() {
        Some("mp4" | "webm" | "mkv" | "mov" | "m4v" | "avi") => Ok(video_inspection(path)),
        Some("pkg") => inspect_package(path),
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp") => inspect_image(path, meta.len()),
        _ => Err(format!(
            "{} is not a .pkg, scene folder, or image",
            path.display()
        )),
    }
}

fn inspect_directory(path: &Path) -> Result<Inspection, String> {
    let Some(project_path) = find_project_file(path)? else {
        return Ok(Inspection {
            path: path.display().to_string(),
            kind: "directory".to_owned(),
            title: file_title(path),
            description: "Folder".to_owned(),
            preview_data_url: None,
        });
    };
    let root = project_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| path.to_path_buf());
    let bytes = read_limited(&project_path, DOC_LIMIT)?;
    let project = Project::parse(&bytes).map_err(|err| err.to_string())?;
    let preview = project
        .preview
        .as_deref()
        .and_then(|relative| preview_file(&root, relative));
    Ok(Inspection {
        path: path.display().to_string(),
        kind: "scene".to_owned(),
        title: project.title.clone().unwrap_or_else(|| file_title(path)),
        description: clip(project.description.as_deref().unwrap_or("")),
        preview_data_url: preview,
    })
}

fn inspect_package(path: &Path) -> Result<Inspection, String> {
    let package = Package::open(path).map_err(|err| err.to_string())?;
    let project = Project::from_package(&package).map_err(|err| err.to_string())?;
    let title = project
        .as_ref()
        .and_then(|item| item.title.clone())
        .unwrap_or_else(|| file_title(path));
    let description = project
        .as_ref()
        .and_then(|item| item.description.clone())
        .unwrap_or_default();
    let preview = thumbnail_from_package(&package, project.as_ref());
    Ok(Inspection {
        path: path.display().to_string(),
        kind: "package".to_owned(),
        title,
        description: clip(&description),
        preview_data_url: preview,
    })
}

fn inspect_image(path: &Path, len: u64) -> Result<Inspection, String> {
    let preview = if len > THUMB_LIMIT {
        None
    } else {
        decode_file(path).and_then(|image| encode_data_url(&image))
    };
    if preview.is_none() && len <= THUMB_LIMIT {
        return Err(format!("{} could not be decoded", path.display()));
    }
    let description = if len > THUMB_LIMIT {
        "The image is larger than 32 MiB, so the inspector skipped the thumbnail."
    } else {
        "Image"
    };
    Ok(Inspection {
        path: path.display().to_string(),
        kind: "image".to_owned(),
        title: file_title(path),
        description: description.to_owned(),
        preview_data_url: preview,
    })
}

fn video_inspection(path: &Path) -> Inspection {
    Inspection {
        path: path.display().to_string(),
        kind: "video".to_owned(),
        title: file_title(path),
        description: "Plays in the daemon. The library preview has no thumbnail.".to_owned(),
        preview_data_url: None,
    }
}

fn thumbnail_from_package(package: &Package, project: Option<&Project>) -> Option<String> {
    if let Some(project) = project {
        if let Some(preview) = project.preview.as_deref() {
            if let Some(url) = package_entry_thumbnail(package, preview) {
                return Some(url);
            }
        }
    }
    if let Ok(Some(scene)) = Scene::from_package(package) {
        for object in scene.objects.iter().take(8) {
            if let Some(image) = object.image.as_deref() {
                if let Some(url) = package_entry_thumbnail(package, image) {
                    return Some(url);
                }
            }
        }
    }
    for name in [
        "preview.png",
        "preview.jpg",
        "preview.jpeg",
        "preview.gif",
        "preview.bmp",
    ] {
        if let Some(url) = package_entry_thumbnail(package, name) {
            return Some(url);
        }
    }
    None
}

fn package_entry_thumbnail(package: &Package, path: &str) -> Option<String> {
    let bytes = lookup_entry(package, path)?;
    if bytes.len() as u64 > THUMB_LIMIT {
        return None;
    }
    let image = decode_preview_bytes(path, bytes)?;
    encode_data_url(&image)
}

fn lookup_entry<'a>(package: &'a Package, path: &str) -> Option<&'a [u8]> {
    if let Some(bytes) = package.get(path) {
        return Some(bytes);
    }
    let suffix = path.replace('\\', "/");
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() || suffix.contains("..") {
        return None;
    }
    package
        .entries()
        .iter()
        .find(|entry| entry.path == suffix || entry.path.ends_with(&format!("/{suffix}")))
        .map(|entry| entry.bytes.as_slice())
}

fn preview_file(root: &Path, relative: &str) -> Option<String> {
    let relative = safe_relative(relative)?;
    let path = root.join(&relative);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.file_type().is_file() || meta.len() > THUMB_LIMIT {
        return None;
    }
    let image = decode_file(&path).or_else(|| {
        let bytes = fs::read(&path).ok()?;
        decode_preview_bytes(&relative, &bytes)
    })?;
    encode_data_url(&image)
}

fn decode_file(path: &Path) -> Option<image::DynamicImage> {
    let reader = image::ImageReader::open(path).ok()?;
    reader.with_guessed_format().ok()?.decode().ok()
}

fn decode_preview_bytes(name: &str, bytes: &[u8]) -> Option<image::DynamicImage> {
    if bytes.len() as u64 > THUMB_LIMIT {
        return None;
    }
    let extension = name.rsplit('.').next().map(|ext| ext.to_ascii_lowercase());
    if extension.as_deref() == Some("tex") || bytes.starts_with(b"TEXV") {
        decode_tex(bytes)
    } else {
        image::load_from_memory(bytes).ok()
    }
}

fn decode_tex(bytes: &[u8]) -> Option<image::DynamicImage> {
    let texture = Texture::parse(bytes).ok()?;
    match texture.primary().ok()? {
        MipContent::Image(rgba) => {
            if rgba.width == 0 || rgba.height == 0 || rgba.width > 8192 || rgba.height > 8192 {
                return None;
            }
            let buffer = image::RgbaImage::from_raw(rgba.width, rgba.height, rgba.pixels)?;
            Some(image::DynamicImage::ImageRgba8(buffer))
        }
        MipContent::Encoded { kind, bytes } => match kind {
            ImageKind::Png | ImageKind::Jpeg | ImageKind::Gif | ImageKind::Bmp => {
                image::load_from_memory(bytes).ok()
            }
            ImageKind::Tiff | ImageKind::Mp4 | ImageKind::Unknown | ImageKind::Other(_) => None,
        },
        MipContent::Video { .. } => None,
    }
}

fn encode_data_url(image: &image::DynamicImage) -> Option<String> {
    if image.width() == 0 || image.height() == 0 || image.width() > 8192 || image.height() > 8192 {
        return None;
    }
    let (width, height) = fitted(image.width(), image.height(), THUMB_EDGE);
    let scaled = if width == image.width() && height == image.height() {
        None
    } else {
        Some(image.resize(width, height, image::imageops::FilterType::Triangle))
    };
    let image = scaled.as_ref().unwrap_or(image);
    let mut cursor = std::io::Cursor::new(Vec::new());
    image.write_to(&mut cursor, image::ImageFormat::Png).ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64_encode(cursor.get_ref())
    ))
}

fn fitted(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let longest = width.max(height);
    if longest <= edge {
        return (width, height);
    }
    let width = u32::try_from(u64::from(width) * u64::from(edge) / u64::from(longest))
        .unwrap_or(1)
        .max(1);
    let height = u32::try_from(u64::from(height) * u64::from(edge) / u64::from(longest))
        .unwrap_or(1)
        .max(1);
    (width, height)
}

fn find_project_file(dir: &Path) -> Result<Option<PathBuf>, String> {
    let root = dir.join("project.json");
    if is_regular_file(&root) {
        return Ok(Some(root));
    }
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let file_type = entry.file_type().map_err(|err| err.to_string())?;
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let candidate = entry.path().join("project.json");
        if is_regular_file(&candidate) {
            found.push(candidate);
        }
    }
    found.sort();
    Ok(found.into_iter().next())
}

fn list_path(path: &Path) -> Result<LibraryListing, String> {
    if path.as_os_str().is_empty() {
        return Err("path is empty".to_owned());
    }
    let meta = fs::symlink_metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{} is a symlink", path.display()));
    }
    if meta.is_file() {
        let kind = file_kind(path);
        return Ok(LibraryListing {
            path: path.display().to_string(),
            parent: parent_dir(path),
            entries: match kind {
                Some(kind) => vec![LibraryEntry {
                    path: path.display().to_string(),
                    name: file_name(path),
                    kind: kind.to_owned(),
                }],
                None => Vec::new(),
            },
        });
    }
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    let mut entries = Vec::new();
    let read = fs::read_dir(path).map_err(|err| format!("{}: {err}", path.display()))?;
    for entry in read {
        if entries.len() >= LIBRARY_CAP {
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let child = entry.path();
        let kind = if file_type.is_dir() {
            if is_regular_file(&child.join("project.json")) {
                "scene"
            } else {
                "directory"
            }
        } else if file_type.is_file() {
            match file_kind(&child) {
                Some(kind) => kind,
                None => continue,
            }
        } else {
            continue;
        };
        entries.push(LibraryEntry {
            path: child.display().to_string(),
            name: name.to_owned(),
            kind: kind.to_owned(),
        });
    }
    entries.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    Ok(LibraryListing {
        path: path.display().to_string(),
        parent: parent_dir(path),
        entries,
    })
}

fn file_kind(path: &Path) -> Option<&'static str> {
    match extension(path).as_deref() {
        Some("pkg") => Some("package"),
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp") => Some("image"),
        Some("mp4" | "webm" | "mkv" | "mov" | "m4v" | "avi") => Some("video"),
        _ => None,
    }
}

fn parent_dir(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    if parent.as_os_str().is_empty() || parent == path {
        None
    } else {
        Some(parent.display().to_string())
    }
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_file())
        .unwrap_or(false)
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if !meta.file_type().is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if meta.len() > limit {
        return Err(format!("{} exceeds {limit} bytes", path.display()));
    }
    fs::read(path).map_err(|err| format!("{}: {err}", path.display()))
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Wallpaper")
        .to_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("wallpaper")
        .to_owned()
}

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

fn clip(text: &str) -> String {
    let mut chars = text.chars();
    let clipped: String = chars.by_ref().take(2000).collect();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let chunk = (u32::from(bytes[index]) << 16)
            | (u32::from(bytes[index + 1]) << 8)
            | u32::from(bytes[index + 2]);
        out.push(TABLE[((chunk >> 18) & 63) as usize] as char);
        out.push(TABLE[((chunk >> 12) & 63) as usize] as char);
        out.push(TABLE[((chunk >> 6) & 63) as usize] as char);
        out.push(TABLE[(chunk & 63) as usize] as char);
        index += 3;
    }
    let rest = bytes.len() - index;
    if rest == 1 {
        let chunk = u32::from(bytes[index]) << 16;
        out.push(TABLE[((chunk >> 18) & 63) as usize] as char);
        out.push(TABLE[((chunk >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rest == 2 {
        let chunk = (u32::from(bytes[index]) << 16) | (u32::from(bytes[index + 1]) << 8);
        out.push(TABLE[((chunk >> 18) & 63) as usize] as char);
        out.push(TABLE[((chunk >> 12) & 63) as usize] as char);
        out.push(TABLE[((chunk >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = PathBuf::from(format!(
            "/tmp/iris-ui-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("scratch dir");
        path
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hi"), "aGk=");
    }

    #[test]
    fn inspect_png_returns_a_preview() {
        let dir = scratch("png");
        let path = dir.join("dot.png");
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        image.save(&path).expect("write png");
        let inspection = inspect_path(&path).expect("inspect");
        assert_eq!(inspection.kind, "image");
        assert_eq!(inspection.title, "dot");
        let preview = inspection.preview_data_url.expect("preview");
        assert!(preview.starts_with("data:image/png;base64,"), "{preview}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_scene_directory_reads_project_json() {
        let dir = scratch("scene");
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]));
        image.save(dir.join("preview.png")).expect("preview");
        fs::write(
            dir.join("project.json"),
            r#"{"title":"Night","description":"A dark shore","preview":"preview.png"}"#,
        )
        .expect("project");
        let inspection = inspect_path(&dir).expect("inspect");
        assert_eq!(inspection.kind, "scene");
        assert_eq!(inspection.title, "Night");
        assert_eq!(inspection.description, "A dark shore");
        assert!(inspection.preview_data_url.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_project_json_is_an_error() {
        let dir = scratch("bad-scene");
        fs::write(dir.join("project.json"), r#"{"title":1}"#).expect("project");
        let error = inspect_path(&dir).expect_err("bad title");
        assert!(error.contains("title"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn video_file_is_named_without_a_thumbnail() {
        let dir = scratch("video");
        let path = dir.join("clip.mp4");
        fs::write(&path, b"not a video").expect("video");
        let inspection = inspect_path(&path).expect("inspect");
        assert_eq!(inspection.kind, "video");
        assert!(inspection.preview_data_url.is_none());
        assert!(inspection.description.contains("daemon"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_lists_packages_images_and_scene_folders() {
        let dir = scratch("lib");
        fs::write(dir.join("shore.pkg"), b"pkg").expect("pkg");
        fs::write(dir.join("notes.txt"), b"ignore").expect("txt");
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([9, 9, 9, 255]));
        image.save(dir.join("still.png")).expect("png");
        let scene = dir.join("Night");
        fs::create_dir(&scene).expect("scene");
        fs::write(scene.join("project.json"), r#"{"title":"Night"}"#).expect("project");
        let listing = list_path(&dir).expect("list");
        let kinds: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.kind.as_str()))
            .collect();
        assert!(kinds.contains(&("shore.pkg", "package")), "{kinds:?}");
        assert!(kinds.contains(&("still.png", "image")), "{kinds:?}");
        assert!(kinds.contains(&("Night", "scene")), "{kinds:?}");
        assert!(!kinds.iter().any(|(name, _)| *name == "notes.txt"));
        let _ = fs::remove_dir_all(&dir);
    }
}
