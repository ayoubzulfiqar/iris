//! Desktop wallpaper for sessions without `zwlr_layer_shell_v1`.
//!
//! GNOME's compositor does not advertise layer-shell, so a GPU surface cannot
//! sit behind the icons. A still image is applied with `gsettings` on
//! `org.gnome.desktop.background`. Video and GIF are staged for the desktop
//! webview; the Iris Shell extension lowers that window behind other windows.
//! The previous background and the active wallpaper are stored under
//! `$XDG_STATE_HOME/iris`.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use iris_core::{RunState, ScaleMode, WallpaperConfig};
use serde::{Deserialize, Serialize};

use crate::error::Error;

const SCHEMA: &str = "org.gnome.desktop.background";

/// One still image currently shown as the session background.
pub(crate) struct DesktopBackground {
    pub index: u32,
    pub connector: String,
    pub primary: bool,
    pub config: WallpaperConfig,
    pub state: RunState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SavedBackground {
    picture_uri: String,
    picture_uri_dark: String,
    picture_options: String,
}

/// Play `video` on the desktop when the compositor has no layer-shell.
///
/// The file is hard-linked into the cache directory. A copy is used when the
/// wallpaper and the cache are on different filesystems.
pub(crate) fn video_project(
    video: &Path,
    config: &WallpaperConfig,
) -> Result<iris_web::WebProject, Error> {
    let clip = clip_name(video);
    stage_media(
        video,
        clip,
        &video_page(
            config.scale,
            config.playback.rate,
            config.volume,
            config.playback.muted,
            config.playback.looping,
            clip,
        ),
    )?;
    let directory = video_stage_dir();
    let root = iris_web::AssetRoot::directory(&directory)?;
    let port = video_port()?;
    Ok(iris_web::WebProject {
        entry_url: format!("http://127.0.0.1:{port}/index.html"),
        properties: serde_json::json!({}),
        general: serde_json::Value::Null,
        root: Arc::new(root),
    })
}

fn video_port() -> Result<u16, Error> {
    static PORT: OnceLock<Mutex<Option<u16>>> = OnceLock::new();
    let slot = PORT.get_or_init(|| Mutex::new(None));
    let mut guard = slot
        .lock()
        .map_err(|_| Error::Load("video server lock poisoned".to_owned()))?;
    if let Some(port) = *guard {
        return Ok(port);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| Error::Io(format!("video server: {err}")))?;
    let port = listener
        .local_addr()
        .map_err(|err| Error::Io(format!("video server: {err}")))?
        .port();
    let root = video_stage_dir();
    std::thread::Builder::new()
        .name("iris-video-http".to_owned())
        .spawn(move || accept_video(listener, root))
        .map_err(|err| Error::Io(format!("video server: {err}")))?;
    *guard = Some(port);
    Ok(port)
}

fn accept_video(listener: TcpListener, root: PathBuf) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(30)));
        let _ = serve_video(&mut stream, &root);
    }
}

fn serve_video(stream: &mut TcpStream, root: &Path) -> std::io::Result<()> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    while header.len() < 8192 {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&header);
    let mut lines = text.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return write_status(stream, 405, "text/plain", None, b"method");
    }
    let (file_name, mime) = match path {
        "/index.html" | "/" => ("index.html", "text/html; charset=utf-8"),
        "/clip.mp4" => ("clip.mp4", "video/mp4"),
        "/clip.webm" => ("clip.webm", "video/webm"),
        "/clip.gif" => ("clip.gif", "image/gif"),
        _ => return write_status(stream, 404, "text/plain", None, b"not found"),
    };
    let mut file = File::open(root.join(file_name))?;
    let len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    let range = text
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("range:"))
        .and_then(|line| line.split_once(':'))
        .map(|(_, value)| value.trim());
    if let Some(spec) = range {
        let Some((start, end)) = byte_range(spec, len) else {
            let body = b"range";
            let headers = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes())?;
            stream.write_all(body)?;
            return Ok(());
        };
        let slice = end - start + 1;
        let headers = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {mime}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {slice}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(headers.as_bytes())?;
        if method != "HEAD" {
            copy_range(stream, &mut file, start, slice)?;
        }
        return Ok(());
    }
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nAccept-Ranges: bytes\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(headers.as_bytes())?;
    if method != "HEAD" {
        copy_range(stream, &mut file, 0, len)?;
    }
    Ok(())
}

fn copy_range(
    stream: &mut TcpStream,
    file: &mut File,
    start: u64,
    len: u64,
) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(start))?;
    let mut left = len;
    let mut buffer = [0u8; 64 * 1024];
    while left > 0 {
        let want = usize::try_from(left.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = file.read(&mut buffer[..want])?;
        if read == 0 {
            break;
        }
        stream.write_all(&buffer[..read])?;
        left -= read as u64;
    }
    Ok(())
}

fn write_status(
    stream: &mut TcpStream,
    status: u16,
    mime: &str,
    extra: Option<&str>,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let extra = extra.unwrap_or("");
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

/// Inclusive byte range from a `Range` value such as `bytes=0-99`.
fn byte_range(spec: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let spec = spec.trim().strip_prefix("bytes=")?;
    let (start_text, end_text) = spec.split_once('-')?;
    let last = len - 1;
    if start_text.is_empty() {
        let suffix = end_text.parse::<u64>().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = len.saturating_sub(suffix);
        return Some((start, last));
    }
    let start = start_text.parse::<u64>().ok()?;
    if start > last {
        return None;
    }
    let end = if end_text.is_empty() {
        last
    } else {
        end_text.parse::<u64>().ok()?.min(last)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

/// Play an animated GIF in the same desktop window used for video.
pub(crate) fn gif_project(
    gif: &Path,
    config: &WallpaperConfig,
) -> Result<iris_web::WebProject, Error> {
    stage_media(gif, "clip.gif", &gif_page(config.scale))?;
    let directory = video_stage_dir();
    let root = iris_web::AssetRoot::directory(&directory)?;
    let port = video_port()?;
    Ok(iris_web::WebProject {
        entry_url: format!("http://127.0.0.1:{port}/index.html"),
        properties: serde_json::json!({}),
        general: serde_json::Value::Null,
        root: Arc::new(root),
    })
}

fn stage_media(source: &Path, name: &str, html: &str) -> Result<(), Error> {
    let source = source
        .canonicalize()
        .map_err(|err| Error::Io(format!("{}: {err}", source.display())))?;
    let meta = fs::symlink_metadata(&source)
        .map_err(|err| Error::Io(format!("{}: {err}", source.display())))?;
    if !meta.file_type().is_file() {
        return Err(Error::Load(format!("{} is not a file", source.display())));
    }
    let directory = video_stage_dir();
    fs::create_dir_all(&directory)
        .map_err(|err| Error::Io(format!("{}: {err}", directory.display())))?;
    for stale in ["clip.mp4", "clip.webm", "clip.gif"] {
        let path = directory.join(stale);
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
        }
    }
    let dest = directory.join(name);
    if fs::hard_link(&source, &dest).is_err() {
        fs::copy(&source, &dest).map_err(|err| Error::Io(format!("{}: {err}", dest.display())))?;
    }
    let page = directory.join("index.html");
    fs::write(&page, html).map_err(|err| Error::Io(format!("{}: {err}", page.display())))?;
    Ok(())
}

/// HTML document that loops an animated GIF.
pub(crate) fn gif_page(scale: ScaleMode) -> String {
    let fit = fit_css(scale);
    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>IrisWallpaper</title>
<style>
html,body{{margin:0;height:100%;background:#000;overflow:hidden}}
img{{position:fixed;inset:0;width:100vw;height:100vh;object-fit:{fit}}}
</style>
</head>
<body>
<img src="clip.gif" alt="">
</body>
</html>
"#
    )
}

/// HTML document that loops `clip.mp4` inside the wallpaper webview.
pub(crate) fn video_page(
    scale: ScaleMode,
    rate: f32,
    volume: f32,
    muted: bool,
    looping: bool,
    clip: &str,
) -> String {
    let fit = fit_css(scale);
    let rate = if rate.is_finite() && rate > 0.0 && rate <= 8.0 {
        rate
    } else {
        1.0
    };
    let volume = if muted || !volume.is_finite() {
        0.0
    } else {
        volume.clamp(0.0, 1.0)
    };
    let loop_attr = if looping { " loop" } else { "" };
    let muted_attr = if muted { " muted" } else { "" };
    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>IrisWallpaper</title>
<style>
html,body{{margin:0;height:100%;background:#000;overflow:hidden}}
video{{position:fixed;inset:0;width:100vw;height:100vh;object-fit:{fit};background:#000}}
#status{{position:fixed;left:24px;top:24px;color:#fff;font:16px sans-serif;z-index:2}}
</style>
</head>
<body>
<p id="status">Loading video…</p>
<video id="clip" src="{clip}" autoplay playsinline{loop_attr}{muted_attr}></video>
<script>
const clip=document.getElementById("clip");
const status=document.getElementById("status");
clip.playbackRate={rate:.4};
clip.volume={volume:.4};
clip.addEventListener("playing",function(){{status.remove();}});
clip.addEventListener("error",function(){{status.textContent="This video did not start";}});
function wallpaperPaused(){{return !!(window.iris&&window.iris.paused);}}
setInterval(function(){{
  if(wallpaperPaused()){{if(!clip.paused)clip.pause();}}
  else if(clip.paused&&!clip.ended){{clip.play();}}
}},400);
clip.play();
</script>
</body>
</html>
"#
    )
}

fn video_stage_dir() -> PathBuf {
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            home.join(".cache")
        }
    };
    base.join("iris").join("video-wallpaper")
}

fn fit_css(scale: ScaleMode) -> &'static str {
    match scale {
        ScaleMode::Fill => "cover",
        ScaleMode::Fit => "contain",
        ScaleMode::Stretch => "fill",
        ScaleMode::Center => "none",
    }
}

/// GNOME picture-options value for an Iris fit mode.
pub(crate) fn picture_options(scale: ScaleMode) -> &'static str {
    match scale {
        ScaleMode::Fill => "zoom",
        ScaleMode::Fit => "scaled",
        ScaleMode::Stretch => "stretched",
        ScaleMode::Center => "centered",
    }
}

/// `file://` URI for an absolute path. Bytes outside the unreserved set are percent-encoded.
pub(crate) fn file_uri(path: &Path) -> Result<String, Error> {
    let absolute = path
        .canonicalize()
        .map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
    let mut uri = String::from("file://");
    for &byte in std::os::unix::ffi::OsStrExt::as_bytes(absolute.as_os_str()) {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    Ok(uri)
}

/// Set the GNOME background to `image`. The first call remembers the previous picture.
pub(crate) fn apply_image(image: &Path, scale: ScaleMode) -> Result<(), Error> {
    remember_current()?;
    let uri = file_uri(image)?;
    let quoted = quote_gvariant(&uri);
    let options = quote_gvariant(picture_options(scale));
    gsettings_set("picture-uri", &quoted)?;
    gsettings_set("picture-uri-dark", &quoted)?;
    gsettings_set("picture-options", &options)?;
    tracing::info!("set GNOME desktop background to {uri}");
    Ok(())
}

/// Put back the background saved by [`apply_image`], if one was saved.
pub(crate) fn restore() -> Result<(), Error> {
    let path = state_path();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(Error::Io(format!("{}: {err}", path.display()))),
    };
    let saved: SavedBackground = serde_json::from_slice(&bytes)
        .map_err(|err| Error::Load(format!("saved desktop background: {err}")))?;
    gsettings_set("picture-uri", &saved.picture_uri)?;
    gsettings_set("picture-uri-dark", &saved.picture_uri_dark)?;
    gsettings_set("picture-options", &saved.picture_options)?;
    fs::remove_file(&path).map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
    tracing::info!("restored the previous GNOME desktop background");
    Ok(())
}

fn remember_current() -> Result<(), Error> {
    let path = state_path();
    if path.is_file() {
        return Ok(());
    }
    let saved = SavedBackground {
        picture_uri: gsettings_get("picture-uri")?,
        picture_uri_dark: gsettings_get("picture-uri-dark")?,
        picture_options: gsettings_get("picture-options")?,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| Error::Io(format!("{}: {err}", parent.display())))?;
    }
    let bytes = serde_json::to_vec_pretty(&saved)
        .map_err(|err| Error::Load(format!("save desktop background: {err}")))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, &bytes)
        .map_err(|err| Error::Io(format!("{}: {err}", temporary.display())))?;
    fs::rename(&temporary, &path).map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
    Ok(())
}

fn gsettings_get(key: &str) -> Result<String, Error> {
    let output = Command::new("gsettings")
        .args(["get", SCHEMA, key])
        .output()
        .map_err(|err| Error::Load(format!("gsettings get {key}: {err}")))?;
    if !output.status.success() {
        return Err(Error::Load(format!(
            "gsettings get {key}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() {
        return Err(Error::Load(format!("gsettings get {key} returned nothing")));
    }
    Ok(text)
}

fn gsettings_set(key: &str, value: &str) -> Result<(), Error> {
    let output = Command::new("gsettings")
        .args(["set", SCHEMA, key, value])
        .output()
        .map_err(|err| Error::Load(format!("gsettings set {key}: {err}")))?;
    if !output.status.success() {
        return Err(Error::Load(format!(
            "gsettings set {key}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn quote_gvariant(value: &str) -> String {
    let mut quoted = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('\'');
    quoted
}

/// The Shell extension writes this after it loads the background-group build.
const EXTENSION_STAMP: &str = "background-group-1";

/// True when this login loaded the extension that moves video into the desktop background.
pub(crate) fn shell_extension_ready() -> bool {
    let path = state_dir().join("shell-extension");
    fs::read_to_string(path)
        .map(|text| extension_stamp_ready(&text))
        .unwrap_or(false)
}

fn extension_stamp_ready(text: &str) -> bool {
    text.lines().any(|line| line.trim() == EXTENSION_STAMP)
}

fn state_dir() -> PathBuf {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            home.join(".local").join("state")
        }
    };
    base.join("iris")
}

fn state_path() -> PathBuf {
    state_dir().join("desktop-background.json")
}

fn active_path() -> PathBuf {
    state_dir().join("active-wallpaper.json")
}

fn clip_name(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("webm") => "clip.webm",
        Some("gif") => "clip.gif",
        _ => "clip.mp4",
    }
}

/// Remember the wallpaper that should come back the next time the daemon starts.
pub(crate) fn save_active(config: &WallpaperConfig) -> Result<(), Error> {
    let path = active_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| Error::Io(format!("{}: {err}", parent.display())))?;
    }
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|err| Error::Load(format!("save wallpaper: {err}")))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes)
        .map_err(|err| Error::Io(format!("{}: {err}", temporary.display())))?;
    fs::rename(&temporary, &path).map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
    Ok(())
}

/// The wallpaper saved by [`save_active`], when it is still valid.
pub(crate) fn load_active() -> Option<WallpaperConfig> {
    let path = active_path();
    let bytes = fs::read(&path).ok()?;
    let config: WallpaperConfig = match serde_json::from_slice(&bytes) {
        Ok(config) => config,
        Err(err) => {
            tracing::warn!("saved wallpaper {}: {err}", path.display());
            return None;
        }
    };
    if let Err(err) = config.validate() {
        tracing::warn!("saved wallpaper {}: {err}", path.display());
        return None;
    }
    Some(config)
}

/// Drop the saved wallpaper so the next start stays blank.
pub(crate) fn forget_active() {
    let path = active_path();
    if path.is_file() {
        if let Err(err) = fs::remove_file(&path) {
            tracing::warn!("forget wallpaper {}: {err}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_page_covers_and_names_the_clip() {
        let page = video_page(ScaleMode::Fill, 1.0, 0.5, true, true, "clip.mp4");
        assert!(page.contains("object-fit:cover"));
        assert!(page.contains("width:100vw"));
        assert!(page.contains("src=\"clip.mp4\""));
        assert!(page.contains("<title>IrisWallpaper</title>"));
        assert!(page.contains("window.iris.paused"));
        assert!(!page.contains("document.hidden"));
        assert_eq!(byte_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(byte_range("bytes=100-", 1000), Some((100, 999)));
        assert_eq!(byte_range("bytes=-10", 1000), Some((990, 999)));
        assert!(byte_range("bytes=5000-1", 1000).is_none());
        assert!(page.contains(" muted"));
        assert!(page.contains(" loop"));
        assert!(page.contains("playbackRate=1.0000"));
        let fit = video_page(ScaleMode::Fit, 1.0, 1.0, false, false, "clip.mp4");
        assert!(fit.contains("object-fit:contain"));
        assert!(!fit.contains(" muted"));
    }

    #[test]
    fn fill_maps_to_gnome_zoom() {
        assert_eq!(picture_options(ScaleMode::Fill), "zoom");
        assert_eq!(picture_options(ScaleMode::Fit), "scaled");
        assert_eq!(picture_options(ScaleMode::Stretch), "stretched");
        assert_eq!(picture_options(ScaleMode::Center), "centered");
    }

    #[test]
    fn extension_stamp_requires_the_background_group_build() {
        assert!(extension_stamp_ready("background-group-1\n"));
        assert!(!extension_stamp_ready(""));
        assert!(!extension_stamp_ready("old\n"));
    }

    #[test]
    fn file_uri_encodes_spaces() {
        let dir = std::env::temp_dir().join(format!("iris bg {}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp");
        let file = dir.join("a b.png");
        fs::write(&file, b"png").expect("file");
        let uri = file_uri(&file).expect("uri");
        assert!(uri.starts_with("file://"), "{uri}");
        assert!(uri.contains("%20"), "{uri}");
        assert!(!uri.contains(' '), "{uri}");
        let _ = fs::remove_dir_all(&dir);
    }
}
