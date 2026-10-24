//! Failures while classifying a wallpaper or reading an asset.

/// A web-runtime failure that does not include GTK or WebView2 types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem call failed.
    #[error("io: {0}")]
    Io(String),
    /// `project.json` or the package could not be read.
    #[error(transparent)]
    Pkg(#[from] iris_pkg::Error),
    /// The wallpaper could not be classified or opened.
    #[error("web: {0}")]
    Message(String),
}
