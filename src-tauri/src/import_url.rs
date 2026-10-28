//! Download a direct image or video URL into the local library.
//!
//! Only `http` and `https` are accepted. The response must be a known media
//! type, and the body is capped at 512 MiB. HTML pages are rejected so a
//! website link has to be the file itself, not a gallery page.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// A file saved from a remote URL.
#[derive(Debug, Clone, Serialize)]
pub struct ImportedMedia {
    /// Absolute path of the saved file.
    pub path: String,
    /// `image` or `video`.
    pub kind: String,
    /// Bytes written.
    pub bytes: u64,
}

/// Fetch `url` and store it under `Wallpaper/Imports` or `Pictures/Iris`.
#[tauri::command]
pub async fn import_media_url(url: String) -> Result<ImportedMedia, String> {
    let start = validate_url(&url)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(Duration::from_secs(180))
        .user_agent("Iris/0.1")
        .build()
        .map_err(|err| format!("download: {err}"))?;
    let response = client
        .get(start)
        .send()
        .await
        .map_err(|err| format!("download: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("download: HTTP {}", response.status()));
    }
    let final_url = response.url().clone();
    validate_url(final_url.as_str())?;
    if let Some(length) = response.content_length() {
        if length > MAX_BYTES {
            return Err("download: file is larger than 512 MiB".to_owned());
        }
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let extension = media_extension(final_url.path(), content_type.as_deref())?;
    let kind = if is_video(extension) {
        "video"
    } else {
        "image"
    };
    let directory = import_dir()?;
    fs::create_dir_all(&directory).map_err(|err| format!("{}: {err}", directory.display()))?;
    let stem = stem_from(final_url.path());
    let destination = unique_path(&directory, &stem, extension);
    let mut file =
        File::create(&destination).map_err(|err| format!("{}: {err}", destination.display()))?;
    let mut total = 0u64;
    let mut response = response;
    loop {
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| format!("download: {err}"))?
        else {
            break;
        };
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_BYTES {
            drop(file);
            let _ = fs::remove_file(&destination);
            return Err("download: file is larger than 512 MiB".to_owned());
        }
        file.write_all(&chunk)
            .map_err(|err| format!("{}: {err}", destination.display()))?;
    }
    if total == 0 {
        let _ = fs::remove_file(&destination);
        return Err("download: the server returned an empty file".to_owned());
    }
    Ok(ImportedMedia {
        path: destination.display().to_string(),
        kind: kind.to_owned(),
        bytes: total,
    })
}

fn validate_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|err| format!("url: {err}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("url: only http and https links can be downloaded".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("url: links with a username or password are rejected".to_owned());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "url: the link has no host".to_owned())?;
    let lowered = host.to_ascii_lowercase();
    if lowered == "localhost" || lowered == "127.0.0.1" || lowered == "0.0.0.0" || lowered == "::1"
    {
        return Err("url: local addresses are rejected".to_owned());
    }
    Ok(url)
}

fn media_extension(path: &str, content_type: Option<&str>) -> Result<&'static str, String> {
    if let Some(extension) = path_extension(path) {
        return Ok(extension);
    }
    let mime = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let extension = match mime.as_str() {
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "video/x-matroska" => "mkv",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => {
            return Err(
                "url: the link is not a direct mp4, webm, mov, mkv, png, jpeg, gif, webp, or bmp file"
                    .to_owned(),
            );
        }
    };
    Ok(extension)
}

fn path_extension(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "mp4" => "mp4",
        "webm" => "webm",
        "mkv" => "mkv",
        "mov" => "mov",
        "m4v" => "m4v",
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        _ => return None,
    })
}

fn is_video(extension: &str) -> bool {
    matches!(extension, "mp4" | "webm" | "mkv" | "mov" | "m4v")
}

fn stem_from(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("wallpaper");
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    let mut cleaned = String::new();
    for ch in stem.chars() {
        if cleaned.len() >= 80 {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            cleaned.push(ch);
        } else if ch == ' ' && !cleaned.ends_with('-') {
            cleaned.push('-');
        }
    }
    let cleaned = cleaned.trim_matches('-').to_owned();
    if cleaned.is_empty() {
        "wallpaper".to_owned()
    } else {
        cleaned
    }
}

fn unique_path(directory: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = directory.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    for index in 2..10_000 {
        let candidate = directory.join(format!("{stem}-{index}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{stem}-{}.{extension}", std::process::id()))
}

fn import_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "download: HOME is not set".to_owned())?;
    let wallpaper = home.join("Wallpaper");
    if wallpaper.is_dir() {
        return Ok(wallpaper.join("Imports"));
    }
    Ok(home.join("Pictures").join("Iris"))
}
