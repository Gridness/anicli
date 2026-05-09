use std::{
	env, fs,
	path::{Path, PathBuf},
};

use crate::{QualityPreference, TranslationMode};
use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};

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
	pub settings_path: PathBuf,
	pub anilist_auth_path: PathBuf,
	pub anilist_client_id: Option<String>,
	pub anilist_token: Option<String>,
	pub player: PlayerChoice,
	pub skip_intro: bool,
	pub download_mode: bool,
	pub skip_title: Option<String>,
	pub no_detach: bool,
	pub exit_after_play: bool,
	pub log_episode: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSettings {
	#[serde(default)]
	pub mode: TranslationMode,
	#[serde(default)]
	pub quality: QualityPreference,
	#[serde(default)]
	pub skip_intro: bool,
	#[serde(default)]
	pub download_mode: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UserSettingsFile {
	#[serde(default)]
	mode: Option<String>,
	#[serde(default)]
	quality: Option<String>,
	#[serde(default)]
	skip_intro: Option<bool>,
	#[serde(default)]
	download_mode: Option<bool>,
}

impl AppConfig {
	pub fn from_env() -> Self {
		let settings_path = default_settings_path();
		let anilist_auth_path = settings_path.with_file_name("anilist.toml");
		let settings = UserSettings::load(&settings_path).unwrap_or_default();
		let mode = env::var("ANI_CLI_MODE")
			.ok()
			.map(|value| parse_mode(&value))
			.unwrap_or(settings.mode);
		let quality = env::var("ANI_CLI_QUALITY")
			.ok()
			.map(QualityPreference::parse)
			.unwrap_or(settings.quality);
		let download_dir = env::var_os("ANI_CLI_DOWNLOAD_DIR")
			.map(PathBuf::from)
			.unwrap_or_else(|| PathBuf::from("."));
		let history_dir = env::var_os("ANI_CLI_HIST_DIR")
			.map(PathBuf::from)
			.unwrap_or_else(default_history_dir);
		let player_env = env::var("ANI_CLI_PLAYER").ok();
		let player = player_env
			.as_deref()
			.map(PlayerChoice::from_env_value)
			.unwrap_or(PlayerChoice::Auto);
		let download_mode = match &player {
			PlayerChoice::Download => true,
			_ if player_env.is_some() => false,
			_ => settings.download_mode,
		};
		let skip_title = env::var("ANI_CLI_SKIP_TITLE")
			.ok()
			.filter(|value| !value.is_empty());
		let anilist_client_id = env::var("ANI_CLI_ANILIST_CLIENT_ID")
			.ok()
			.map(|value| value.trim().to_owned())
			.filter(|value| !value.is_empty());
		let anilist_token = env::var("ANI_CLI_ANILIST_TOKEN")
			.ok()
			.map(|value| value.trim().to_owned())
			.filter(|value| !value.is_empty());

		Self {
			mode,
			quality,
			download_dir,
			history_dir,
			settings_path,
			anilist_auth_path,
			anilist_client_id,
			anilist_token,
			player,
			skip_intro: env_flag("ANI_CLI_SKIP_INTRO")
				.unwrap_or(settings.skip_intro),
			download_mode,
			skip_title,
			no_detach: env_flag("ANI_CLI_NO_DETACH").unwrap_or(false),
			exit_after_play: env_flag("ANI_CLI_EXIT_AFTER_PLAY")
				.unwrap_or(false),
			log_episode: env_flag("ANI_CLI_LOG").unwrap_or(true),
		}
	}

	pub fn user_settings(&self) -> UserSettings {
		UserSettings {
			mode: self.mode,
			quality: self.quality.clone(),
			skip_intro: self.skip_intro,
			download_mode: self.download_mode,
		}
	}

	pub fn save_user_settings(&self, settings: &UserSettings) -> Result<()> {
		settings.save(&self.settings_path)
	}
}

impl UserSettings {
	pub fn load(path: &Path) -> Result<Self> {
		if !path.exists() {
			return Ok(Self::default());
		}
		let contents = fs::read_to_string(path)
			.wrap_err_with(|| format!("failed to read {}", path.display()))?;
		let file: UserSettingsFile = toml::from_str(&contents)
			.wrap_err_with(|| format!("failed to parse {}", path.display()))?;
		Ok(Self {
			mode: file.mode.as_deref().map(parse_mode).unwrap_or_default(),
			quality: file
				.quality
				.map(QualityPreference::parse)
				.unwrap_or_default(),
			skip_intro: file.skip_intro.unwrap_or_default(),
			download_mode: file.download_mode.unwrap_or_default(),
		})
	}

	pub fn save(&self, path: &Path) -> Result<()> {
		if let Some(parent) = path.parent() {
			fs::create_dir_all(parent).wrap_err_with(|| {
				format!("failed to create {}", parent.display())
			})?;
		}
		let file = UserSettingsFile {
			mode: Some(self.mode.to_string()),
			quality: Some(self.quality.to_string()),
			skip_intro: Some(self.skip_intro),
			download_mode: Some(self.download_mode),
		};
		let contents = toml::to_string_pretty(&file)
			.wrap_err("failed to serialize user settings")?;
		fs::write(path, format!("{contents}\n"))
			.wrap_err_with(|| format!("failed to write {}", path.display()))
	}
}

fn parse_mode(value: &str) -> TranslationMode {
	match value {
		"dub" => TranslationMode::Dub,
		_ => TranslationMode::Sub,
	}
}

fn env_flag(name: &str) -> Option<bool> {
	env::var(name)
		.map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
		.ok()
}

fn default_history_dir() -> PathBuf {
	dirs::state_dir()
		.or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
		.unwrap_or_else(|| PathBuf::from("."))
		.join("ani-cli")
}

fn default_settings_path() -> PathBuf {
	dirs::config_dir()
		.or_else(|| dirs::home_dir().map(|home| home.join(".config")))
		.unwrap_or_else(|| PathBuf::from("."))
		.join("anicli-rs")
		.join("settings.toml")
}
