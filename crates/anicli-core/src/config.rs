use std::{env, path::PathBuf};

use crate::{QualityPreference, TranslationMode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerChoice {
    Auto,
    Iina,
    Mpv,
    Vlc,
    Syncplay,
    Download,
    Debug,
    Custom(String),
}

impl PlayerChoice {
    pub fn from_env_value(value: &str) -> Self {
        match value {
            "" => Self::Auto,
            "iina" => Self::Iina,
            "mpv" | "flatpak_mpv" | "mpv.exe" | "android_mpv" => Self::Mpv,
            "vlc" | "vlc.exe" | "android_vlc" => Self::Vlc,
            "syncplay" | "syncplay.exe" => Self::Syncplay,
            "download" => Self::Download,
            "debug" => Self::Debug,
            other => Self::Custom(other.to_owned()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub mode: TranslationMode,
    pub quality: QualityPreference,
    pub download_dir: PathBuf,
    pub history_dir: PathBuf,
    pub player: PlayerChoice,
    pub skip_intro: bool,
    pub skip_title: Option<String>,
    pub no_detach: bool,
    pub exit_after_play: bool,
    pub log_episode: bool,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let mode = match env::var("ANI_CLI_MODE").unwrap_or_default().as_str() {
            "dub" => TranslationMode::Dub,
            _ => TranslationMode::Sub,
        };
        let quality = QualityPreference::parse(
            env::var("ANI_CLI_QUALITY").unwrap_or_else(|_| "best".to_owned()),
        );
        let download_dir = env::var_os("ANI_CLI_DOWNLOAD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let history_dir = env::var_os("ANI_CLI_HIST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_history_dir);
        let player = env::var("ANI_CLI_PLAYER")
            .map(|value| PlayerChoice::from_env_value(&value))
            .unwrap_or(PlayerChoice::Auto);
        let skip_title = env::var("ANI_CLI_SKIP_TITLE")
            .ok()
            .filter(|value| !value.is_empty());

        Self {
            mode,
            quality,
            download_dir,
            history_dir,
            player,
            skip_intro: env_flag("ANI_CLI_SKIP_INTRO", false),
            skip_title,
            no_detach: env_flag("ANI_CLI_NO_DETACH", false),
            exit_after_play: env_flag("ANI_CLI_EXIT_AFTER_PLAY", false),
            log_episode: env_flag("ANI_CLI_LOG", true),
        }
    }
}

fn env_flag(name: &str, default: bool) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn default_history_dir() -> PathBuf {
    if let Some(value) = env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(value).join("ani-cli");
    }

    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/state/ani-cli")
}
