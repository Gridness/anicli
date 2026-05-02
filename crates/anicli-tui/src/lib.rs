use std::{
	io,
	time::{Duration, Instant},
};

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
	PlaybackOutcome, PlaybackRequest, PlayerKind, default_player,
	is_iina_running, launch, read_system_logs,
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
	widgets::{
		Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap,
	},
};
use tokio::{
	sync::mpsc,
	task::{self, JoinHandle},
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
	Loading,
	IinaClosed,
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
		app.drain_playback_events();
		app.update_iina_monitor();
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
	loading: Option<LoadingState>,
	playback_rx: Option<mpsc::UnboundedReceiver<PlaybackEvent>>,
	playback_task: Option<JoinHandle<()>>,
	iina_monitor: Option<IinaMonitor>,
	iina_closed_index: usize,
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

#[derive(Debug, Clone)]
struct LoadingState {
	title: String,
	subject: String,
	detail: String,
	stage: usize,
	total_stages: usize,
	notes: Vec<String>,
	started: Instant,
	return_screen: Screen,
}

#[derive(Debug, Clone)]
struct LoadingProgress {
	detail: String,
	stage: usize,
	note: Option<String>,
}

#[derive(Debug, Clone)]
struct IinaMonitor {
	started: Instant,
	last_checked: Instant,
	seen_running: bool,
	episode: String,
}

#[derive(Debug)]
enum PlaybackEvent {
	Progress(LoadingProgress),
	Ready(PlaybackReady),
	Error(String),
}

#[derive(Debug)]
enum PlaybackReady {
	TrackLanguage {
		pending: PendingPlayback,
		choices: Vec<TrackLanguageChoice>,
	},
	Launched {
		show: AnimeSearchResult,
		episode: String,
		links: Vec<StreamLink>,
		selected: SelectedStream,
		status: String,
		return_screen: Screen,
		monitor_iina: bool,
	},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IinaClosedAction {
	Reopen,
	NextEpisode,
	SelectEpisode,
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
			loading: None,
			playback_rx: None,
			playback_task: None,
			iina_monitor: None,
			iina_closed_index: 0,
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

		if self.screen == Screen::Loading {
			if key.code == KeyCode::Esc {
				self.cancel_loading();
			}
			return Ok(());
		}

		if self.screen == Screen::IinaClosed {
			self.handle_iina_closed_key(key)?;
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
			Screen::Loading => {}
			Screen::IinaClosed => {}
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
				| Screen::Loading
				| Screen::IinaClosed
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
				if !self.episodes.is_empty() {
					self.episode_index =
						if self.episode_index == self.episodes.len() - 1 {
							0
						} else {
							self.episode_index + 1
						};
				}
			}
			KeyCode::Down => {
				if !self.episodes.is_empty() {
					self.episode_index = if self.episode_index == 0 {
						self.episodes.len() - 1
					} else {
						self.episode_index - 1
					};
				}
			}
			KeyCode::Enter => {
				if let Some(episode) =
					self.episodes.get(self.episode_index).cloned()
				{
					if let Err(err) = self.begin_play_episode(episode) {
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
						if let Err(err) = self.begin_play_episode(next) {
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
						if let Err(err) = self.begin_play_episode(previous) {
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
					if let Err(err) = self.begin_play_episode(current) {
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

	fn handle_iina_closed_key(&mut self, key: KeyEvent) -> Result<()> {
		match key.code {
			KeyCode::Up => {
				self.iina_closed_index =
					self.iina_closed_index.saturating_sub(1);
			}
			KeyCode::Down => {
				self.iina_closed_index = (self.iina_closed_index + 1)
					.min(iina_closed_actions().len().saturating_sub(1));
			}
			KeyCode::Enter => {
				match iina_closed_actions()
					.get(self.iina_closed_index)
					.copied()
					.unwrap_or(IinaClosedAction::Reopen)
				{
					IinaClosedAction::Reopen => self.reopen_iina()?,
					IinaClosedAction::NextEpisode => {
						self.play_next_after_iina_closed()?
					}
					IinaClosedAction::SelectEpisode => {
						self.select_episode_after_iina_closed();
					}
				}
			}
			KeyCode::Esc => {
				self.screen = Screen::Playing;
				self.status =
					"IINA is closed. Playback actions remain available."
						.to_owned();
			}
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
					self.begin_launch_episode(
						pending.show,
						pending.episode,
						pending.links,
						selected,
						pending.playback_config,
						Screen::Episodes,
					)?;
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
		self.iina_monitor = None;
		self.nav_stack.clear();
		self.screen = Screen::Search;
	}

	fn cleanup_screen(&mut self, screen: Screen) {
		if screen == Screen::TrackLanguage {
			self.pending_playback = None;
			self.track_language_choices.clear();
			self.status = "Track language selection cancelled.".to_owned();
		} else if screen == Screen::Loading {
			self.cancel_loading();
		} else if screen == Screen::SettingOptions {
			self.active_setting = None;
			self.setting_option_index = 0;
		}
	}

	fn drain_playback_events(&mut self) {
		let mut events = Vec::new();
		let mut disconnected = false;
		if let Some(rx) = &mut self.playback_rx {
			loop {
				match rx.try_recv() {
					Ok(event) => events.push(event),
					Err(mpsc::error::TryRecvError::Empty) => break,
					Err(mpsc::error::TryRecvError::Disconnected) => {
						disconnected = true;
						break;
					}
				}
			}
		}

		if disconnected && events.is_empty() {
			events.push(PlaybackEvent::Error(
				"Episode loading stopped before it finished.".to_owned(),
			));
		}

		for event in events {
			self.handle_playback_event(event);
		}
	}

	fn handle_playback_event(&mut self, event: PlaybackEvent) {
		match event {
			PlaybackEvent::Progress(progress) => {
				if let Some(loading) = &mut self.loading {
					loading.stage =
						progress.stage.clamp(1, loading.total_stages);
					loading.detail = progress.detail.clone();
					if let Some(note) = progress.note {
						if loading.notes.last() != Some(&note) {
							loading.notes.push(note);
						}
						if loading.notes.len() > 5 {
							loading.notes.remove(0);
						}
					}
				}
				self.status = progress.detail;
			}
			PlaybackEvent::Ready(ready) => self.handle_playback_ready(ready),
			PlaybackEvent::Error(err) => {
				let return_screen = self
					.loading
					.as_ref()
					.map(|loading| loading.return_screen)
					.unwrap_or(Screen::Episodes);
				self.finish_playback_task();
				self.screen = return_screen;
				self.status = err;
			}
		}
	}

	fn handle_playback_ready(&mut self, ready: PlaybackReady) {
		match ready {
			PlaybackReady::TrackLanguage { pending, choices } => {
				self.finish_playback_task();
				self.links = pending.links.clone();
				self.last_stream = Some(pending.selected.clone());
				self.track_language_index = 0;
				self.track_language_choices = choices;
				self.pending_playback = Some(pending);
				self.screen = Screen::TrackLanguage;
				self.status =
					"Select track language for this episode.".to_owned();
			}
			PlaybackReady::Launched {
				show,
				episode,
				links,
				selected,
				status,
				return_screen,
				monitor_iina,
			} => {
				self.finish_playback_task();
				self.selected_show = Some(show);
				self.links = links;
				self.last_stream = Some(selected);
				self.current_episode = Some(episode.clone());
				self.pending_playback = None;
				self.track_language_choices.clear();
				if return_screen != Screen::Playing
					&& self.nav_stack.last() != Some(&return_screen)
				{
					self.nav_stack.push(return_screen);
				}
				self.screen = Screen::Playing;
				self.status = status;
				if monitor_iina {
					self.arm_iina_monitor(episode);
				}
			}
		}
	}

	fn start_loading(
		&mut self,
		return_screen: Screen,
		title: impl Into<String>,
		subject: impl Into<String>,
		detail: impl Into<String>,
	) {
		let detail = detail.into();
		self.loading = Some(LoadingState {
			title: title.into(),
			subject: subject.into(),
			detail: detail.clone(),
			stage: 1,
			total_stages: 5,
			notes: vec![detail],
			started: Instant::now(),
			return_screen,
		});
		self.screen = Screen::Loading;
	}

	fn abort_playback_task(&mut self) {
		if let Some(task) = self.playback_task.take() {
			task.abort();
		}
		self.playback_rx = None;
		self.loading = None;
	}

	fn finish_playback_task(&mut self) {
		self.playback_rx = None;
		self.playback_task = None;
		self.loading = None;
	}

	fn cancel_loading(&mut self) {
		let return_screen = self
			.loading
			.as_ref()
			.map(|loading| loading.return_screen)
			.unwrap_or(Screen::Episodes);
		self.abort_playback_task();
		self.screen = return_screen;
		self.status = "Episode loading cancelled.".to_owned();
	}

	fn update_iina_monitor(&mut self) {
		let Some(monitor) = &mut self.iina_monitor else {
			return;
		};
		if monitor.last_checked.elapsed() < Duration::from_secs(1) {
			return;
		}
		monitor.last_checked = Instant::now();

		if is_iina_running() {
			monitor.seen_running = true;
			return;
		}

		if monitor.seen_running
			|| monitor.started.elapsed() >= Duration::from_secs(8)
		{
			let episode = monitor.episode.clone();
			self.iina_monitor = None;
			self.iina_closed_index = 0;
			self.screen = Screen::IinaClosed;
			self.status = format!(
				"IINA is closed after episode {episode}. Choose what to do next."
			);
		}
	}

	fn arm_iina_monitor(&mut self, episode: String) {
		let now = Instant::now();
		self.iina_monitor = Some(IinaMonitor {
			started: now,
			last_checked: now,
			seen_running: false,
			episode,
		});
	}

	fn reopen_iina(&mut self) -> Result<()> {
		let show = self
			.selected_show
			.clone()
			.ok_or_else(|| eyre!("no anime selected"))?;
		let episode = self
			.current_episode
			.clone()
			.ok_or_else(|| eyre!("no episode is currently selected"))?;
		let selected = self
			.last_stream
			.clone()
			.ok_or_else(|| eyre!("no previous stream is available"))?;
		let mut playback_config = self.config.clone();
		playback_config.quality = self.quality.clone();
		playback_config.player = PlayerChoice::Iina;
		self.iina_monitor = None;
		self.begin_launch_episode(
			show,
			episode,
			self.links.clone(),
			selected,
			playback_config,
			Screen::Playing,
		)
	}

	fn play_next_after_iina_closed(&mut self) -> Result<()> {
		let current = self
			.current_episode
			.clone()
			.ok_or_else(|| eyre!("no episode is currently selected"))?;
		let next = next_episode(&self.episodes, &current)
			.map(ToOwned::to_owned)
			.ok_or_else(|| eyre!("out of range: no next episode"))?;
		self.iina_monitor = None;
		self.begin_play_episode(next)
	}

	fn select_episode_after_iina_closed(&mut self) {
		self.iina_monitor = None;
		self.screen = Screen::Episodes;
		self.status =
			"Select an episode from the list and press Enter.".to_owned();
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
			self.iina_monitor = None;
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

	fn begin_play_episode(&mut self, episode: String) -> Result<()> {
		let show = self
			.selected_show
			.clone()
			.ok_or_else(|| eyre!("no anime selected"))?;
		let filter_soft_subs = !self.download_mode
			&& matches!(
				default_player(&self.config.player),
				PlayerKind::Vlc(_)
			);
		let mut playback_config = self.config.clone();
		playback_config.quality = self.quality.clone();
		if self.download_mode {
			playback_config.player = PlayerChoice::Download;
		} else if matches!(playback_config.player, PlayerChoice::Download) {
			playback_config.player = PlayerChoice::Auto;
		}

		let return_screen = match self.screen {
			Screen::Playing => Screen::Playing,
			Screen::TrackLanguage => Screen::Episodes,
			_ => Screen::Episodes,
		};
		self.iina_monitor = None;
		self.abort_playback_task();
		let (tx, rx) = mpsc::unbounded_channel();
		self.playback_rx = Some(rx);
		self.start_loading(
			return_screen,
			"Loading Episode",
			episode_summary(&show, &episode),
			"Requesting episode source list from AllAnime.",
		);
		self.status = format!(
			"Loading {}. The stream providers are being checked in parallel.",
			episode_summary(&show, &episode)
		);

		let task = tokio::spawn(load_episode_task(LoadEpisodeRequest {
			allanime: self.allanime.clone(),
			aniskip: self.aniskip.clone(),
			history: self.history.clone(),
			show,
			episode,
			mode: self.mode,
			quality: self.quality.clone(),
			filter_soft_subs,
			playback_config,
			skip_intro: self.skip_intro,
			return_screen,
			tx,
		}));
		self.playback_task = Some(task);
		Ok(())
	}

	fn begin_launch_episode(
		&mut self,
		show: AnimeSearchResult,
		episode: String,
		links: Vec<StreamLink>,
		selected: SelectedStream,
		playback_config: AppConfig,
		return_screen: Screen,
	) -> Result<()> {
		self.abort_playback_task();
		self.iina_monitor = None;
		let (tx, rx) = mpsc::unbounded_channel();
		self.playback_rx = Some(rx);
		let return_screen = match return_screen {
			Screen::TrackLanguage => Screen::Episodes,
			screen => screen,
		};
		self.start_loading(
			return_screen,
			"Starting Player",
			episode_summary(&show, &episode),
			"Preparing the selected stream for playback.",
		);
		self.status = format!(
			"Preparing {} for playback.",
			episode_summary(&show, &episode)
		);

		let task = tokio::spawn(launch_episode_task(LaunchEpisodeRequest {
			allanime: self.allanime.clone(),
			aniskip: self.aniskip.clone(),
			history: self.history.clone(),
			show,
			episode,
			links,
			selected,
			playback_config,
			skip_intro: self.skip_intro,
			return_screen,
			tx,
		}));
		self.playback_task = Some(task);
		Ok(())
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
		self.begin_play_episode(next)
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

struct LoadEpisodeRequest {
	allanime: AllAnimeClient,
	aniskip: AniSkipClient,
	history: HistoryStore,
	show: AnimeSearchResult,
	episode: String,
	mode: TranslationMode,
	quality: QualityPreference,
	filter_soft_subs: bool,
	playback_config: AppConfig,
	skip_intro: bool,
	return_screen: Screen,
	tx: mpsc::UnboundedSender<PlaybackEvent>,
}

struct LaunchEpisodeRequest {
	allanime: AllAnimeClient,
	aniskip: AniSkipClient,
	history: HistoryStore,
	show: AnimeSearchResult,
	episode: String,
	links: Vec<StreamLink>,
	selected: SelectedStream,
	playback_config: AppConfig,
	skip_intro: bool,
	return_screen: Screen,
	tx: mpsc::UnboundedSender<PlaybackEvent>,
}

async fn load_episode_task(request: LoadEpisodeRequest) {
	let tx = request.tx.clone();
	let event = match load_episode_task_inner(request).await {
		Ok(ready) => PlaybackEvent::Ready(ready),
		Err(err) => PlaybackEvent::Error(format!("{err:#}")),
	};
	let _ = tx.send(event);
}

async fn load_episode_task_inner(
	request: LoadEpisodeRequest,
) -> Result<PlaybackReady> {
	let summary = episode_summary(&request.show, &request.episode);
	send_progress(
		&request.tx,
		1,
		format!("Requesting source list for {summary}."),
		Some("Contacting AllAnime for the episode source list.".to_owned()),
	);

	let sources = request
		.allanime
		.episode_sources(
			&request.show.id,
			request.mode,
			&request.episode,
			&request.quality,
			request.filter_soft_subs,
		)
		.await
		.wrap_err("failed to load episode streams")?;

	send_progress(
		&request.tx,
		3,
		format!(
			"Found {} stream option(s) for {summary}.",
			sources.links.len()
		),
		Some(
			"Stream providers responded; selecting the requested quality."
				.to_owned(),
		),
	);

	let selected = sources.selected.clone();
	let choices =
		track_language_choices(request.mode, &sources.links, &selected);
	if choices.len() > 1 {
		send_progress(
			&request.tx,
			4,
			format!("Multiple track languages are available for {summary}."),
			Some("Waiting for your track language choice.".to_owned()),
		);
		return Ok(PlaybackReady::TrackLanguage {
			pending: PendingPlayback {
				show: request.show,
				episode: request.episode,
				links: sources.links,
				selected,
				playback_config: request.playback_config,
			},
			choices,
		});
	}

	launch_episode_task_inner(LaunchEpisodeRequest {
		allanime: request.allanime,
		aniskip: request.aniskip,
		history: request.history,
		show: request.show,
		episode: request.episode,
		links: sources.links,
		selected,
		playback_config: request.playback_config,
		skip_intro: request.skip_intro,
		return_screen: request.return_screen,
		tx: request.tx,
	})
	.await
}

async fn launch_episode_task(request: LaunchEpisodeRequest) {
	let tx = request.tx.clone();
	let event = match launch_episode_task_inner(request).await {
		Ok(ready) => PlaybackEvent::Ready(ready),
		Err(err) => PlaybackEvent::Error(format!("{err:#}")),
	};
	let _ = tx.send(event);
}

async fn launch_episode_task_inner(
	request: LaunchEpisodeRequest,
) -> Result<PlaybackReady> {
	let summary = episode_summary(&request.show, &request.episode);
	send_progress(
		&request.tx,
		4,
		format!("Preparing player command for {summary}."),
		Some(format!(
			"Selected {}.",
			selected_stream_summary(&request.selected)
		)),
	);

	let mut playback_request = PlaybackRequest::from_config(
		&request.playback_config,
		request.show.media_title_prefix(),
		request.episode.clone(),
		request.selected.clone(),
	);

	if request.skip_intro
		&& !matches!(request.playback_config.player, PlayerChoice::Download)
	{
		send_progress(
			&request.tx,
			4,
			format!("Looking up AniSkip markers for {summary}."),
			Some("Opening and ending skip markers are optional; playback will continue if unavailable.".to_owned()),
		);
		match prepare_skip_for_episode(
			&request.allanime,
			&request.aniskip,
			&request.playback_config,
			&request.show,
			&request.episode,
		)
		.await
		{
			Ok(skip) => {
				playback_request.skip = Some(skip);
				send_progress(
					&request.tx,
					4,
					format!("AniSkip markers are ready for {summary}."),
					Some(
						"Skip markers will be passed to the player.".to_owned(),
					),
				);
			}
			Err(err) => {
				send_progress(
					&request.tx,
					4,
					format!("AniSkip unavailable for {summary}; continuing."),
					Some(format!("AniSkip skipped: {err:#}")),
				);
			}
		}
	}

	let player = player_label(&playback_request.player);
	let monitor_iina = matches!(playback_request.player, PlayerKind::Iina(_));
	send_progress(
		&request.tx,
		5,
		format!("{player} is being launched for {summary}."),
		Some(
			"The stream URL is ready; handing it to the player now.".to_owned(),
		),
	);

	let launch_request = playback_request.clone();
	let outcome = task::spawn_blocking(move || launch(&launch_request))
		.await
		.wrap_err("player launcher task failed")??;
	let status = playback_status(
		&request.show,
		&request.episode,
		&request.selected,
		&playback_request,
		&outcome,
	);

	let history = request.history.clone();
	let history_entry = HistoryEntry {
		episode: request.episode.clone(),
		anime_id: request.show.id.clone(),
		title: request.show.display_title(),
	};
	task::spawn_blocking(move || history.upsert(history_entry))
		.await
		.wrap_err("history writer task failed")??;

	Ok(PlaybackReady::Launched {
		show: request.show,
		episode: request.episode,
		links: request.links,
		selected: request.selected,
		status,
		return_screen: request.return_screen,
		monitor_iina,
	})
}

async fn prepare_skip_for_episode(
	allanime: &AllAnimeClient,
	aniskip: &AniSkipClient,
	config: &AppConfig,
	show: &AnimeSearchResult,
	episode: &str,
) -> Result<anicli_aniskip::SkipLaunch> {
	let mal_id = match allanime.mal_id(&show.id).await.ok().flatten() {
		Some(mal_id) => mal_id,
		None => {
			let query = config
				.skip_title
				.as_deref()
				.unwrap_or(&show.title)
				.to_owned();
			resolve_skip_query(aniskip, query).await?
		}
	};

	build_mpv_skip_launch(aniskip, mal_id, episode, &MpvSkipOptions::default())
		.await
}

fn send_progress(
	tx: &mpsc::UnboundedSender<PlaybackEvent>,
	stage: usize,
	detail: impl Into<String>,
	note: Option<String>,
) {
	let _ = tx.send(PlaybackEvent::Progress(LoadingProgress {
		detail: detail.into(),
		stage,
		note,
	}));
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

fn iina_closed_actions() -> &'static [IinaClosedAction] {
	&[
		IinaClosedAction::Reopen,
		IinaClosedAction::NextEpisode,
		IinaClosedAction::SelectEpisode,
	]
}

fn iina_closed_action_label(action: IinaClosedAction) -> &'static str {
	match action {
		IinaClosedAction::Reopen => "Reopen IINA",
		IinaClosedAction::NextEpisode => "Play next episode",
		IinaClosedAction::SelectEpisode => "Select episode",
	}
}

fn iina_closed_action_description(action: IinaClosedAction) -> &'static str {
	match action {
		IinaClosedAction::Reopen => {
			"Launch the current stream in IINA again without refetching sources."
		}
		IinaClosedAction::NextEpisode => {
			"Load the next available episode and start playback."
		}
		IinaClosedAction::SelectEpisode => {
			"Return to the episode list and choose a specific episode."
		}
	}
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
		Screen::Loading => draw_loading(frame, chunks[1], app),
		Screen::IinaClosed => draw_iina_closed(frame, chunks[1], app),
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
		Line::from(format!("Current: {}", current_episode_summary(app))),
		Line::from(""),
		Line::from(
			"n next  p previous  r replay  e episodes  F2 settings  q quit",
		),
	];
	if let Some(stream) = &app.last_stream {
		lines.push(Line::from(format!(
			"Stream: {}",
			selected_stream_summary(stream)
		)));
	}
	if !app.links.is_empty() {
		lines.push(Line::from(""));
		lines.push(Line::from("Available streams:"));
		for link in app.links.iter().take(8) {
			lines.push(Line::from(format!("- {}", stream_link_summary(link))));
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

fn draw_loading(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let Some(loading) = &app.loading else {
		draw_text(frame, area, "Loading", "Preparing episode...");
		return;
	};
	let elapsed = loading.started.elapsed();
	let chunks = Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Length(6),
			Constraint::Length(3),
			Constraint::Min(4),
		])
		.split(area);

	let header = vec![
		Line::from(vec![
			Span::styled(
				spinner_frame(elapsed),
				Style::default()
					.fg(Color::Cyan)
					.add_modifier(Modifier::BOLD),
			),
			Span::raw(" "),
			Span::styled(
				&loading.subject,
				Style::default().add_modifier(Modifier::BOLD),
			),
		]),
		Line::from(""),
		Line::from(loading.detail.as_str()),
		Line::from(format!("Elapsed: {}s", elapsed.as_secs())),
	];
	frame.render_widget(
		Paragraph::new(header)
			.block(
				Block::default()
					.borders(Borders::ALL)
					.title(loading.title.as_str()),
			)
			.wrap(Wrap { trim: true }),
		chunks[0],
	);

	let ratio = loading.stage as f64 / loading.total_stages.max(1) as f64;
	frame.render_widget(
		Gauge::default()
			.block(Block::default().borders(Borders::ALL).title("Progress"))
			.gauge_style(
				Style::default()
					.fg(Color::Cyan)
					.bg(Color::Black)
					.add_modifier(Modifier::BOLD),
			)
			.label(format!(
				"stage {} of {}",
				loading.stage, loading.total_stages
			))
			.ratio(ratio.clamp(0.0, 1.0)),
		chunks[1],
	);

	let notes = loading
		.notes
		.iter()
		.rev()
		.map(|note| Line::from(format!("- {note}")))
		.collect::<Vec<_>>();
	frame.render_widget(
		Paragraph::new(notes)
			.block(
				Block::default()
					.borders(Borders::ALL)
					.title("What is happening"),
			)
			.wrap(Wrap { trim: true }),
		chunks[2],
	);
}

fn draw_iina_closed(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let chunks = Layout::default()
		.direction(Direction::Vertical)
		.constraints([Constraint::Length(6), Constraint::Min(5)])
		.split(area);
	let title = app
		.selected_show
		.as_ref()
		.map(|show| show.title.as_str())
		.unwrap_or("Unknown title");
	let episode = app.current_episode.as_deref().unwrap_or("unknown");
	let next = app
		.current_episode
		.as_deref()
		.and_then(|current| next_episode(&app.episodes, current))
		.map(|episode| format!("Episode {episode}"))
		.unwrap_or_else(|| "No next episode available".to_owned());
	let summary = vec![
		Line::from(vec![
			Span::styled(
				"IINA is closed",
				Style::default()
					.fg(Color::Yellow)
					.add_modifier(Modifier::BOLD),
			),
			Span::raw(format!(" after {title} episode {episode}.")),
		]),
		Line::from(""),
		Line::from("Choose how to continue playback."),
		Line::from(format!("Next: {next}")),
	];
	frame.render_widget(
		Paragraph::new(summary)
			.block(
				Block::default()
					.borders(Borders::ALL)
					.title("Playback Paused"),
			)
			.wrap(Wrap { trim: true }),
		chunks[0],
	);

	let items = iina_closed_actions()
		.iter()
		.map(|action| {
			ListItem::new(vec![
				Line::from(Span::styled(
					iina_closed_action_label(*action),
					Style::default().add_modifier(Modifier::BOLD),
				)),
				Line::from(Span::styled(
					iina_closed_action_description(*action),
					Style::default().fg(Color::DarkGray),
				)),
			])
		})
		.collect::<Vec<_>>();
	let mut state = ListState::default();
	state.select(Some(app.iina_closed_index));
	frame.render_stateful_widget(
		List::new(items)
			.block(Block::default().borders(Borders::ALL).title("Continue"))
			.highlight_style(
				Style::default()
					.fg(Color::Black)
					.bg(Color::Cyan)
					.add_modifier(Modifier::BOLD),
			),
		chunks[1],
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

fn spinner_frame(elapsed: Duration) -> &'static str {
	const FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
	let index = (elapsed.as_millis() / 120) as usize % FRAMES.len();
	FRAMES[index]
}

fn playback_status(
	show: &AnimeSearchResult,
	episode: &str,
	stream: &SelectedStream,
	request: &PlaybackRequest,
	outcome: &PlaybackOutcome,
) -> String {
	let episode = episode_summary(show, episode);
	let stream = selected_stream_summary(stream);
	match &request.player {
		PlayerKind::Download => match outcome.exit_code {
			Some(0) => format!(
				"Download finished: {episode} ({stream}) in {}.",
				request.download_dir.display()
			),
			Some(code) => format!(
				"Download exited with code {code}: {episode} ({stream}) in {}.",
				request.download_dir.display()
			),
			None => format!(
				"Download started: {episode} ({stream}) in {}.",
				request.download_dir.display()
			),
		},
		PlayerKind::Debug => {
			format!("Debug playback ready: {episode} ({stream}).")
		}
		player if outcome.detached => format!(
			"{} will now launch shortly: {episode} ({stream}).",
			player_label(player)
		),
		player => match outcome.exit_code {
			Some(0) => {
				format!(
					"{} finished: {episode} ({stream}).",
					player_label(player)
				)
			}
			Some(code) => format!(
				"{} exited with code {code}: {episode} ({stream}).",
				player_label(player)
			),
			None => {
				format!(
					"{} launched: {episode} ({stream}).",
					player_label(player)
				)
			}
		},
	}
}

fn player_label(player: &PlayerKind) -> String {
	match player {
		PlayerKind::Iina(_) => "IINA".to_owned(),
		PlayerKind::Mpv(_) => "mpv".to_owned(),
		PlayerKind::Vlc(_) => "VLC".to_owned(),
		PlayerKind::Syncplay(_) => "Syncplay".to_owned(),
		PlayerKind::Custom(program) => program.to_owned(),
		PlayerKind::Download => "Download".to_owned(),
		PlayerKind::Debug => "Debug".to_owned(),
	}
}

fn current_episode_summary(app: &App) -> String {
	let title = app
		.selected_show
		.as_ref()
		.map(|show| show.title.as_str())
		.unwrap_or("unknown");
	let episode = app.current_episode.as_deref().unwrap_or("unknown");
	format!("{title} episode {episode}")
}

fn episode_summary(show: &AnimeSearchResult, episode: &str) -> String {
	format!("{} episode {}", show.title, episode)
}

fn selected_stream_summary(stream: &SelectedStream) -> String {
	let mut parts = stream_parts(&stream.quality, &stream.source);
	if let Some(audio) = &stream.audio_language {
		parts.push(format!("audio: {audio}"));
	}
	if let Some(hardsub) = &stream.hardsub_language {
		parts.push(format!("hard subtitles: {hardsub}"));
	}
	if let Some(subtitle) = selected_subtitle_label(stream) {
		parts.push(format!("subtitles: {subtitle}"));
	}
	parts.join(", ")
}

fn stream_link_summary(link: &StreamLink) -> String {
	let mut parts = stream_parts(&link.quality, &link.source);
	if let Some(audio) = &link.audio_language {
		parts.push(format!("audio: {audio}"));
	}
	if let Some(hardsub) = &link.hardsub_language {
		parts.push(format!("hard subtitles: {hardsub}"));
	}
	if let Some(subtitles) = subtitle_labels(&link.subtitles) {
		parts.push(format!("subtitles: {subtitles}"));
	} else if link.subtitle.is_some() {
		parts.push("external subtitles".to_owned());
	} else if link.soft_subbed {
		parts.push("soft subtitles".to_owned());
	}
	parts.join(", ")
}

fn stream_parts(quality: &str, source: &str) -> Vec<String> {
	let mut parts = Vec::new();
	if quality.is_empty() {
		parts.push("unknown quality".to_owned());
	} else {
		parts.push(format!("{quality} quality"));
	}
	if !source.is_empty() {
		parts.push(format!("source: {source}"));
	}
	parts
}

fn selected_subtitle_label(stream: &SelectedStream) -> Option<String> {
	let subtitle = stream.subtitle.as_deref()?;
	stream
		.subtitles
		.iter()
		.find(|track| track.url == subtitle)
		.map(SubtitleTrack::display_label)
		.or_else(|| Some("selected track".to_owned()))
}

fn subtitle_labels(tracks: &[SubtitleTrack]) -> Option<String> {
	if tracks.is_empty() {
		return None;
	}
	let mut labels = tracks
		.iter()
		.take(3)
		.map(SubtitleTrack::display_label)
		.collect::<Vec<_>>();
	if tracks.len() > labels.len() {
		labels.push(format!("+{} more", tracks.len() - labels.len()));
	}
	Some(labels.join(", "))
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
		Screen::Loading => "Loading episode | Esc cancel",
		Screen::IinaClosed => "Up/Down select | Enter continue | Esc dismiss",
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

#[cfg(test)]
mod tests {
	use std::path::PathBuf;

	use super::*;

	#[test]
	fn iina_launch_status_is_human_readable_without_stream_url() {
		let show = AnimeSearchResult {
			id: "demo".to_owned(),
			title: "Demo Show".to_owned(),
			episode_count: Some(12),
		};
		let stream = SelectedStream {
			quality: "1080p".to_owned(),
			url: "https://stream.example/video.m3u8".to_owned(),
			source: "AllAnime".to_owned(),
			referrer: None,
			subtitle: Some("https://subs.example/en.vtt".to_owned()),
			subtitles: vec![SubtitleTrack {
				lang: "en".to_owned(),
				label: "English".to_owned(),
				url: "https://subs.example/en.vtt".to_owned(),
			}],
			hardsub_language: None,
			audio_language: None,
		};
		let request = PlaybackRequest {
			title: "Demo Show".to_owned(),
			episode: "7".to_owned(),
			stream: stream.clone(),
			player: PlayerKind::Iina("iina".to_owned()),
			download_dir: PathBuf::from("/tmp"),
			no_detach: false,
			exit_after_play: false,
			log_episode: false,
			skip: None,
		};
		let outcome = PlaybackOutcome {
			command: "iina https://stream.example/video.m3u8".to_owned(),
			detached: true,
			exit_code: None,
		};

		let status = playback_status(&show, "7", &stream, &request, &outcome);

		assert_eq!(
			status,
			"IINA will now launch shortly: Demo Show episode 7 (1080p quality, source: AllAnime, subtitles: English (en))."
		);
		assert!(!status.contains("https://"));
	}
}
