use std::{io, time::Duration};

use anicli_allanime::{
	AllAnimeClient, NextEpisodeStatus, fetch_next_episode_status,
	select_quality,
};
use anicli_aniskip::{
	AniSkipClient, MpvSkipOptions, build_mpv_skip_launch, install_iina_plugin,
};
use anicli_core::{
	AnimeSearchResult, AppConfig, HistoryEntry, HistoryStore, PlayerChoice,
	QualityPreference, SelectedStream, StreamLink, SubtitleTrack,
	TranslationMode, next_episode, previous_episode,
};
use anicli_player::{
	PlaybackRequest, PlayerKind, default_player, launch, read_system_logs,
};
use crossterm::{
	event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
	execute,
	terminal::{
		EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
		enable_raw_mode,
	},
};
use eyre::{Result, WrapErr, eyre};
use ratatui::{
	Frame, Terminal,
	backend::CrosstermBackend,
	layout::{Constraint, Direction, Layout, Rect},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
	Search,
	Results,
	Episodes,
	Playing,
	Quality,
	Language,
	TrackLanguage,
	Settings,
	SettingOptions,
	Help,
	History,
	Logs,
	Schedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingChoice {
	Language,
	Quality,
	DownloadMode,
	AniSkip,
}

pub async fn run() -> Result<()> {
	let _terminal = TerminalGuard::enter()?;
	let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
		.wrap_err("failed to create terminal")?;
	let mut app = App::new()?;

	loop {
		terminal.draw(|frame| draw(frame, &app))?;
		if app.quit {
			break;
		}
		if event::poll(Duration::from_millis(200))? {
			if let Event::Key(key) = event::read()? {
				app.handle_key(key).await?;
			}
		}
	}

	Ok(())
}

struct TerminalGuard;

impl TerminalGuard {
	fn enter() -> Result<Self> {
		enable_raw_mode().wrap_err("failed to enable raw mode")?;
		execute!(io::stdout(), EnterAlternateScreen)
			.wrap_err("failed to enter alternate screen")?;
		Ok(Self)
	}
}

impl Drop for TerminalGuard {
	fn drop(&mut self) {
		let _ = disable_raw_mode();
		let _ = execute!(io::stdout(), LeaveAlternateScreen);
	}
}

struct App {
	config: AppConfig,
	history: HistoryStore,
	allanime: AllAnimeClient,
	aniskip: AniSkipClient,
	screen: Screen,
	nav_stack: Vec<Screen>,
	query: String,
	mode: TranslationMode,
	quality: QualityPreference,
	skip_intro: bool,
	download_mode: bool,
	results: Vec<AnimeSearchResult>,
	result_index: usize,
	selected_show: Option<AnimeSearchResult>,
	episodes: Vec<String>,
	episode_index: usize,
	current_episode: Option<String>,
	links: Vec<StreamLink>,
	last_stream: Option<SelectedStream>,
	history_entries: Vec<HistoryEntry>,
	history_index: usize,
	logs: String,
	schedule: Vec<NextEpisodeStatus>,
	quality_index: usize,
	language_index: usize,
	settings_index: usize,
	active_setting: Option<SettingChoice>,
	setting_option_index: usize,
	track_language_index: usize,
	track_language_choices: Vec<TrackLanguageChoice>,
	pending_playback: Option<PendingPlayback>,
	status: String,
	quit: bool,
}

#[derive(Debug, Clone)]
struct PendingPlayback {
	show: AnimeSearchResult,
	episode: String,
	links: Vec<StreamLink>,
	selected: SelectedStream,
	playback_config: AppConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrackLanguageKind {
	Subtitle,
	Hardsub,
	DubAudio,
}

#[derive(Debug, Clone)]
struct TrackLanguageChoice {
	kind: TrackLanguageKind,
	label: String,
	code: Option<String>,
	subtitle: Option<SubtitleTrack>,
}

impl App {
	fn new() -> Result<Self> {
		let config = AppConfig::from_env();
		Ok(Self {
			history: HistoryStore::new(config.history_dir.clone()),
			allanime: AllAnimeClient::new()?,
			aniskip: AniSkipClient::new()?,
			screen: Screen::Search,
			nav_stack: Vec::new(),
			query: String::new(),
			mode: config.mode,
			quality: config.quality.clone(),
			skip_intro: config.skip_intro,
			download_mode: config.download_mode,
			results: Vec::new(),
			result_index: 0,
			selected_show: None,
			episodes: Vec::new(),
			episode_index: 0,
			current_episode: None,
			links: Vec::new(),
			last_stream: None,
			history_entries: Vec::new(),
			history_index: 0,
			logs: String::new(),
			schedule: Vec::new(),
			quality_index: 0,
			language_index: language_choices()
				.iter()
				.position(|mode| mode == &config.mode)
				.unwrap_or(0),
			settings_index: 0,
			active_setting: None,
			setting_option_index: 0,
			track_language_index: 0,
			track_language_choices: Vec::new(),
			pending_playback: None,
			status: "Type an anime title and press Enter.".to_owned(),
			quit: false,
			config,
		})
	}

	async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
		if key.modifiers.contains(KeyModifiers::CONTROL)
			&& key.code == KeyCode::Char('c')
		{
			self.quit = true;
			return Ok(());
		}

		if key.code == KeyCode::F(1)
			|| (key.code == KeyCode::Char('?') && self.screen != Screen::Search)
		{
			self.show_help();
			return Ok(());
		}

		if key.code == KeyCode::F(2)
			|| (key.code == KeyCode::Char('s')
				&& self.single_key_shortcuts_enabled())
		{
			self.show_settings();
			return Ok(());
		}

		if key.code == KeyCode::Esc {
			self.go_back();
			return Ok(());
		}

		if self.single_key_shortcuts_enabled() {
			match key.code {
				KeyCode::Char('i') => {
					self.install_iina_plugin()?;
					return Ok(());
				}
				KeyCode::Char('l') if self.screen != Screen::Logs => {
					self.show_logs()?;
					return Ok(());
				}
				KeyCode::Char('h') if self.screen != Screen::History => {
					self.show_history()?;
					return Ok(());
				}
				KeyCode::Char('d') => {
					self.set_download_mode(!self.download_mode);
					return Ok(());
				}
				KeyCode::Char('k') => {
					self.set_skip_intro(!self.skip_intro);
					return Ok(());
				}
				KeyCode::Char('m') if self.screen != Screen::Language => {
					self.show_language();
					return Ok(());
				}
				KeyCode::Char('c') if self.screen != Screen::Quality => {
					self.show_quality();
					return Ok(());
				}
				_ => {}
			}
		}

		match self.screen {
			Screen::Search => self.handle_search_key(key).await?,
			Screen::Results => self.handle_results_key(key).await?,
			Screen::Episodes => self.handle_episodes_key(key).await?,
			Screen::Playing => self.handle_playing_key(key).await?,
			Screen::Quality => self.handle_quality_key(key)?,
			Screen::Language => self.handle_language_key(key)?,
			Screen::TrackLanguage => {
				self.handle_track_language_key(key).await?
			}
			Screen::Settings => self.handle_settings_key(key),
			Screen::SettingOptions => self.handle_setting_options_key(key),
			Screen::Help => {}
			Screen::History => self.handle_history_key(key).await?,
			Screen::Logs | Screen::Schedule => {}
		}
		Ok(())
	}

	fn single_key_shortcuts_enabled(&self) -> bool {
		!matches!(
			self.screen,
			Screen::Search
				| Screen::Quality
				| Screen::Language
				| Screen::SettingOptions
				| Screen::Settings
		)
	}

	async fn handle_search_key(&mut self, key: KeyEvent) -> Result<()> {
		match key.code {
			KeyCode::Enter => self.search().await?,
			KeyCode::Backspace => {
				self.query.pop();
			}
			KeyCode::Char(ch) => self.query.push(ch),
			_ => {}
		}
		Ok(())
	}

	async fn handle_results_key(&mut self, key: KeyEvent) -> Result<()> {
		match key.code {
			KeyCode::Up => {
				self.result_index = self.result_index.saturating_sub(1)
			}
			KeyCode::Down => {
				self.result_index = (self.result_index + 1)
					.min(self.results.len().saturating_sub(1))
			}
			KeyCode::Enter => self.select_result().await?,
			KeyCode::Char('/') => self.return_to_screen(Screen::Search),
			_ => {}
		}
		Ok(())
	}

	async fn handle_episodes_key(&mut self, key: KeyEvent) -> Result<()> {
		match key.code {
			KeyCode::Up => {
				self.episode_index = (self.episode_index + 1)
					.min(self.episodes.len().saturating_sub(1))
			}
			KeyCode::Down => {
				self.episode_index = self.episode_index.saturating_sub(1);
			}
			KeyCode::Enter => {
				if let Some(episode) =
					self.episodes.get(self.episode_index).cloned()
				{
					if let Err(err) = self.play_episode(episode).await {
						self.status = format!("{err:#}");
					}
				}
			}
			KeyCode::Char('N') => self.fetch_schedule().await?,
			_ => {}
		}
		Ok(())
	}

	async fn handle_playing_key(&mut self, key: KeyEvent) -> Result<()> {
		match key.code {
			KeyCode::Char('n') => {
				if let Some(current) = self.current_episode.clone() {
					if let Some(next) = next_episode(&self.episodes, &current)
						.map(ToOwned::to_owned)
					{
						if let Err(err) = self.play_episode(next).await {
							self.status = format!("{err:#}");
						}
					} else {
						self.status =
							"Out of range: no next episode.".to_owned();
					}
				}
			}
			KeyCode::Char('p') => {
				if let Some(current) = self.current_episode.clone() {
					if let Some(previous) =
						previous_episode(&self.episodes, &current)
							.map(ToOwned::to_owned)
					{
						if let Err(err) = self.play_episode(previous).await {
							self.status = format!("{err:#}");
						}
					} else {
						self.status =
							"Out of range: no previous episode.".to_owned();
					}
				}
			}
			KeyCode::Char('r') => {
				if let Some(current) = self.current_episode.clone() {
					if let Err(err) = self.play_episode(current).await {
						self.status = format!("{err:#}");
					}
				}
			}
			KeyCode::Char('e') => self.return_to_screen(Screen::Episodes),
			KeyCode::Char('q') => self.quit = true,
			_ => {}
		}
		Ok(())
	}

	fn handle_quality_key(&mut self, key: KeyEvent) -> Result<()> {
		let choices = quality_choices();
		match key.code {
			KeyCode::Up => {
				self.quality_index = self.quality_index.saturating_sub(1)
			}
			KeyCode::Down => {
				self.quality_index = (self.quality_index + 1)
					.min(choices.len().saturating_sub(1))
			}
			KeyCode::Enter => {
				let selected = choices
					.get(self.quality_index)
					.cloned()
					.unwrap_or(QualityPreference::Best);
				self.set_quality(selected);
				self.go_back();
			}
			_ => {}
		}
		Ok(())
	}

	fn handle_language_key(&mut self, key: KeyEvent) -> Result<()> {
		let choices = language_choices();
		match key.code {
			KeyCode::Up => {
				self.language_index = self.language_index.saturating_sub(1)
			}
			KeyCode::Down => {
				self.language_index = (self.language_index + 1)
					.min(choices.len().saturating_sub(1))
			}
			KeyCode::Enter => {
				let selected = choices
					.get(self.language_index)
					.copied()
					.unwrap_or(TranslationMode::Sub);
				let changed = selected != self.mode;
				self.set_mode(selected);
				if changed {
					self.reset_to_search();
				} else {
					self.go_back();
				}
			}
			_ => {}
		}
		Ok(())
	}

	async fn handle_track_language_key(&mut self, key: KeyEvent) -> Result<()> {
		match key.code {
			KeyCode::Up => {
				self.track_language_index =
					self.track_language_index.saturating_sub(1)
			}
			KeyCode::Down => {
				self.track_language_index = (self.track_language_index + 1)
					.min(self.track_language_choices.len().saturating_sub(1))
			}
			KeyCode::Enter => {
				if let (Some(pending), Some(choice)) = (
					self.pending_playback.take(),
					self.track_language_choices
						.get(self.track_language_index)
						.cloned(),
				) {
					let selected = apply_track_language_choice(
						&pending,
						&choice,
						&self.quality,
					);
					self.track_language_choices.clear();
					self.launch_episode(
						pending.show,
						pending.episode,
						selected,
						pending.playback_config,
					)
					.await?;
				}
			}
			_ => {}
		}
		Ok(())
	}

	async fn handle_history_key(&mut self, key: KeyEvent) -> Result<()> {
		match key.code {
			KeyCode::Up => {
				self.history_index = self.history_index.saturating_sub(1)
			}
			KeyCode::Down => {
				self.history_index = (self.history_index + 1)
					.min(self.history_entries.len().saturating_sub(1))
			}
			KeyCode::Enter => {
				if let Err(err) = self.continue_history().await {
					self.status = format!("{err:#}");
				}
			}
			KeyCode::Char('x') => {
				self.history.clear()?;
				self.history_entries.clear();
				self.status = "History deleted.".to_owned();
			}
			_ => {}
		}
		Ok(())
	}

	fn handle_settings_key(&mut self, key: KeyEvent) {
		match key.code {
			KeyCode::Up => {
				self.settings_index = self.settings_index.saturating_sub(1)
			}
			KeyCode::Down => {
				self.settings_index = (self.settings_index + 1)
					.min(setting_choices_len().saturating_sub(1))
			}
			KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => {
				if let Some(choice) = setting_choice(self.settings_index) {
					self.show_setting_options(choice);
				}
			}
			_ => {}
		}
	}

	fn handle_setting_options_key(&mut self, key: KeyEvent) {
		match key.code {
			KeyCode::Up => {
				self.setting_option_index =
					self.setting_option_index.saturating_sub(1)
			}
			KeyCode::Down => {
				let last = self
					.active_setting
					.map(setting_option_count)
					.unwrap_or(0)
					.saturating_sub(1);
				self.setting_option_index =
					(self.setting_option_index + 1).min(last);
			}
			KeyCode::Enter => self.apply_setting_option(),
			_ => {}
		}
	}

	fn push_screen(&mut self, screen: Screen) {
		if self.screen == screen {
			return;
		}
		self.nav_stack.push(self.screen);
		self.screen = screen;
	}

	fn go_back(&mut self) {
		let leaving = self.screen;
		self.cleanup_screen(leaving);
		if let Some(previous) = self.nav_stack.pop() {
			self.screen = previous;
		} else if self.screen == Screen::Search {
			self.quit = true;
		} else {
			self.screen = Screen::Search;
		}
	}

	fn return_to_screen(&mut self, screen: Screen) {
		self.cleanup_screen(self.screen);
		if let Some(index) = self
			.nav_stack
			.iter()
			.rposition(|candidate| *candidate == screen)
		{
			self.nav_stack.truncate(index);
			self.screen = screen;
		} else {
			self.nav_stack.clear();
			self.screen = screen;
		}
	}

	fn reset_to_search(&mut self) {
		self.pending_playback = None;
		self.track_language_choices.clear();
		self.nav_stack.clear();
		self.screen = Screen::Search;
	}

	fn cleanup_screen(&mut self, screen: Screen) {
		if screen == Screen::TrackLanguage {
			self.pending_playback = None;
			self.track_language_choices.clear();
			self.status = "Track language selection cancelled.".to_owned();
		} else if screen == Screen::SettingOptions {
			self.active_setting = None;
			self.setting_option_index = 0;
		}
	}

	fn show_help(&mut self) {
		self.push_screen(Screen::Help);
	}

	fn show_settings(&mut self) {
		if self.screen == Screen::Settings {
			return;
		}
		if self
			.nav_stack
			.iter()
			.any(|candidate| *candidate == Screen::Settings)
		{
			self.return_to_screen(Screen::Settings);
			return;
		}
		self.settings_index = 0;
		self.push_screen(Screen::Settings);
	}

	fn show_setting_options(&mut self, choice: SettingChoice) {
		self.active_setting = Some(choice);
		self.setting_option_index = self.current_setting_option_index(choice);
		self.push_screen(Screen::SettingOptions);
	}

	fn show_language(&mut self) {
		self.language_index = language_choices()
			.iter()
			.position(|mode| mode == &self.mode)
			.unwrap_or(0);
		self.push_screen(Screen::Language);
	}

	fn show_quality(&mut self) {
		self.quality_index = quality_choices()
			.iter()
			.position(|quality| quality == &self.quality)
			.unwrap_or(0);
		self.push_screen(Screen::Quality);
	}

	fn current_setting_option_index(&self, choice: SettingChoice) -> usize {
		match choice {
			SettingChoice::Language => language_choices()
				.iter()
				.position(|mode| mode == &self.mode)
				.unwrap_or(0),
			SettingChoice::Quality => quality_choices()
				.iter()
				.position(|quality| quality == &self.quality)
				.unwrap_or(0),
			SettingChoice::DownloadMode => usize::from(self.download_mode),
			SettingChoice::AniSkip => usize::from(self.skip_intro),
		}
	}

	fn apply_setting_option(&mut self) {
		let Some(choice) = self.active_setting else {
			return;
		};
		match choice {
			SettingChoice::Language => {
				let selected = language_choices()
					.get(self.setting_option_index)
					.copied()
					.unwrap_or(TranslationMode::Sub);
				let changed = selected != self.mode;
				self.set_mode(selected);
				if changed {
					self.nav_stack.clear();
					self.nav_stack.push(Screen::Search);
					self.nav_stack.push(Screen::Settings);
				}
			}
			SettingChoice::Quality => {
				let selected = quality_choices()
					.get(self.setting_option_index)
					.cloned()
					.unwrap_or(QualityPreference::Best);
				self.set_quality(selected);
			}
			SettingChoice::DownloadMode => {
				self.set_download_mode(self.setting_option_index == 1);
			}
			SettingChoice::AniSkip => {
				self.set_skip_intro(self.setting_option_index == 1);
			}
		}
		self.go_back();
	}

	fn set_mode(&mut self, mode: TranslationMode) {
		if mode != self.mode {
			self.mode = mode;
			self.config.mode = mode;
			self.results.clear();
			self.episodes.clear();
			self.links.clear();
			self.selected_show = None;
			self.current_episode = None;
			self.last_stream = None;
			self.pending_playback = None;
			self.track_language_choices.clear();
			self.status = format!(
				"Language set to {}. Search again for matching results.",
				self.mode
			);
		} else {
			self.status = format!("Language remains {}.", self.mode);
		}
		self.persist_user_settings();
	}

	fn set_quality(&mut self, quality: QualityPreference) {
		self.quality = quality;
		self.config.quality = self.quality.clone();
		self.status = format!("Quality set to {}.", self.quality);
		self.persist_user_settings();
	}

	fn set_download_mode(&mut self, enabled: bool) {
		self.download_mode = enabled;
		self.config.download_mode = enabled;
		self.status = format!(
			"Download mode {}.",
			if self.download_mode {
				"enabled"
			} else {
				"disabled"
			}
		);
		self.persist_user_settings();
	}

	fn set_skip_intro(&mut self, enabled: bool) {
		self.skip_intro = enabled;
		self.config.skip_intro = enabled;
		self.status = format!(
			"AniSkip {}.",
			if self.skip_intro {
				"enabled"
			} else {
				"disabled"
			}
		);
		self.persist_user_settings();
	}

	fn persist_user_settings(&mut self) {
		let settings = self.config.user_settings();
		if let Err(err) = self.config.save_user_settings(&settings) {
			self.status = format!(
				"{} Settings changed but could not be saved: {err:#}",
				self.status
			);
		}
	}

	async fn search(&mut self) -> Result<()> {
		let query = self.query.trim();
		if query.is_empty() {
			self.status = "Search query is empty.".to_owned();
			return Ok(());
		}
		self.status = format!("Searching AllAnime for \"{query}\"...");
		self.results = self.allanime.search(query, self.mode).await?;
		self.result_index = 0;
		if self.results.is_empty() {
			self.status = "No results found.".to_owned();
			self.screen = Screen::Search;
		} else {
			self.status =
				format!("{} result(s). Select an anime.", self.results.len());
			self.push_screen(Screen::Results);
		}
		Ok(())
	}

	async fn select_result(&mut self) -> Result<()> {
		let show = self
			.results
			.get(self.result_index)
			.cloned()
			.ok_or_else(|| eyre!("no anime result selected"))?;
		self.status = format!("Fetching episodes for {}...", show.title);
		self.episodes = self.allanime.episodes(&show.id, self.mode).await?;
		self.episode_index = self.episodes.len().saturating_sub(1);
		self.selected_show = Some(show);
		self.push_screen(Screen::Episodes);
		self.status = format!("{} episode(s) available.", self.episodes.len());
		Ok(())
	}

	async fn play_episode(&mut self, episode: String) -> Result<()> {
		let show = self
			.selected_show
			.clone()
			.ok_or_else(|| eyre!("no anime selected"))?;
		self.status = format!("Fetching episode {episode} sources...");

		let filter_soft_subs = !self.download_mode
			&& matches!(
				default_player(&self.config.player),
				PlayerKind::Vlc(_)
			);
		let sources = self
			.allanime
			.episode_sources(
				&show.id,
				self.mode,
				&episode,
				&self.quality,
				filter_soft_subs,
			)
			.await?;
		let mut playback_config = self.config.clone();
		playback_config.quality = self.quality.clone();
		if self.download_mode {
			playback_config.player = PlayerChoice::Download;
		} else if matches!(playback_config.player, PlayerChoice::Download) {
			playback_config.player = PlayerChoice::Auto;
		}

		let selected = sources.selected.clone();
		self.links = sources.links.clone();
		self.last_stream = Some(selected.clone());

		let choices =
			track_language_choices(self.mode, &sources.links, &selected);
		if choices.len() > 1 {
			self.track_language_index = 0;
			self.track_language_choices = choices;
			self.pending_playback = Some(PendingPlayback {
				show,
				episode,
				links: sources.links,
				selected,
				playback_config,
			});
			self.push_screen(Screen::TrackLanguage);
			self.status = "Select track language for this episode.".to_owned();
			return Ok(());
		}

		self.launch_episode(show, episode, selected, playback_config)
			.await
	}

	async fn launch_episode(
		&mut self,
		show: AnimeSearchResult,
		episode: String,
		selected: SelectedStream,
		playback_config: AppConfig,
	) -> Result<()> {
		let mut request = PlaybackRequest::from_config(
			&playback_config,
			show.media_title_prefix(),
			episode.clone(),
			selected.clone(),
		);

		if self.skip_intro && !self.download_mode {
			match self.prepare_skip(&show, &episode).await {
				Ok(skip) => request.skip = Some(skip),
				Err(err) => {
					self.status = format!(
						"AniSkip unavailable: {err:#}. Playing without it."
					);
				}
			}
		}

		let outcome = launch(&request)?;
		self.last_stream = Some(selected);
		self.history.upsert(HistoryEntry {
			episode: episode.clone(),
			anime_id: show.id.clone(),
			title: show.display_title(),
		})?;
		self.current_episode = Some(episode.clone());
		if self.screen == Screen::TrackLanguage {
			self.screen = Screen::Playing;
		} else if self.screen != Screen::Playing {
			self.push_screen(Screen::Playing);
		}
		self.status = if self.download_mode {
			format!(
				"Download started for episode {episode}: {}",
				outcome.command
			)
		} else {
			format!("Playing episode {episode}: {}", outcome.command)
		};
		Ok(())
	}

	async fn prepare_skip(
		&self,
		show: &AnimeSearchResult,
		episode: &str,
	) -> Result<anicli_aniskip::SkipLaunch> {
		let mal_id = match self.allanime.mal_id(&show.id).await.ok().flatten() {
			Some(mal_id) => mal_id,
			None => {
				let query = self
					.config
					.skip_title
					.as_deref()
					.unwrap_or(&show.title)
					.to_owned();
				resolve_skip_query(&self.aniskip, query).await?
			}
		};

		build_mpv_skip_launch(
			&self.aniskip,
			mal_id,
			episode,
			&MpvSkipOptions::default(),
		)
		.await
	}

	fn install_iina_plugin(&mut self) -> Result<()> {
		let install = install_iina_plugin()?;
		self.status = if install.enabled_plugin_system {
			format!(
				"IINA AniSkip plugin installed at {}. Restart IINA if it was open.",
				install.plugin_dir.display()
			)
		} else {
			format!(
				"IINA plugin files installed at {}, but enabling plugin system via defaults failed.",
				install.plugin_dir.display()
			)
		};
		Ok(())
	}

	fn show_history(&mut self) -> Result<()> {
		self.history_entries = self.history.load()?;
		self.history_index = 0;
		self.push_screen(Screen::History);
		self.status = if self.history_entries.is_empty() {
			"History is empty.".to_owned()
		} else {
			"Select an entry to continue, or press x to delete history."
				.to_owned()
		};
		Ok(())
	}

	async fn continue_history(&mut self) -> Result<()> {
		let entry = self
			.history_entries
			.get(self.history_index)
			.cloned()
			.ok_or_else(|| eyre!("no history entry selected"))?;
		self.status = format!("Continuing {}...", entry.title);
		self.episodes =
			self.allanime.episodes(&entry.anime_id, self.mode).await?;
		let next = next_episode(&self.episodes, &entry.episode)
			.map(ToOwned::to_owned)
			.ok_or_else(|| {
				eyre!("no unwatched episode remains for {}", entry.title)
			})?;
		self.episode_index = self
			.episodes
			.iter()
			.position(|episode| episode == &next)
			.unwrap_or_default();
		self.selected_show = Some(AnimeSearchResult {
			id: entry.anime_id,
			title: entry
				.title
				.split(" (")
				.next()
				.unwrap_or(&entry.title)
				.to_owned(),
			episode_count: self.episodes.last().and_then(|ep| ep.parse().ok()),
		});
		self.nav_stack.clear();
		self.nav_stack.push(Screen::Search);
		self.screen = Screen::Episodes;
		self.play_episode(next).await
	}

	fn show_logs(&mut self) -> Result<()> {
		self.logs = read_system_logs()?;
		self.push_screen(Screen::Logs);
		self.status = "Log view. Press Esc to return.".to_owned();
		Ok(())
	}

	async fn fetch_schedule(&mut self) -> Result<()> {
		let query = self
			.selected_show
			.as_ref()
			.map(|show| show.title.as_str())
			.unwrap_or(self.query.as_str());
		self.status = format!("Fetching next episode schedule for {query}...");
		self.schedule = fetch_next_episode_status(query).await?;
		self.push_screen(Screen::Schedule);
		self.status = "Schedule loaded. Press Esc to return.".to_owned();
		Ok(())
	}
}

async fn resolve_skip_query(
	client: &AniSkipClient,
	query: String,
) -> Result<u64> {
	client
		.resolve_mal_id(&query, anicli_aniskip::SkipSource::MyAnimeList, None)
		.await
}

fn quality_choices() -> Vec<QualityPreference> {
	vec![
		QualityPreference::Best,
		QualityPreference::Exact("1080".to_owned()),
		QualityPreference::Exact("720".to_owned()),
		QualityPreference::Exact("480".to_owned()),
		QualityPreference::Exact("360".to_owned()),
		QualityPreference::Worst,
	]
}

fn setting_choices_len() -> usize {
	setting_choices().len()
}

fn setting_choices() -> &'static [SettingChoice] {
	&[
		SettingChoice::Language,
		SettingChoice::Quality,
		SettingChoice::DownloadMode,
		SettingChoice::AniSkip,
	]
}

fn setting_choice(index: usize) -> Option<SettingChoice> {
	setting_choices().get(index).copied()
}

fn setting_option_count(choice: SettingChoice) -> usize {
	match choice {
		SettingChoice::Language => language_choices().len(),
		SettingChoice::Quality => quality_choices().len(),
		SettingChoice::DownloadMode | SettingChoice::AniSkip => 2,
	}
}

fn language_choices() -> Vec<TranslationMode> {
	vec![TranslationMode::Sub, TranslationMode::Dub]
}

fn track_language_choices(
	mode: TranslationMode,
	links: &[StreamLink],
	selected: &SelectedStream,
) -> Vec<TrackLanguageChoice> {
	match mode {
		TranslationMode::Sub => {
			if selected.subtitles.len() > 1 {
				return dedupe_track_choices(
					selected
						.subtitles
						.iter()
						.cloned()
						.map(|track| TrackLanguageChoice {
							kind: TrackLanguageKind::Subtitle,
							label: track.display_label(),
							code: Some(track.lang.clone()),
							subtitle: Some(track),
						})
						.collect(),
				);
			}

			let hardsub_choices = links
				.iter()
				.filter_map(|link| {
					let code = link.hardsub_language.clone()?;
					Some(TrackLanguageChoice {
						kind: TrackLanguageKind::Hardsub,
						label: format!("Hard subtitles ({code})"),
						code: Some(code),
						subtitle: None,
					})
				})
				.collect::<Vec<_>>();
			dedupe_track_choices(hardsub_choices)
		}
		TranslationMode::Dub => {
			let choices = links
				.iter()
				.filter_map(|link| {
					let code = link.audio_language.clone()?;
					Some(TrackLanguageChoice {
						kind: TrackLanguageKind::DubAudio,
						label: format!("Dub audio ({code})"),
						code: Some(code),
						subtitle: None,
					})
				})
				.collect::<Vec<_>>();
			dedupe_track_choices(choices)
		}
	}
}

fn dedupe_track_choices(
	choices: Vec<TrackLanguageChoice>,
) -> Vec<TrackLanguageChoice> {
	let mut deduped = Vec::new();
	for choice in choices {
		let key = (
			choice.kind,
			choice.code.clone(),
			choice.subtitle.as_ref().map(|track| track.url.clone()),
		);
		if !deduped.iter().any(|existing: &TrackLanguageChoice| {
			(
				existing.kind,
				existing.code.clone(),
				existing.subtitle.as_ref().map(|track| track.url.clone()),
			) == key
		}) {
			deduped.push(choice);
		}
	}
	deduped
}

fn apply_track_language_choice(
	pending: &PendingPlayback,
	choice: &TrackLanguageChoice,
	quality: &QualityPreference,
) -> SelectedStream {
	match choice.kind {
		TrackLanguageKind::Subtitle => pending
			.selected
			.clone()
			.with_subtitle_track(choice.subtitle.clone()),
		TrackLanguageKind::Hardsub => select_quality(
			&pending
				.links
				.iter()
				.filter(|link| {
					link.hardsub_language.as_deref() == choice.code.as_deref()
				})
				.cloned()
				.collect::<Vec<_>>(),
			quality,
		)
		.unwrap_or_else(|| pending.selected.clone()),
		TrackLanguageKind::DubAudio => select_quality(
			&pending
				.links
				.iter()
				.filter(|link| {
					link.audio_language.as_deref() == choice.code.as_deref()
				})
				.cloned()
				.collect::<Vec<_>>(),
			quality,
		)
		.unwrap_or_else(|| pending.selected.clone()),
	}
}

fn draw(frame: &mut Frame<'_>, app: &App) {
	let chunks = Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Length(3),
			Constraint::Min(5),
			Constraint::Length(4),
		])
		.split(frame.area());
	draw_header(frame, chunks[0], app);
	match app.screen {
		Screen::Search => draw_search(frame, chunks[1], app),
		Screen::Results => draw_results(frame, chunks[1], app),
		Screen::Episodes => draw_episodes(frame, chunks[1], app),
		Screen::Playing => draw_playing(frame, chunks[1], app),
		Screen::Quality => draw_quality(frame, chunks[1], app),
		Screen::Language => draw_language(frame, chunks[1], app),
		Screen::TrackLanguage => draw_track_language(frame, chunks[1], app),
		Screen::Settings => draw_settings(frame, chunks[1], app),
		Screen::SettingOptions => draw_setting_options(frame, chunks[1], app),
		Screen::Help => draw_help(frame, chunks[1], app),
		Screen::History => draw_history(frame, chunks[1], app),
		Screen::Logs => draw_text(frame, chunks[1], "Logs", &app.logs),
		Screen::Schedule => draw_schedule(frame, chunks[1], app),
	}
	draw_footer(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let title = format!(
		"anicli-rs | mode: {} | quality: {} | skip: {} | download: {}",
		app.mode,
		app.quality,
		if app.skip_intro { "on" } else { "off" },
		if app.download_mode { "on" } else { "off" }
	);
	frame.render_widget(
		Paragraph::new(title)
			.block(Block::default().borders(Borders::ALL).title("Ani CLI TUI")),
		area,
	);
}

fn draw_search(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let text = vec![
		Line::from("Search anime"),
		Line::from(""),
		Line::from(vec![
			Span::styled("> ", Style::default().fg(Color::Cyan)),
			Span::raw(&app.query),
		]),
	];
	frame.render_widget(
		Paragraph::new(text)
			.block(Block::default().borders(Borders::ALL).title("Search"))
			.wrap(Wrap { trim: false }),
		area,
	);
}

fn draw_results(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let items = app
		.results
		.iter()
		.enumerate()
		.map(|(index, result)| {
			let style = selected_style(index == app.result_index);
			ListItem::new(result.display_title()).style(style)
		})
		.collect::<Vec<_>>();
	frame.render_widget(
		List::new(items)
			.block(Block::default().borders(Borders::ALL).title("Results")),
		area,
	);
}

fn draw_episodes(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let title = app
		.selected_show
		.as_ref()
		.map(AnimeSearchResult::display_title)
		.unwrap_or_else(|| "Episodes".to_owned());
	let items = app
		.episodes
		.iter()
		.enumerate()
		.rev()
		.map(|(index, episode)| {
			let style = selected_style(index == app.episode_index);
			ListItem::new(format!("Episode {episode}")).style(style)
		})
		.collect::<Vec<_>>();
	let mut state = ListState::default();
	if !app.episodes.is_empty() {
		state.select(Some(app.episodes.len() - 1 - app.episode_index));
	}
	frame.render_stateful_widget(
		List::new(items)
			.block(Block::default().borders(Borders::ALL).title(title))
			.highlight_style(
				Style::default()
					.fg(Color::Black)
					.bg(Color::Cyan)
					.add_modifier(Modifier::BOLD),
			),
		area,
		&mut state,
	);
}

fn draw_playing(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let mut lines = vec![
		Line::from(format!(
			"Current: {} episode {}",
			app.selected_show
				.as_ref()
				.map(|show| show.title.as_str())
				.unwrap_or("unknown"),
			app.current_episode.as_deref().unwrap_or("unknown")
		)),
		Line::from(""),
		Line::from(
			"n next  p previous  r replay  e episodes  F2 settings  q quit",
		),
	];
	if let Some(stream) = &app.last_stream {
		if let Some(audio) = &stream.audio_language {
			lines.push(Line::from(format!("Audio: {audio}")));
		}
		if let Some(hardsub) = &stream.hardsub_language {
			lines.push(Line::from(format!("Hard subtitles: {hardsub}")));
		}
		if let Some(subtitle) = &stream.subtitle {
			lines.push(Line::from(format!("External subtitles: {subtitle}")));
		}
	}
	if !app.links.is_empty() {
		lines.push(Line::from(""));
		lines.push(Line::from("Fetched links:"));
		for link in app.links.iter().take(8) {
			lines.push(Line::from(format!(
				"{} {} {}",
				link.quality, link.source, link.url
			)));
		}
	}
	frame.render_widget(
		Paragraph::new(lines)
			.block(Block::default().borders(Borders::ALL).title("Playing"))
			.wrap(Wrap { trim: true }),
		area,
	);
}

fn draw_quality(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let items = quality_choices()
		.into_iter()
		.enumerate()
		.map(|(index, quality)| {
			ListItem::new(quality.to_string())
				.style(selected_style(index == app.quality_index))
		})
		.collect::<Vec<_>>();
	frame.render_widget(
		List::new(items)
			.block(Block::default().borders(Borders::ALL).title("Quality")),
		area,
	);
}

fn draw_language(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let items = language_choices()
		.into_iter()
		.enumerate()
		.map(|(index, mode)| {
			let label = match mode {
				TranslationMode::Sub => "Subtitles (sub)",
				TranslationMode::Dub => "Dubbed audio (dub)",
			};
			ListItem::new(label)
				.style(selected_style(index == app.language_index))
		})
		.collect::<Vec<_>>();
	frame.render_widget(
		List::new(items)
			.block(Block::default().borders(Borders::ALL).title("Language")),
		area,
	);
}

fn draw_track_language(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let title = match app
		.track_language_choices
		.first()
		.map(|choice| choice.kind)
		.unwrap_or(TrackLanguageKind::Subtitle)
	{
		TrackLanguageKind::Subtitle => "Subtitle Language",
		TrackLanguageKind::Hardsub => "Hard Subtitle Language",
		TrackLanguageKind::DubAudio => "Dub Audio Language",
	};
	let items = app
		.track_language_choices
		.iter()
		.enumerate()
		.map(|(index, choice)| {
			ListItem::new(choice.label.clone())
				.style(selected_style(index == app.track_language_index))
		})
		.collect::<Vec<_>>();
	let mut state = ListState::default();
	if !app.track_language_choices.is_empty() {
		state.select(Some(app.track_language_index));
	}
	frame.render_stateful_widget(
		List::new(items)
			.block(Block::default().borders(Borders::ALL).title(title))
			.highlight_style(
				Style::default()
					.fg(Color::Black)
					.bg(Color::Cyan)
					.add_modifier(Modifier::BOLD),
			),
		area,
		&mut state,
	);
}

fn draw_settings(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let items = setting_choices()
		.iter()
		.map(|choice| {
			format!(
				"{}: {}",
				setting_choice_label(*choice),
				current_setting_value_label(app, *choice)
			)
		})
		.into_iter()
		.enumerate()
		.map(|(index, label)| {
			ListItem::new(label)
				.style(selected_style(index == app.settings_index))
		})
		.collect::<Vec<_>>();
	let mut state = ListState::default();
	state.select(Some(app.settings_index));
	frame.render_stateful_widget(
		List::new(items)
			.block(Block::default().borders(Borders::ALL).title("Settings"))
			.highlight_style(
				Style::default()
					.fg(Color::Black)
					.bg(Color::Cyan)
					.add_modifier(Modifier::BOLD),
			),
		area,
		&mut state,
	);
}

fn draw_setting_options(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let Some(choice) = app.active_setting else {
		frame.render_widget(
			Paragraph::new("No setting selected.")
				.block(Block::default().borders(Borders::ALL).title("Setting")),
			area,
		);
		return;
	};

	let items = setting_option_labels(choice)
		.into_iter()
		.enumerate()
		.map(|(index, label)| {
			ListItem::new(label)
				.style(selected_style(index == app.setting_option_index))
		})
		.collect::<Vec<_>>();
	let mut state = ListState::default();
	state.select(Some(app.setting_option_index));
	frame.render_stateful_widget(
		List::new(items)
			.block(
				Block::default()
					.borders(Borders::ALL)
					.title(setting_choice_label(choice)),
			)
			.highlight_style(
				Style::default()
					.fg(Color::Black)
					.bg(Color::Cyan)
					.add_modifier(Modifier::BOLD),
			),
		area,
		&mut state,
	);
}

fn setting_choice_label(choice: SettingChoice) -> &'static str {
	match choice {
		SettingChoice::Language => "Language",
		SettingChoice::Quality => "Quality",
		SettingChoice::DownloadMode => "Download mode",
		SettingChoice::AniSkip => "AniSkip",
	}
}

fn current_setting_value_label(app: &App, choice: SettingChoice) -> String {
	match choice {
		SettingChoice::Language => app.mode.to_string(),
		SettingChoice::Quality => app.quality.to_string(),
		SettingChoice::DownloadMode => on_off(app.download_mode).to_owned(),
		SettingChoice::AniSkip => on_off(app.skip_intro).to_owned(),
	}
}

fn setting_option_labels(choice: SettingChoice) -> Vec<String> {
	match choice {
		SettingChoice::Language => language_choices()
			.into_iter()
			.map(|mode| match mode {
				TranslationMode::Sub => "Subtitles (sub)".to_owned(),
				TranslationMode::Dub => "Dubbed audio (dub)".to_owned(),
			})
			.collect(),
		SettingChoice::Quality => quality_choices()
			.into_iter()
			.map(|quality| quality.to_string())
			.collect(),
		SettingChoice::DownloadMode | SettingChoice::AniSkip => {
			vec!["off".to_owned(), "on".to_owned()]
		}
	}
}

fn on_off(value: bool) -> &'static str {
	if value { "on" } else { "off" }
}

fn draw_help(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let lines = vec![
		Line::from("Global"),
		Line::from("F1 help  F2 settings  Esc back  Ctrl-C quit"),
		Line::from("? help outside search  s settings outside search"),
		Line::from("m language  c quality  d download mode  k AniSkip"),
		Line::from("h history  l logs  i install IINA AniSkip plugin"),
		Line::from(""),
		Line::from("Search"),
		Line::from("Type a title  Enter search"),
		Line::from(""),
		Line::from("Results"),
		Line::from("Up/Down select  Enter episodes  / search"),
		Line::from(""),
		Line::from("Episodes"),
		Line::from("Up/Down select  Enter play  N schedule"),
		Line::from(""),
		Line::from("Playing"),
		Line::from("n next  p previous  r replay  e episodes  q quit"),
		Line::from(""),
		Line::from("Settings"),
		Line::from("Up/Down select setting  Enter open options"),
		Line::from(""),
		Line::from("Setting options"),
		Line::from(
			"Up/Down select option  Enter apply  Esc return to settings",
		),
		Line::from(""),
		Line::from("History"),
		Line::from("Up/Down select  Enter continue  x delete history"),
		Line::from(""),
		Line::from(format!(
			"Settings file: {}",
			app.config.settings_path.display()
		)),
	];
	frame.render_widget(
		Paragraph::new(lines)
			.block(Block::default().borders(Borders::ALL).title("Help"))
			.wrap(Wrap { trim: true }),
		area,
	);
}

fn draw_history(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let items = app
		.history_entries
		.iter()
		.enumerate()
		.map(|(index, entry)| {
			ListItem::new(format!(
				"{} - next after episode {}",
				entry.title, entry.episode
			))
			.style(selected_style(index == app.history_index))
		})
		.collect::<Vec<_>>();
	frame.render_widget(
		List::new(items)
			.block(Block::default().borders(Borders::ALL).title("History")),
		area,
	);
}

fn draw_schedule(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let lines = app
		.schedule
		.iter()
		.flat_map(|status| {
			[
				Line::from(
					status
						.english_title
						.as_deref()
						.or(status.japanese_title.as_deref())
						.unwrap_or("Unknown title")
						.to_owned(),
				),
				Line::from(format!("Status: {}", status.status)),
				Line::from(format!(
					"Next raw: {}",
					status.next_raw_release.as_deref().unwrap_or("-")
				)),
				Line::from(format!(
					"Next sub: {}",
					status.next_sub_release.as_deref().unwrap_or("-")
				)),
				Line::from(""),
			]
		})
		.collect::<Vec<_>>();
	frame.render_widget(
		Paragraph::new(lines)
			.block(Block::default().borders(Borders::ALL).title("Schedule"))
			.wrap(Wrap { trim: true }),
		area,
	);
}

fn draw_text(frame: &mut Frame<'_>, area: Rect, title: &str, text: &str) {
	frame.render_widget(
		Paragraph::new(text.to_owned())
			.block(Block::default().borders(Borders::ALL).title(title))
			.wrap(Wrap { trim: false }),
		area,
	);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let controls = match app.screen {
		Screen::Search => "Enter search | F1 help | F2 settings | Esc quit",
		Screen::Results => {
			"Up/Down select | Enter episodes | / search | F2 settings | Esc back"
		}
		Screen::Episodes => {
			"Up/Down select | Enter play | N schedule | F2 settings | Esc back"
		}
		Screen::Playing => {
			"n/p/r/e playback | F2 settings | h history | l logs | Esc back | q quit"
		}
		Screen::Quality => "Up/Down select | Enter apply | Esc back",
		Screen::Language => "Up/Down select | Enter apply | Esc back",
		Screen::TrackLanguage => {
			"Up/Down select | Enter play | F2 settings | Esc cancel"
		}
		Screen::Settings => {
			"Up/Down select setting | Enter open options | Esc back"
		}
		Screen::SettingOptions => {
			"Up/Down select option | Enter apply | Esc settings"
		}
		Screen::Help => "F2 settings | Esc back",
		Screen::History => {
			"Up/Down select | Enter continue | x delete | F2 settings | Esc back"
		}
		Screen::Logs | Screen::Schedule => "F2 settings | Esc back",
	};
	let lines = vec![
		Line::from(app.status.as_str()),
		Line::from(Span::styled(
			controls,
			Style::default().fg(Color::DarkGray),
		)),
	];
	frame.render_widget(
		Paragraph::new(lines)
			.block(Block::default().borders(Borders::ALL).title("Status")),
		area,
	);
}

fn selected_style(selected: bool) -> Style {
	if selected {
		Style::default()
			.fg(Color::Black)
			.bg(Color::Cyan)
			.add_modifier(Modifier::BOLD)
	} else {
		Style::default()
	}
}
