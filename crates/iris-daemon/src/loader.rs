//! Web wallpaper classification and the per-frame bridge.
//!
//! Image, video, and scene sources stay on the GPU loader. A directory whose
//! `project.json` says `"type": "web"`, or a directory that is only an
//! `index.html`, is opened as a [`iris_web::WebWallpaperInstance`].

use iris_core::{RunState, WallpaperConfig, WallpaperSource};
use iris_scene::AudioFftData;
use iris_telemetry::TelemetrySnapshot;

use crate::error::Error;

/// One web wallpaper bound to a monitor. The webview is not a GPU slot.
pub(crate) struct WebSlot {
    pub index: u32,
    pub connector: String,
    pub primary: bool,
    pub config: WallpaperConfig,
    pub state: RunState,
    pub message: Option<String>,
    pub user_paused: bool,
    pub auto_paused: bool,
    pub instance: iris_web::WebWallpaperInstance,
}

/// `Some` when `source` should skip the GPU loader.
pub(crate) fn web_project(source: &WallpaperSource) -> Result<Option<iris_web::WebProject>, Error> {
    let path = match source {
        WallpaperSource::Package { path } | WallpaperSource::SceneDirectory { path } => path,
        _ => return Ok(None),
    };
    iris_web::detect(path).map_err(Error::from)
}

/// Push the latest spectrum and telemetry while the slot is playing.
pub(crate) fn drive(slot: &mut WebSlot, audio: &AudioFftData, telemetry: &TelemetrySnapshot) {
    if slot.state != RunState::Playing {
        return;
    }
    if slot.config.audio_reactive {
        slot.instance.push_audio(&audio.bands_16, &audio.bands_64);
    } else {
        slot.instance.push_audio(&[0.0; 16], &[0.0; 64]);
    }
    slot.instance.push_telemetry(telemetry);
}

pub(crate) fn user_pause(slot: &mut WebSlot) {
    slot.user_paused = true;
    slot.auto_paused = false;
    if slot.state == RunState::Failed {
        return;
    }
    slot.state = RunState::Paused;
    slot.instance.set_paused(true);
}

pub(crate) fn user_resume(slot: &mut WebSlot) {
    slot.user_paused = false;
    slot.auto_paused = false;
    if slot.state != RunState::Paused {
        return;
    }
    if !slot.instance.launched() {
        slot.state = RunState::Failed;
        return;
    }
    slot.state = RunState::Playing;
    slot.message = None;
    slot.instance.set_paused(false);
}

pub(crate) fn apply_policy(
    slot: &mut WebSlot,
    fullscreen: bool,
    daemon_fullscreen: bool,
    daemon_game: bool,
) {
    let want = crate::policy::want_pause(daemon_fullscreen, daemon_game, &slot.config, fullscreen);
    let playing = slot.state == RunState::Playing;
    match crate::policy::context_step(slot.user_paused, slot.auto_paused, playing, want) {
        crate::policy::ContextStep::Hold => {}
        crate::policy::ContextStep::AutoPause => {
            slot.state = RunState::Paused;
            slot.auto_paused = true;
            slot.instance.set_paused(true);
        }
        crate::policy::ContextStep::AutoResume => {
            slot.auto_paused = false;
            if slot.state == RunState::Paused && slot.instance.launched() {
                slot.state = RunState::Playing;
                slot.instance.set_paused(false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn an_image_source_is_not_a_web_wallpaper() {
        let source = WallpaperSource::Image {
            path: PathBuf::from("still.png"),
        };
        let found = web_project(&source).expect("image");
        assert!(found.is_none());
    }
}
