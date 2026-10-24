//! `iris://` asset map.
//!
//! WebKit and WebView2 block `file://` ES modules and WebGL loads. The webview
//! asks for `iris://wallpaper/...` and this module reads that path from the
//! wallpaper directory or package. `..`, absolute paths, and symlinks that
//! leave the root are rejected before any bytes are returned.

use std::borrow::Cow;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_TYPE};
use http::{Response, StatusCode};
use iris_pkg::Package;

use crate::error::Error;

/// Custom scheme registered with wry.
pub const SCHEME: &str = "iris";

/// Document the webview loads when `project.json` does not name another file.
pub const ENTRY_URL: &str = "iris://wallpaper/index.html";

const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;

/// Where wallpaper bytes live.
pub enum AssetRoot {
    /// Unpacked wallpaper directory. The path is canonical.
    Directory(PathBuf),
    /// `.pkg` archive already parsed into memory.
    Archive(Package),
}

/// One file the protocol is willing to return.
struct LoadedFile {
    mime: &'static str,
    bytes: Vec<u8>,
}

enum Miss {
    Traversal,
    Missing,
    Io(String),
}

impl AssetRoot {
    /// Canonicalize `path` and keep it as the only directory the protocol reads.
    pub fn directory(path: &Path) -> Result<Self, Error> {
        let root = fs::canonicalize(path)
            .map_err(|err| Error::Io(format!("{}: {err}", path.display())))?;
        if !root.is_dir() {
            return Err(Error::Message(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        Ok(Self::Directory(root))
    }

    /// Answer one webview request.
    ///
    /// Traversal is 403. A missing file is 404. The body is empty only when the
    /// response builder itself fails, and that response is 500.
    pub fn respond(&self, uri: &str) -> Response<Cow<'static, [u8]>> {
        match self.lookup(uri) {
            Ok(file) => response(StatusCode::OK, file.mime, file.bytes),
            Err(Miss::Traversal) => {
                response(StatusCode::FORBIDDEN, "text/plain", b"forbidden".to_vec())
            }
            Err(Miss::Missing) => {
                response(StatusCode::NOT_FOUND, "text/plain", b"not found".to_vec())
            }
            Err(Miss::Io(message)) => response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "text/plain",
                message.into_bytes(),
            ),
        }
    }

    fn lookup(&self, uri: &str) -> Result<LoadedFile, Miss> {
        let relative = request_relative(uri)?;
        match self {
            Self::Directory(root) => read_inside(root, &relative),
            Self::Archive(package) => read_archive(package, &relative),
        }
    }
}

/// `iris://wallpaper/{file}` for a safe relative file, otherwise [`ENTRY_URL`].
pub fn entry_url(file: Option<&str>) -> String {
    match file.and_then(safe_file) {
        Some(relative) => format!("iris://wallpaper/{relative}"),
        None => ENTRY_URL.to_owned(),
    }
}

fn request_relative(uri: &str) -> Result<String, Miss> {
    if uri.contains('\0') {
        return Err(Miss::Traversal);
    }
    let bare = uri.split(['?', '#']).next().unwrap_or(uri);
    let (host, raw_path) = split_host_path(bare);
    if let Some(host) = host.as_deref() {
        if host != "wallpaper" && !host.eq_ignore_ascii_case("localhost") {
            return Err(Miss::Traversal);
        }
    }
    let decoded = percent_encoding::percent_decode_str(raw_path).decode_utf8_lossy();
    if decoded.contains('\0') {
        return Err(Miss::Traversal);
    }
    let normalized = decoded.replace('\\', "/");
    let mut text = normalized.trim_start_matches('/').to_owned();
    let host_is_root = matches!(host.as_deref(), Some("wallpaper") | Some("localhost"));
    if !host_is_root {
        if let Some(rest) = text.strip_prefix("wallpaper/") {
            text = rest.to_owned();
        }
    }
    if text.is_empty() {
        return Ok("index.html".to_owned());
    }
    if text.starts_with('/') || has_drive_prefix(&text) {
        return Err(Miss::Traversal);
    }
    let mut parts = Vec::new();
    for part in text.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') || part.contains('\0') {
            return Err(Miss::Traversal);
        }
        parts.push(part);
    }
    if parts.is_empty() {
        Ok("index.html".to_owned())
    } else {
        Ok(parts.join("/"))
    }
}

fn split_host_path(uri: &str) -> (Option<String>, &str) {
    if let Some(rest) = uri.strip_prefix("iris://") {
        return split_authority(rest);
    }
    if let Some(rest) = uri
        .strip_prefix("https://")
        .or_else(|| uri.strip_prefix("http://"))
    {
        return split_http(rest);
    }
    (None, uri)
}

fn split_authority(rest: &str) -> (Option<String>, &str) {
    match rest.split_once('/') {
        Some((host, path)) => (Some(host.to_owned()), path),
        None if rest.is_empty() => (None, ""),
        None => (Some(rest.to_owned()), ""),
    }
}

fn split_http(rest: &str) -> (Option<String>, &str) {
    let (host, path) = match rest.split_once('/') {
        Some(pair) => pair,
        None => (rest, ""),
    };
    if let Some(virtual_host) = host.strip_prefix("iris.") {
        (Some(virtual_host.to_owned()), path)
    } else {
        (Some(host.to_owned()), path)
    }
}

fn safe_file(raw: &str) -> Option<String> {
    match request_relative(&format!("iris://wallpaper/{raw}")) {
        Ok(relative) => Some(relative),
        Err(_) => None,
    }
}

fn has_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

fn read_inside(root: &Path, relative: &str) -> Result<LoadedFile, Miss> {
    let candidate = root.join(relative);
    let canon = match candidate.canonicalize() {
        Ok(path) => path,
        Err(err) if err.kind() == ErrorKind::NotFound => return Err(Miss::Missing),
        Err(err) => return Err(Miss::Io(err.to_string())),
    };
    if !canon.starts_with(root) {
        return Err(Miss::Traversal);
    }
    let meta = match fs::metadata(&canon) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Err(Miss::Missing),
        Err(err) => return Err(Miss::Io(err.to_string())),
    };
    if !meta.is_file() {
        return Err(Miss::Missing);
    }
    if meta.len() > MAX_ASSET_BYTES {
        return Err(Miss::Io("asset exceeds 256 MiB".to_owned()));
    }
    let bytes = fs::read(&canon).map_err(|err| Miss::Io(err.to_string()))?;
    Ok(LoadedFile {
        mime: mime_of(relative),
        bytes,
    })
}

fn read_archive(package: &Package, relative: &str) -> Result<LoadedFile, Miss> {
    let Some(bytes) = package.get(relative) else {
        return Err(Miss::Missing);
    };
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        return Err(Miss::Io("asset exceeds 256 MiB".to_owned()));
    }
    Ok(LoadedFile {
        mime: mime_of(relative),
        bytes: bytes.to_vec(),
    })
}

fn mime_of(relative: &str) -> &'static str {
    let ext = relative
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" | "md" => "text/plain",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}

fn response(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    match Response::builder()
        .status(status)
        .header(CONTENT_TYPE, mime)
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Cow::Owned(body))
    {
        Ok(response) => response,
        Err(_) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Cow::Owned(Vec::new()))
            .unwrap_or_else(|_| Response::new(Cow::Owned(Vec::new()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iris-web-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn traversal_and_mime_stay_inside_the_wallpaper_root() {
        let root = scratch("proto");
        let outside = scratch("outside");
        fs::write(outside.join("secret.txt"), b"secret").expect("secret");
        fs::write(root.join("index.html"), b"<html>ok</html>").expect("html");
        fs::write(root.join("app.js"), b"console.log(1)").expect("js");
        fs::create_dir(root.join("models")).expect("models");
        fs::write(root.join("models").join("hero.glb"), b"glb").expect("glb");
        fs::write(root.join("tex.png"), b"png").expect("png");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("leak.txt"))
                .expect("symlink");
        }
        let asset = AssetRoot::directory(&root).expect("root");

        let page = asset.respond("iris://wallpaper/index.html");
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(
            page.headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/html")
        );
        assert_eq!(page.body().as_ref(), b"<html>ok</html>");
        assert_eq!(
            page.headers()
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );

        let script = asset.respond("iris://wallpaper/app.js");
        assert_eq!(
            script
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/javascript")
        );
        let model = asset.respond("iris://wallpaper/models/hero.glb");
        assert_eq!(model.status(), StatusCode::OK);
        assert_eq!(
            model
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("model/gltf-binary")
        );
        let image = asset.respond("http://iris.wallpaper/tex.png");
        assert_eq!(image.status(), StatusCode::OK);
        assert_eq!(
            image
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/png")
        );

        assert_eq!(
            asset.respond("iris://wallpaper/../secret.txt").status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            asset.respond("iris://wallpaper/%2e%2e/secret.txt").status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            asset
                .respond("iris://wallpaper/models/%2e%2e/%2e%2e/secret.txt")
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            asset.respond("iris://evil/index.html").status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            asset.respond("iris://wallpaper/missing.html").status(),
            StatusCode::NOT_FOUND
        );
        #[cfg(unix)]
        {
            assert_eq!(
                asset.respond("iris://wallpaper/leak.txt").status(),
                StatusCode::FORBIDDEN
            );
        }

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn entry_url_rejects_a_parent_segment() {
        assert_eq!(entry_url(None), ENTRY_URL);
        assert_eq!(entry_url(Some("page.html")), "iris://wallpaper/page.html");
        assert_eq!(entry_url(Some("../etc/passwd")), ENTRY_URL);
        assert_eq!(entry_url(Some("js/app.js")), "iris://wallpaper/js/app.js");
    }
}
