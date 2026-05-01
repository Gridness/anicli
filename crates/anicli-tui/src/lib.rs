use std::{io, time::Duration};

use anicli_allanime::{
    AllAnimeClient, NextEpisodeStatus, fetch_next_episode_status, select_quality,
};
use anicli_aniskip::{AniSkipClient, MpvSkipOptions, build_mpv_skip_launch, install_iina_plugin};
use anicli_core::{
    AnimeSearchResult, AppConfig, HistoryEntry, HistoryStore, PlayerChoice, QualityPreference,
    SelectedStream, StreamLink, SubtitleTrack, TranslationMode, next_episode, previous_episode,
};
use anicli_player::{PlaybackRequest, PlayerKind, default_player, launch, read_system_logs};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
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
    History,
    Logs,
    Schedule,
}

pub async fn run() -> Result<()> {
    let _terminal = TerminalGuard::enter()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).wrap_err("failed to create terminal")?;
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
    previous_screen: Screen,
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
            previous_screen: Screen::Search,
            query: String::new(),
            mode: config.mode,
            quality: config.quality.clone(),
            skip_intro: config.skip_intro,
            download_mode: matches!(config.player, PlayerChoice::Download),
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
            track_language_index: 0,
            track_language_choices: Vec::new(),
            pending_playback: None,
            status: "Type an anime title and press Enter.".to_owned(),
            quit: false,
            config,
        })
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return Ok(());
        }

        match key.code {
            KeyCode::Esc => {
                if self.screen == Screen::Search {
                    self.quit = true;
                } else {
                    self.screen = Screen::Search;
                    self.status = "Search reset. Type a title and press Enter.".to_owned();
                }
            }
            KeyCode::Char('i') if self.global_shortcuts_enabled() => self.install_iina_plugin()?,
            KeyCode::Char('l') if self.global_shortcuts_enabled() => self.show_logs()?,
            KeyCode::Char('h') if self.global_shortcuts_enabled() => self.show_history()?,
            KeyCode::Char('d') if self.global_shortcuts_enabled() => {
                self.download_mode = !self.download_mode;
                self.status = format!(
                    "Download mode {}.",
                    if self.download_mode {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
            }
            KeyCode::Char('k') if self.global_shortcuts_enabled() => {
                self.skip_intro = !self.skip_intro;
                self.status = format!(
                    "AniSkip {}.",
                    if self.skip_intro {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
            }
            KeyCode::Char('m') if self.global_shortcuts_enabled() => {
                self.previous_screen = self.screen;
                self.screen = Screen::Language;
                self.language_index = language_choices()
                    .iter()
                    .position(|mode| mode == &self.mode)
                    .unwrap_or(0);
            }
            KeyCode::Char('c') if self.global_shortcuts_enabled() => {
                self.previous_screen = self.screen;
                self.screen = Screen::Quality;
                self.quality_index = quality_choices()
                    .iter()
                    .position(|quality| quality == &self.quality)
                    .unwrap_or(0);
            }
            _ => match self.screen {
                Screen::Search => self.handle_search_key(key).await?,
                Screen::Results => self.handle_results_key(key).await?,
                Screen::Episodes => self.handle_episodes_key(key).await?,
                Screen::Playing => self.handle_playing_key(key).await?,
                Screen::Quality => self.handle_quality_key(key)?,
                Screen::Language => self.handle_language_key(key)?,
                Screen::TrackLanguage => self.handle_track_language_key(key).await?,
                Screen::History => self.handle_history_key(key).await?,
                Screen::Logs | Screen::Schedule => self.handle_text_key(key),
            },
        }
        Ok(())
    }

    fn global_shortcuts_enabled(&self) -> bool {
        !matches!(self.screen, Screen::Search | Screen::TrackLanguage)
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
            KeyCode::Up => self.result_index = self.result_index.saturating_sub(1),
            KeyCode::Down => {
                self.result_index =
                    (self.result_index + 1).min(self.results.len().saturating_sub(1))
            }
            KeyCode::Enter => self.select_result().await?,
            KeyCode::Char('/') => self.screen = Screen::Search,
            _ => {}
        }
        Ok(())
    }

    async fn handle_episodes_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Up => {
                self.episode_index =
                    (self.episode_index + 1).min(self.episodes.len().saturating_sub(1))
            }
            KeyCode::Down => {
                self.episode_index = self.episode_index.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(episode) = self.episodes.get(self.episode_index).cloned() {
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
                    if let Some(next) =
                        next_episode(&self.episodes, &current).map(ToOwned::to_owned)
                    {
                        if let Err(err) = self.play_episode(next).await {
                            self.status = format!("{err:#}");
                        }
                    } else {
                        self.status = "Out of range: no next episode.".to_owned();
                    }
                }
            }
            KeyCode::Char('p') => {
                if let Some(current) = self.current_episode.clone() {
                    if let Some(previous) =
                        previous_episode(&self.episodes, &current).map(ToOwned::to_owned)
                    {
                        if let Err(err) = self.play_episode(previous).await {
                            self.status = format!("{err:#}");
                        }
                    } else {
                        self.status = "Out of range: no previous episode.".to_owned();
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
            KeyCode::Char('e') => self.screen = Screen::Episodes,
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
        Ok(())
    }

    fn handle_quality_key(&mut self, key: KeyEvent) -> Result<()> {
        let choices = quality_choices();
        match key.code {
            KeyCode::Up => self.quality_index = self.quality_index.saturating_sub(1),
            KeyCode::Down => {
                self.quality_index = (self.quality_index + 1).min(choices.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                self.quality = choices
                    .get(self.quality_index)
                    .cloned()
                    .unwrap_or(QualityPreference::Best);
                self.screen = self.previous_screen;
                self.status = format!("Quality set to {}.", self.quality);
            }
            KeyCode::Char('b') => self.screen = self.previous_screen,
            _ => {}
        }
        Ok(())
    }

    fn handle_language_key(&mut self, key: KeyEvent) -> Result<()> {
        let choices = language_choices();
        match key.code {
            KeyCode::Up => self.language_index = self.language_index.saturating_sub(1),
            KeyCode::Down => {
                self.language_index = (self.language_index + 1).min(choices.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                let selected = choices
                    .get(self.language_index)
                    .copied()
                    .unwrap_or(TranslationMode::Sub);
                if selected != self.mode {
                    self.mode = selected;
                    self.results.clear();
                    self.episodes.clear();
                    self.selected_show = None;
                    self.current_episode = None;
                    self.status = format!(
                        "Language set to {}. Search again for matching results.",
                        self.mode
                    );
                    self.screen = Screen::Search;
                } else {
                    self.status = format!("Language remains {}.", self.mode);
                    self.screen = self.previous_screen;
                }
            }
            KeyCode::Char('b') => self.screen = self.previous_screen,
            _ => {}
        }
        Ok(())
    }

    async fn handle_track_language_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Up => self.track_language_index = self.track_language_index.saturating_sub(1),
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
                    let selected = apply_track_language_choice(&pending, &choice, &self.quality);
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
            KeyCode::Char('b') => {
                self.pending_playback = None;
                self.track_language_choices.clear();
                self.screen = Screen::Episodes;
                self.status = "Track language selection cancelled.".to_owned();
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_history_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Up => self.history_index = self.history_index.saturating_sub(1),
            KeyCode::Down => {
                self.history_index =
                    (self.history_index + 1).min(self.history_entries.len().saturating_sub(1))
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
            KeyCode::Char('b') => self.screen = Screen::Search,
            _ => {}
        }
        Ok(())
    }

    fn handle_text_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Char('b') | KeyCode::Enter) {
            self.screen = self.previous_screen;
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
            self.status = format!("{} result(s). Select an anime.", self.results.len());
            self.screen = Screen::Results;
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
        self.screen = Screen::Episodes;
        self.status = format!("{} episode(s) available.", self.episodes.len());
        Ok(())
    }

    async fn play_episode(&mut self, episode: String) -> Result<()> {
        let show = self
            .selected_show
            .clone()
            .ok_or_else(|| eyre!("no anime selected"))?;
        self.status = format!("Fetching episode {episode} sources...");

        let filter_soft_subs = matches!(default_player(&self.config.player), PlayerKind::Vlc(_));
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
        }

        let selected = sources.selected.clone();
        self.links = sources.links.clone();
        self.last_stream = Some(selected.clone());

        let choices = track_language_choices(self.mode, &sources.links, &selected);
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
            self.screen = Screen::TrackLanguage;
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
                    self.status = format!("AniSkip unavailable: {err:#}. Playing without it.");
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
        self.screen = Screen::Playing;
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

        build_mpv_skip_launch(&self.aniskip, mal_id, episode, &MpvSkipOptions::default()).await
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
        self.previous_screen = self.screen;
        self.screen = Screen::History;
        self.status = if self.history_entries.is_empty() {
            "History is empty.".to_owned()
        } else {
            "Select an entry to continue, or press x to delete history.".to_owned()
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
        self.episodes = self.allanime.episodes(&entry.anime_id, self.mode).await?;
        let next = next_episode(&self.episodes, &entry.episode)
            .map(ToOwned::to_owned)
            .ok_or_else(|| eyre!("no unwatched episode remains for {}", entry.title))?;
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
        self.screen = Screen::Episodes;
        self.play_episode(next).await
    }

    fn show_logs(&mut self) -> Result<()> {
        self.logs = read_system_logs()?;
        self.previous_screen = self.screen;
        self.screen = Screen::Logs;
        self.status = "Log view. Press b to return.".to_owned();
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
        self.previous_screen = self.screen;
        self.screen = Screen::Schedule;
        self.status = "Schedule loaded. Press b to return.".to_owned();
        Ok(())
    }
}

async fn resolve_skip_query(client: &AniSkipClient, query: String) -> Result<u64> {
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

fn dedupe_track_choices(choices: Vec<TrackLanguageChoice>) -> Vec<TrackLanguageChoice> {
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
                .filter(|link| link.hardsub_language.as_deref() == choice.code.as_deref())
                .cloned()
                .collect::<Vec<_>>(),
            quality,
        )
        .unwrap_or_else(|| pending.selected.clone()),
        TrackLanguageKind::DubAudio => select_quality(
            &pending
                .links
                .iter()
                .filter(|link| link.audio_language.as_deref() == choice.code.as_deref())
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
        Paragraph::new(title).block(Block::default().borders(Borders::ALL).title("Ani CLI TUI")),
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
        List::new(items).block(Block::default().borders(Borders::ALL).title("Results")),
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
        Line::from("n next  p previous  r replay  e episodes  m language  c quality  q quit"),
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
            ListItem::new(quality.to_string()).style(selected_style(index == app.quality_index))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Quality")),
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
            ListItem::new(label).style(selected_style(index == app.language_index))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Language")),
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
        List::new(items).block(Block::default().borders(Borders::ALL).title("History")),
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
        Screen::Search => "Enter search | Esc quit",
        Screen::Results => "Up/Down select | Enter episodes | / search | m language | h history",
        Screen::Episodes => {
            "Up/Down select | Enter play | N schedule | m language | c quality | d download | k skip"
        }
        Screen::Playing => {
            "n/p/r playback | e episodes | m language | c quality | h history | l logs | i install IINA skip | q quit"
        }
        Screen::Quality => "Up/Down select | Enter apply | b back",
        Screen::Language => "Up/Down select | Enter apply | b back",
        Screen::TrackLanguage => "Up/Down select | Enter play | b cancel",
        Screen::History => "Up/Down select | Enter continue | x delete | b back",
        Screen::Logs | Screen::Schedule => "b back",
    };
    let lines = vec![
        Line::from(app.status.as_str()),
        Line::from(Span::styled(controls, Style::default().fg(Color::DarkGray))),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Status")),
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
