use std::{
	path::{Path, PathBuf},
	process::{Command, Stdio},
};

use anicli_aniskip::SkipLaunch;
use anicli_core::{AppConfig, PlayerChoice, SelectedStream};
use eyre::{Context, Result, eyre};

#[derive(Debug, Clone)]
pub enum PlayerKind {
	Iina(String),
	Mpv(String),
	Vlc(String),
	Syncplay(String),
	Download,
	Debug,
	Custom(String),
}

#[derive(Debug, Clone)]
pub struct PlaybackRequest {
	pub title: String,
	pub episode: String,
	pub stream: SelectedStream,
	pub player: PlayerKind,
	pub download_dir: PathBuf,
	pub no_detach: bool,
	pub exit_after_play: bool,
	pub log_episode: bool,
	pub skip: Option<SkipLaunch>,
}

#[derive(Debug, Clone)]
pub struct PlaybackOutcome {
	pub command: String,
	pub detached: bool,
	pub exit_code: Option<i32>,
}

impl PlaybackRequest {
	pub fn from_config(
		config: &AppConfig,
		title: impl Into<String>,
		episode: impl Into<String>,
		stream: SelectedStream,
	) -> Self {
		Self {
			title: title.into(),
			episode: episode.into(),
			stream,
			player: default_player(&config.player),
			download_dir: config.download_dir.clone(),
			no_detach: config.no_detach,
			exit_after_play: config.exit_after_play,
			log_episode: config.log_episode,
			skip: None,
		}
	}

	pub fn media_title(&self) -> String {
		format!("{}Episode {}", self.title, self.episode)
	}
}

pub fn launch(request: &PlaybackRequest) -> Result<PlaybackOutcome> {
	if request.log_episode
		&& !matches!(request.player, PlayerKind::Debug | PlayerKind::Download)
	{
		let _ = Command::new("logger")
			.args(["-t", "ani-cli", &request.media_title()])
			.status();
	}

	match &request.player {
		PlayerKind::Debug => Ok(PlaybackOutcome {
			command: format!(
				"debug {} [{}] {}",
				request.media_title(),
				request.stream.quality,
				request.stream.url
			),
			detached: false,
			exit_code: Some(0),
		}),
		PlayerKind::Download => download(request),
		PlayerKind::Iina(program) => launch_iina(program, request),
		PlayerKind::Mpv(program) => launch_mpv(program, request),
		PlayerKind::Vlc(program) => launch_vlc(program, request),
		PlayerKind::Syncplay(program) => launch_syncplay(program, request),
		PlayerKind::Custom(program) => launch_custom(program, request),
	}
}

pub fn read_system_logs() -> Result<String> {
	let output = if cfg!(target_os = "macos") {
		Command::new("log")
			.args([
				"show",
				"--style",
				"compact",
				"--last",
				"1d",
				"--predicate",
				"process == \"logger\"",
			])
			.output()
			.wrap_err("failed to run macOS log command")?
	} else if cfg!(target_os = "linux") {
		Command::new("journalctl")
			.args(["-t", "ani-cli", "-n", "80", "--no-pager"])
			.output()
			.wrap_err("failed to run journalctl")?
	} else {
		return Err(eyre!("log viewer is not implemented for this platform"));
	};

	let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
	if text.trim().is_empty() {
		text = String::from_utf8_lossy(&output.stderr).into_owned();
	}
	Ok(text)
}

pub fn default_player(choice: &PlayerChoice) -> PlayerKind {
	match choice {
		PlayerChoice::Auto => auto_player(),
		PlayerChoice::Iina => PlayerKind::Iina(where_iina()),
		PlayerChoice::Mpv => PlayerKind::Mpv(where_mpv()),
		PlayerChoice::Vlc => PlayerKind::Vlc(where_vlc()),
		PlayerChoice::Syncplay => PlayerKind::Syncplay(where_syncplay()),
		PlayerChoice::Download => PlayerKind::Download,
		PlayerChoice::Debug => PlayerKind::Debug,
		PlayerChoice::Custom(value) => PlayerKind::Custom(value.clone()),
	}
}

fn auto_player() -> PlayerKind {
	if cfg!(target_os = "macos") {
		PlayerKind::Iina(where_iina())
	} else {
		PlayerKind::Mpv(where_mpv())
	}
}

fn where_iina() -> String {
	let bundled = "/Applications/IINA.app/Contents/MacOS/iina-cli";
	if Path::new(bundled).exists() {
		bundled.to_owned()
	} else {
		"iina".to_owned()
	}
}

fn where_mpv() -> String {
	if Command::new("flatpak")
		.args(["info", "io.mpv.Mpv"])
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.status()
		.map(|status| status.success())
		.unwrap_or(false)
	{
		"flatpak_mpv".to_owned()
	} else {
		"mpv".to_owned()
	}
}

fn where_vlc() -> String {
	if cfg!(target_os = "windows") {
		"vlc.exe".to_owned()
	} else {
		"vlc".to_owned()
	}
}

fn where_syncplay() -> String {
	if cfg!(target_os = "macos") {
		"/Applications/Syncplay.app/Contents/MacOS/syncplay".to_owned()
	} else if cfg!(target_os = "windows") {
		"syncplay.exe".to_owned()
	} else {
		"syncplay".to_owned()
	}
}

fn launch_iina(
	program: &str,
	request: &PlaybackRequest,
) -> Result<PlaybackOutcome> {
	let mut args = vec!["--no-stdin".to_owned()];
	if !iina_running() {
		args.push("--keep-running".to_owned());
	}
	args.push(format!("--mpv-force-media-title={}", request.media_title()));
	if let Some(subtitle) = &request.stream.subtitle {
		args.push(format!("--mpv-sub-file={subtitle}"));
	}
	if let Some(referrer) = &request.stream.referrer {
		args.push(format!("--mpv-referrer={referrer}"));
	}
	if let Some(skip) = &request.skip {
		args.extend(skip.iina_args());
	}
	args.push(request.stream.url.clone());
	spawn_or_wait(program, args, true, request.exit_after_play)
}

fn launch_mpv(
	program: &str,
	request: &PlaybackRequest,
) -> Result<PlaybackOutcome> {
	if program == "flatpak_mpv" {
		let mut args = vec!["run".to_owned(), "io.mpv.Mpv".to_owned()];
		args.extend(mpv_args(request));
		return spawn_or_wait(
			"flatpak",
			args,
			!request.no_detach,
			request.exit_after_play,
		);
	}
	spawn_or_wait(
		program,
		mpv_args(request),
		!request.no_detach,
		request.exit_after_play,
	)
}

fn mpv_args(request: &PlaybackRequest) -> Vec<String> {
	let mut args =
		vec![format!("--force-media-title={}", request.media_title())];
	if let Some(subtitle) = &request.stream.subtitle {
		args.push(format!("--sub-file={subtitle}"));
	}
	if let Some(referrer) = &request.stream.referrer {
		args.push(format!("--referrer={referrer}"));
	}
	if let Some(skip) = &request.skip {
		args.extend(skip.mpv_args());
	}
	args.push(request.stream.url.clone());
	args
}

fn launch_vlc(
	program: &str,
	request: &PlaybackRequest,
) -> Result<PlaybackOutcome> {
	let mut args = Vec::new();
	if let Some(referrer) = &request.stream.referrer {
		args.push(format!("--http-referrer={referrer}"));
	}
	args.push("--play-and-exit".to_owned());
	args.push(format!("--meta-title={}", request.media_title()));
	args.push(request.stream.url.clone());
	spawn_or_wait(program, args, true, false)
}

fn launch_syncplay(
	program: &str,
	request: &PlaybackRequest,
) -> Result<PlaybackOutcome> {
	let mut args = vec![request.stream.url.clone(), "--".to_owned()];
	args.extend(
		mpv_args(request)
			.into_iter()
			.filter(|arg| arg != &request.stream.url),
	);
	spawn_or_wait(program, args, true, false)
}

fn launch_custom(
	program: &str,
	request: &PlaybackRequest,
) -> Result<PlaybackOutcome> {
	spawn_or_wait(program, vec![request.stream.url.clone()], true, false)
}

fn download(request: &PlaybackRequest) -> Result<PlaybackOutcome> {
	std::fs::create_dir_all(&request.download_dir).wrap_err_with(|| {
		format!("failed to create {}", request.download_dir.display())
	})?;
	if let Some(subtitle) = &request.stream.subtitle {
		let _ = Command::new("curl")
			.args([
				"-s",
				subtitle,
				"-o",
				&request
					.download_dir
					.join(format!("{}.vtt", request.media_title()))
					.display()
					.to_string(),
			])
			.status();
	}

	let output = request
		.download_dir
		.join(format!("{}.mp4", request.media_title()));
	if request.stream.url.contains(".m3u8") {
		if command_exists("yt-dlp") {
			let mut args = Vec::new();
			if let Some(referrer) = &request.stream.referrer {
				args.push("--referer".to_owned());
				args.push(referrer.clone());
			}
			args.extend([
				request.stream.url.clone(),
				"--no-skip-unavailable-fragments".to_owned(),
				"--fragment-retries".to_owned(),
				"infinite".to_owned(),
				"-N".to_owned(),
				"16".to_owned(),
				"-o".to_owned(),
				output.display().to_string(),
			]);
			return spawn_or_wait("yt-dlp", args, false, true);
		}
		let mut args = vec![
			"-extension_picky".to_owned(),
			"0".to_owned(),
			"-loglevel".to_owned(),
			"error".to_owned(),
			"-stats".to_owned(),
		];
		if let Some(referrer) = &request.stream.referrer {
			args.push("-referer".to_owned());
			args.push(referrer.clone());
		}
		args.extend([
			"-i".to_owned(),
			request.stream.url.clone(),
			"-c".to_owned(),
			"copy".to_owned(),
			output.display().to_string(),
		]);
		return spawn_or_wait("ffmpeg", args, false, true);
	}

	let mut args = vec![
		"--enable-rpc=false".to_owned(),
		"--check-certificate=false".to_owned(),
		"--continue".to_owned(),
		"--summary-interval=0".to_owned(),
		"-x".to_owned(),
		"16".to_owned(),
		"-s".to_owned(),
		"16".to_owned(),
	];
	if let Some(referrer) = &request.stream.referrer {
		args.push(format!("--referer={referrer}"));
	}
	args.extend([
		request.stream.url.clone(),
		format!("--dir={}", request.download_dir.display()),
		format!("-o={}.mp4", request.media_title()),
		"--download-result=hide".to_owned(),
	]);
	spawn_or_wait("aria2c", args, false, true)
}

fn spawn_or_wait(
	program: &str,
	args: Vec<String>,
	detach: bool,
	wait: bool,
) -> Result<PlaybackOutcome> {
	let command_line = format!("{} {}", program, args.join(" "));
	let mut command = Command::new(program);
	command.args(&args);

	if detach && !wait {
		command
			.stdin(Stdio::null())
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.spawn()
			.wrap_err_with(|| format!("failed to launch {program}"))?;
		return Ok(PlaybackOutcome {
			command: command_line,
			detached: true,
			exit_code: None,
		});
	}

	let status = command
		.status()
		.wrap_err_with(|| format!("failed to run {program}"))?;
	Ok(PlaybackOutcome {
		command: command_line,
		detached: false,
		exit_code: status.code(),
	})
}

fn iina_running() -> bool {
	Command::new("pgrep")
		.args(["-f", "IINA"])
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.status()
		.map(|status| status.success())
		.unwrap_or(false)
}

fn command_exists(program: &str) -> bool {
	Command::new("sh")
		.args(["-c", &format!("command -v {program} >/dev/null 2>&1")])
		.status()
		.map(|status| status.success())
		.unwrap_or(false)
}
