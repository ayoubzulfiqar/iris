//! Process-wide tracing setup.
//!
//! Call [`init`] once, from the daemon or from the Tauri process. Libraries
//! should emit `tracing` events and leave the subscriber to the binary.

use tracing_subscriber::EnvFilter;

use crate::config::LogConfig;
use crate::error::Error;

/// Install a stderr [`tracing`] subscriber from `config`.
///
/// The filter string is an `EnvFilter` directive (`info`, `iris_daemon=debug`).
/// This function does not read `RUST_LOG`. A second call returns
/// [`Error::LogInit`] because the subscriber is process-global.
pub fn init(config: &LogConfig) -> Result<(), Error> {
    let directive = config.directive();
    let filter = EnvFilter::try_new(directive).map_err(|err| Error::LogFilter {
        filter: directive.to_owned(),
        detail: err.to_string(),
    })?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|err| Error::LogInit(err.to_string()))
}

/// Parse a filter directive without installing it.
pub fn parse_directive(directive: &str) -> Result<(), Error> {
    let directive = crate::config::normalize_filter(directive);
    EnvFilter::try_new(directive)
        .map(|_| ())
        .map_err(|err| Error::LogFilter {
            filter: directive.to_owned(),
            detail: err.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_directive_parses() {
        assert!(parse_directive("info").is_ok());
    }

    #[test]
    fn broken_directive_is_an_error() {
        let err = parse_directive("[").expect_err("unclosed directive");
        assert!(matches!(err, Error::LogFilter { .. }));
    }
}
