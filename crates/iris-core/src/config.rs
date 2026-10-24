//! Daemon process configuration.
//!
//! Wallpaper assignment itself is an IPC call ([`crate::WallpaperConfig`]). This
//! module is the file the daemon reads when it starts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ipc::default_true;

/// Where the manager connects to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcEndpoint {
    /// Filesystem path of a Unix domain socket.
    Unix {
        /// Socket path. The parent directory is created by the daemon, not by this crate.
        path: PathBuf,
    },
    /// Windows named-pipe path, for example `\\.\pipe\iris-ipc-Ada`.
    NamedPipe {
        /// Pipe name, including the `\\.\pipe\` prefix.
        name: String,
    },
}

impl IpcEndpoint {
    /// Endpoint used when the daemon config does not override it.
    ///
    /// On Unix this is `$XDG_RUNTIME_DIR/iris/iris.sock` when that variable is
    /// set, otherwise `/tmp/iris-$UID.sock`. On Windows it is the named pipe
    /// `\\.\pipe\iris-ipc-{username}`.
    pub fn local_default() -> Self {
        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_owned());
            Self::NamedPipe {
                name: named_pipe_path(&user),
            }
        }
        #[cfg(not(windows))]
        {
            Self::Unix {
                path: default_unix_socket(),
            }
        }
    }

    /// Path or pipe name, for logs.
    pub fn display_name(&self) -> &Path {
        match self {
            Self::Unix { path } => path.as_path(),
            Self::NamedPipe { name } => Path::new(name),
        }
    }
}

/// Tracing filter and the fields around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogConfig {
    /// [`tracing_subscriber::EnvFilter`] directive. An empty string means `info`.
    #[serde(default = "default_filter")]
    pub filter: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            filter: default_filter(),
        }
    }
}

impl LogConfig {
    /// Directive that [`crate::logging::init`] will install.
    pub fn directive(&self) -> &str {
        normalize_filter(&self.filter)
    }
}

/// Static settings for one daemon process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// IPC listen address.
    pub endpoint: IpcEndpoint,
    /// Log filter.
    #[serde(default)]
    pub log: LogConfig,
    /// Pause wallpapers while another window is fullscreen.
    #[serde(default = "default_true")]
    pub pause_on_fullscreen: bool,
    /// Pause wallpapers while a game is detected.
    #[serde(default = "default_true")]
    pub pause_on_game: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            endpoint: IpcEndpoint::local_default(),
            log: LogConfig::default(),
            pause_on_fullscreen: true,
            pause_on_game: true,
        }
    }
}

fn default_filter() -> String {
    "info".to_owned()
}

pub(crate) fn normalize_filter(filter: &str) -> &str {
    let trimmed = filter.trim();
    if trimmed.is_empty() {
        "info"
    } else {
        trimmed
    }
}

/// Windows pipe path for an account name.
///
/// Characters outside ASCII letters, digits, `_`, and `-` are dropped so the
/// name cannot escape the `\\.\pipe\` namespace.
pub fn named_pipe_path(username: &str) -> String {
    let mut cleaned = String::new();
    for ch in username.chars() {
        if cleaned.len() >= 64 {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            cleaned.push(ch);
        }
    }
    if cleaned.is_empty() {
        cleaned.push_str("user");
    }
    format!(r"\\.\pipe\iris-ipc-{cleaned}")
}

#[cfg(not(windows))]
fn default_unix_socket() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        if !runtime.as_os_str().is_empty() {
            return runtime.join("iris").join("iris.sock");
        }
    }
    let uid = current_uid().unwrap_or(0);
    PathBuf::from(format!("/tmp/iris-{uid}.sock"))
}

/// Real user id from a `/proc/self/status` document.
///
/// The first field of the `Uid:` line is the real uid. Reading the file keeps
/// this crate free of `unsafe` (`libc::getuid` is not).
#[cfg(not(windows))]
fn uid_from_status(text: &str) -> Option<u32> {
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        let first = rest.split_whitespace().next()?;
        return first.parse().ok();
    }
    None
}

#[cfg(not(windows))]
fn current_uid() -> Option<u32> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    uid_from_status(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_log_filter_falls_back_to_info() {
        let config = LogConfig {
            filter: "  ".to_owned(),
        };
        assert_eq!(config.directive(), "info");
    }

    #[test]
    fn named_pipe_path_keeps_the_iris_prefix() {
        assert_eq!(named_pipe_path("Ada"), r"\\.\pipe\iris-ipc-Ada");
        assert_eq!(named_pipe_path("a b/../c"), r"\\.\pipe\iris-ipc-abc");
        assert_eq!(named_pipe_path("***"), r"\\.\pipe\iris-ipc-user");
    }

    #[test]
    fn default_endpoint_names_iris() {
        let endpoint = IpcEndpoint::local_default();
        let name = endpoint.display_name().to_string_lossy();
        assert!(name.contains("iris"), "{name}");
    }

    #[cfg(not(windows))]
    #[test]
    fn uid_from_status_reads_the_real_uid() {
        let text = "Name:\tiris\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\n";
        assert_eq!(uid_from_status(text), Some(1000));
        assert_eq!(uid_from_status("Name:\tnone\n"), None);
    }

    #[test]
    fn daemon_config_roundtrip_keeps_pause_flags() {
        let config = DaemonConfig::default();
        let bytes = serde_json::to_vec(&config).expect("serialize");
        let parsed: DaemonConfig = serde_json::from_slice(&bytes).expect("deserialize");
        assert!(parsed.pause_on_fullscreen);
        assert!(parsed.pause_on_game);
        assert_eq!(parsed.log.directive(), "info");
    }
}
