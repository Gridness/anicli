# anicli-rs

`anicli-rs` is a Rust/Ratatui implementation of the current `ani-cli`
workflow. It searches AllAnime, lets you pick a result and episode in a TUI,
fetches playable sources, launches the configured player, stores watch history,
can sync AniList progress, and can integrate AniSkip timestamps for OP/ED
skipping.

The workspace is split by responsibility:

- `anicli-core`: config, shared models, history, quality, episode helpers.
- `anicli-allanime`: AllAnime GraphQL client, provider decoding, link parsing,
  quality selection, next-episode schedule lookup.
- `anicli-anilist`: AniList OAuth helpers, GraphQL client, list retrieval, and
  progress updates.
- `anicli-aniskip`: MAL lookup, AniSkip API client, mpv skip script/chapter
  generation, IINA plugin installer.
- `anicli-player`: IINA/mpv/VLC/syncplay/download/debug launchers.
- `anicli-tui`: Ratatui application and key handling.
- `anicli`: binary entry point.

## Usage

```sh
cargo run -p anicli
```

Useful keys:

- Search screen: type a title, `Enter` searches.
- Results: `Up`/`Down`, `Enter` selects an anime, `/` returns to search.
- Episodes: `Up`/`Down`, `Enter` plays, `N` shows next episode schedule.
- Playing: `n` next, `p` previous, `r` replay, `e` episode list.
- Help/settings: `F1` opens all shortcuts, `F2` opens settings from any screen.
- AniList: `F3` opens AniList login or your AniList anime lists.
- AniList lists: `f` opens list filter options, including all lists.
- Settings: `Enter` opens a list of values for the selected setting; `Esc`
  returns to Settings from that list.
- Global outside search: `c` quality, `m` language, `d` download mode,
  `k` AniSkip, `h` history, `l` logs, `i` install IINA AniSkip plugin.
- `Esc` returns to the previous menu; at the root search screen it quits.
- `Ctrl-C` quits.

Mode, quality, download mode, and AniSkip settings are saved as TOML in the
machine's standard config directory as reported by the `dirs` crate.

AniList login is browser-based. Press `F3`, enter an AniList API client ID, log
in in the browser, then paste the returned access token or redirect URL into the
TUI. For a local CLI client, configure the AniList app redirect URL as
`https://anilist.co/api/v2/oauth/pin`. The app stores AniList auth in
`anilist.toml` next to `settings.toml`; `ANI_CLI_ANILIST_CLIENT_ID` and
`ANI_CLI_ANILIST_TOKEN` can override it.

When logged in, selecting a title resolves it against AniList using the AllAnime
MAL id when available, places the episode selector on the last watched AniList
episode, and syncs playback progress with status `CURRENT`. Starting playback
from planning, paused, dropped, completed, or a missing AniList entry moves or
adds that anime to watching. Without AniList login, local history is retained
for the most recent 1000 watched episodes and is used to place the episode
selector.

When an episode exposes multiple subtitle, hardsub, or dub-audio languages, a
second language picker appears before playback so you can choose the concrete
track, for example English or Russian subtitles.

`anicli --help` and `anicli --version` are intentionally minimal so package
managers can test the binary without opening the TUI.

## Player Behavior

On macOS the default player is IINA, using
`/Applications/IINA.app/Contents/MacOS/iina-cli` when present and falling back
to `iina`. Other systems default to mpv, with flatpak mpv detected when
available. `ANI_CLI_PLAYER` can override this with `iina`, `mpv`, `vlc`,
`syncplay`, `download`, `debug`, or a custom command.

AniSkip support is built in. For mpv and IINA CLI launches, the app fetches
AniSkip timestamps and passes a generated Lua script plus chapter metadata to
the player. The `i` key also installs a native IINA plugin under
`~/Library/Application Support/com.colliderli.iina/plugins` and enables IINA's
plugin system; restart IINA if it was open.

## Environment

The app follows the upstream `ani-cli` environment names where they map cleanly:

- `ANI_CLI_MODE=sub|dub`
- `ANI_CLI_QUALITY=best|worst|360|480|720|1080`
- `ANI_CLI_DOWNLOAD_DIR`
- `ANI_CLI_HIST_DIR`
- `ANI_CLI_PLAYER`
- `ANI_CLI_SKIP_INTRO=1`
- `ANI_CLI_SKIP_TITLE`
- `ANI_CLI_NO_DETACH=1`
- `ANI_CLI_EXIT_AFTER_PLAY=1`
- `ANI_CLI_LOG=0|1`
- `ANI_CLI_ANILIST_CLIENT_ID`
- `ANI_CLI_ANILIST_TOKEN`

## Development

```sh
cargo fmt --all
cargo test --workspace
```
