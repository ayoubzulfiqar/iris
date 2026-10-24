//! Decide whether a directory or package is a web wallpaper.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use iris_pkg::{Package, Project};
use serde_json::Value;

use crate::bridge::{general_block, user_properties};
use crate::error::Error;
use crate::protocol::{entry_url, AssetRoot};

const MAX_PROJECT_BYTES: u64 = 8 * 1024 * 1024;

/// A wallpaper the webview can open.
#[derive(Clone)]
pub struct WebProject {
    /// `iris://wallpaper/...` document URL.
    pub entry_url: String,
    /// Slider, color, and checkbox values from `project.json`.
    pub properties: Value,
    /// The `general` object, or null.
    pub general: Value,
    /// Directory or package the protocol reads.
    pub root: Arc<AssetRoot>,
}

/// `Some` when `path` is a web project or a directory whose document is `index.html`.
///
/// A scene project stays `None` so the GPU loader keeps it. Image and video
/// files are `None`.
pub fn detect(path: &Path) -> Result<Option<WebProject>, Error> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(Error::Io(format!("{}: {err}", path.display())));
        }
        Err(err) => return Err(Error::Io(format!("{}: {err}", path.display()))),
    };
    if meta.file_type().is_file() {
        return detect_package(path);
    }
    if meta.file_type().is_dir() {
        return detect_directory(path);
    }
    Ok(None)
}

fn detect_package(path: &Path) -> Result<Option<WebProject>, Error> {
    let package = Package::open(path)?;
    let Some(project) = Project::from_package(&package)? else {
        return Ok(None);
    };
    if project.wallpaper_type.as_deref() != Some("web") {
        return Ok(None);
    }
    Ok(Some(WebProject {
        entry_url: entry_url(project.file.as_deref()),
        properties: user_properties(&project),
        general: general_block(&project),
        root: Arc::new(AssetRoot::Archive(package)),
    }))
}

fn detect_directory(dir: &Path) -> Result<Option<WebProject>, Error> {
    if let Some(bytes) = read_file(&dir.join("project.json"))? {
        let project = Project::parse(&bytes)?;
        if project.wallpaper_type.as_deref() == Some("web") {
            return Ok(Some(finish_dir(dir, &project)?));
        }
        if project.wallpaper_type.is_some() {
            return Ok(None);
        }
    }
    if dir.join("scene.json").is_file() {
        return Ok(None);
    }
    if dir.join("index.html").is_file() {
        let root = AssetRoot::directory(dir)?;
        return Ok(Some(WebProject {
            entry_url: entry_url(Some("index.html")),
            properties: serde_json::json!({}),
            general: Value::Null,
            root: Arc::new(root),
        }));
    }
    Ok(None)
}

fn finish_dir(dir: &Path, project: &Project) -> Result<WebProject, Error> {
    let root = AssetRoot::directory(dir)?;
    Ok(WebProject {
        entry_url: entry_url(project.file.as_deref()),
        properties: user_properties(project),
        general: general_block(project),
        root: Arc::new(root),
    })
}

fn read_file(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::Io(format!("{}: {err}", path.display()))),
    };
    if !meta.file_type().is_file() {
        return Ok(None);
    }
    if meta.len() > MAX_PROJECT_BYTES {
        return Err(Error::Message(format!("{} exceeds 8 MiB", path.display())));
    }
    fs::read(path)
        .map(Some)
        .map_err(|err| Error::Io(format!("{}: {err}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iris-web-detect-{name}-{}-{}",
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
    fn type_web_wins_and_a_scene_stays_on_the_gpu_path() {
        let web = scratch("web");
        fs::write(
            web.join("project.json"),
            br#"{"type":"web","file":"index.html","general":{"properties":{"hue":{"type":"color","value":"0 0 1"}}}}"#,
        )
        .expect("project");
        fs::write(web.join("index.html"), b"<html></html>").expect("html");
        let found = detect(&web).expect("detect").expect("web");
        assert_eq!(found.entry_url, "iris://wallpaper/index.html");
        assert!(found.properties.get("hue").is_some());

        let scene = scratch("scene");
        fs::write(
            scene.join("project.json"),
            br#"{"type":"scene","file":"scene.json"}"#,
        )
        .expect("project");
        fs::write(scene.join("index.html"), b"<html></html>").expect("html");
        fs::write(scene.join("scene.json"), b"{}").expect("scene");
        assert!(detect(&scene).expect("scene").is_none());

        let bare = scratch("bare");
        fs::write(bare.join("index.html"), b"<html></html>").expect("html");
        let bare_project = detect(&bare).expect("bare").expect("index");
        assert_eq!(bare_project.entry_url, "iris://wallpaper/index.html");

        let still = scratch("still");
        fs::write(still.join("preview.png"), b"png").expect("png");
        assert!(detect(&still).expect("still").is_none());

        let _ = fs::remove_dir_all(&web);
        let _ = fs::remove_dir_all(&scene);
        let _ = fs::remove_dir_all(&bare);
        let _ = fs::remove_dir_all(&still);
    }
}
