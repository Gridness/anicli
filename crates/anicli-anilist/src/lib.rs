use std::{
	collections::HashSet,
	fs,
	path::{Path, PathBuf},
	process::Command,
	time::Duration,
};

use eyre::{Result, WrapErr, eyre};
use reqwest::{Client, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;

const GRAPHQL_ENDPOINT: &str = "https://graphql.anilist.co";
const AUTHORIZE_ENDPOINT: &str = "https://anilist.co/api/v2/oauth/authorize";
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AniListAuth {
	pub client_id: Option<String>,
	pub access_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AniListAuthStore {
	path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AniListAuthFile {
	#[serde(default)]
	client_id: Option<String>,
	#[serde(default)]
	access_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AniListClient {
	http: Client,
	access_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AniListViewer {
	pub id: u32,
	pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AniListMediaListStatus {
	#[serde(rename = "CURRENT")]
	Current,
	#[serde(rename = "PLANNING")]
	Planning,
	#[serde(rename = "COMPLETED")]
	Completed,
	#[serde(rename = "DROPPED")]
	Dropped,
	#[serde(rename = "PAUSED")]
	Paused,
	#[serde(rename = "REPEATING")]
	Repeating,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AniListMediaListEntry {
	pub id: u32,
	pub media_id: u32,
	pub status: AniListMediaListStatus,
	pub progress: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AniListMedia {
	pub id: u32,
	pub mal_id: Option<u32>,
	pub title: String,
	pub titles: Vec<String>,
	pub episodes: Option<u32>,
	pub list_entry: Option<AniListMediaListEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AniListListEntry {
	pub list_entry_id: u32,
	pub media_id: u32,
	pub mal_id: Option<u32>,
	pub title: String,
	pub titles: Vec<String>,
	pub status: AniListMediaListStatus,
	pub progress: u32,
	pub episodes: Option<u32>,
	pub list_name: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlResponse<T> {
	data: Option<T>,
	#[serde(default)]
	errors: Vec<GraphqlError>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
	message: String,
	#[serde(default)]
	status: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct ViewerData {
	#[serde(rename = "Viewer")]
	viewer: AniListViewer,
}

#[derive(Debug, Deserialize)]
struct MediaListCollectionData {
	#[serde(rename = "MediaListCollection")]
	collection: Option<MediaListCollectionRaw>,
}

#[derive(Debug, Deserialize)]
struct MediaListCollectionRaw {
	#[serde(default)]
	lists: Vec<MediaListGroupRaw>,
}

#[derive(Debug, Deserialize)]
struct MediaListGroupRaw {
	name: String,
	status: Option<AniListMediaListStatus>,
	#[serde(default)]
	entries: Vec<MediaListEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct MediaListEntryRaw {
	id: u32,
	#[serde(rename = "mediaId")]
	media_id: u32,
	status: AniListMediaListStatus,
	progress: u32,
	media: AniListMediaRaw,
}

#[derive(Debug, Deserialize)]
struct MediaData {
	#[serde(rename = "Media")]
	media: Option<AniListMediaRaw>,
}

#[derive(Debug, Deserialize)]
struct MediaSearchData {
	#[serde(rename = "Page")]
	page: MediaSearchPage,
}

#[derive(Debug, Deserialize)]
struct MediaSearchPage {
	#[serde(default)]
	media: Vec<AniListMediaRaw>,
}

#[derive(Debug, Deserialize)]
struct AniListMediaRaw {
	id: u32,
	#[serde(rename = "idMal")]
	mal_id: Option<u32>,
	episodes: Option<u32>,
	title: AniListTitleRaw,
	#[serde(rename = "mediaListEntry")]
	list_entry: Option<MediaListEntryCompactRaw>,
}

#[derive(Debug, Deserialize)]
struct MediaListEntryCompactRaw {
	id: u32,
	#[serde(rename = "mediaId")]
	media_id: u32,
	status: AniListMediaListStatus,
	progress: u32,
}

#[derive(Debug, Deserialize)]
struct AniListTitleRaw {
	#[serde(rename = "userPreferred")]
	user_preferred: Option<String>,
	romaji: Option<String>,
	english: Option<String>,
	native: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SaveMediaListEntryData {
	#[serde(rename = "SaveMediaListEntry")]
	entry: MediaListEntryCompactRaw,
}

impl AniListAuthStore {
	pub fn new(path: PathBuf) -> Self {
		Self { path }
	}

	pub fn path(&self) -> &Path {
		&self.path
	}

	pub fn load(&self) -> Result<AniListAuth> {
		if !self.path.exists() {
			return Ok(AniListAuth::default());
		}
		let contents = fs::read_to_string(&self.path).wrap_err_with(|| {
			format!("failed to read {}", self.path.display())
		})?;
		let file: AniListAuthFile =
			toml::from_str(&contents).wrap_err_with(|| {
				format!("failed to parse {}", self.path.display())
			})?;
		Ok(AniListAuth {
			client_id: clean_optional(file.client_id),
			access_token: clean_optional(file.access_token),
		})
	}

	pub fn save(&self, auth: &AniListAuth) -> Result<()> {
		if let Some(parent) = self.path.parent() {
			fs::create_dir_all(parent).wrap_err_with(|| {
				format!("failed to create {}", parent.display())
			})?;
		}
		let file = AniListAuthFile {
			client_id: auth.client_id.clone(),
			access_token: auth.access_token.clone(),
		};
		let contents = toml::to_string_pretty(&file)
			.wrap_err("failed to serialize AniList auth")?;
		fs::write(&self.path, format!("{contents}\n")).wrap_err_with(|| {
			format!("failed to write {}", self.path.display())
		})
	}
}

impl AniListClient {
	pub fn new(access_token: Option<String>) -> Result<Self> {
		let mut headers = header::HeaderMap::new();
		headers.insert(
			header::ACCEPT,
			header::HeaderValue::from_static("application/json"),
		);
		headers.insert(
			header::CONTENT_TYPE,
			header::HeaderValue::from_static("application/json"),
		);
		let http = Client::builder()
			.default_headers(headers)
			.connect_timeout(CONNECT_TIMEOUT)
			.timeout(HTTP_TIMEOUT)
			.build()
			.wrap_err("failed to build AniList HTTP client")?;
		Ok(Self { http, access_token })
	}

	pub fn set_access_token(&mut self, access_token: Option<String>) {
		self.access_token = access_token;
	}

	pub fn is_authenticated(&self) -> bool {
		self.access_token.is_some()
	}

	pub async fn viewer(&self) -> Result<AniListViewer> {
		let data: ViewerData = self
			.post_graphql(
				r#"query { Viewer { id name } }"#,
				serde_json::Value::Null,
			)
			.await?;
		Ok(data.viewer)
	}

	pub async fn anime_list(
		&self,
		user_id: u32,
	) -> Result<Vec<AniListListEntry>> {
		let data: MediaListCollectionData = self
			.post_graphql(
				r#"
query ($userId: Int!) {
  MediaListCollection(userId: $userId, type: ANIME) {
    lists {
      name
      status
      entries {
        id
        mediaId
        status
        progress
        media {
          id
          idMal
          episodes
          title {
            userPreferred
            romaji
            english
            native
          }
        }
      }
    }
  }
}
"#,
				json!({ "userId": user_id }),
			)
			.await?;
		let Some(collection) = data.collection else {
			return Ok(Vec::new());
		};

		let mut seen = HashSet::new();
		let mut entries = Vec::new();
		for group in collection.lists {
			for entry in group.entries {
				if !seen.insert(entry.media_id) {
					continue;
				}
				let media = entry.media;
				let titles = media.title.titles();
				let title = media.title.display_title();
				entries.push(AniListListEntry {
					list_entry_id: entry.id,
					media_id: entry.media_id,
					mal_id: media.mal_id,
					title,
					titles,
					status: group.status.unwrap_or(entry.status),
					progress: entry.progress,
					episodes: media.episodes,
					list_name: group.name.clone(),
				});
			}
		}
		entries.sort_by(|left, right| {
			status_order(left.status)
				.cmp(&status_order(right.status))
				.then_with(|| left.title.cmp(&right.title))
		});
		Ok(entries)
	}

	pub async fn resolve_media(
		&self,
		mal_id: Option<u32>,
		search: &str,
	) -> Result<Option<AniListMedia>> {
		if let Some(mal_id) = mal_id
			&& let Some(media) = self.media_by_mal_id(mal_id).await?
		{
			return Ok(Some(media));
		}
		self.search_media(search).await
	}

	pub async fn media_by_mal_id(
		&self,
		mal_id: u32,
	) -> Result<Option<AniListMedia>> {
		let data: MediaData = self
			.post_graphql(
				r#"
query ($idMal: Int!) {
  Media(idMal: $idMal, type: ANIME) {
    id
    idMal
    episodes
    title {
      userPreferred
      romaji
      english
      native
    }
    mediaListEntry {
      id
      mediaId
      status
      progress
    }
  }
}
"#,
				json!({ "idMal": mal_id }),
			)
			.await?;
		Ok(data.media.map(AniListMedia::from))
	}

	pub async fn search_media(
		&self,
		search: &str,
	) -> Result<Option<AniListMedia>> {
		let data: MediaSearchData = self
			.post_graphql(
				r#"
query ($search: String!) {
  Page(page: 1, perPage: 1) {
    media(search: $search, type: ANIME, sort: SEARCH_MATCH) {
      id
      idMal
      episodes
      title {
        userPreferred
        romaji
        english
        native
      }
      mediaListEntry {
        id
        mediaId
        status
        progress
      }
    }
  }
}
"#,
				json!({ "search": search }),
			)
			.await?;
		Ok(data.page.media.into_iter().next().map(AniListMedia::from))
	}

	pub async fn save_progress(
		&self,
		media_id: u32,
		progress: u32,
	) -> Result<AniListMediaListEntry> {
		let data: SaveMediaListEntryData = self
			.post_graphql(
				r#"
mutation ($mediaId: Int!, $progress: Int!, $status: MediaListStatus!) {
  SaveMediaListEntry(mediaId: $mediaId, progress: $progress, status: $status) {
    id
    mediaId
    status
    progress
  }
}
"#,
				json!({
					"mediaId": media_id,
					"progress": progress,
					"status": AniListMediaListStatus::Current.as_str(),
				}),
			)
			.await?;
		Ok(data.entry.into())
	}

	async fn post_graphql<T>(
		&self,
		query: &str,
		variables: serde_json::Value,
	) -> Result<T>
	where
		T: DeserializeOwned,
	{
		let mut request = self.http.post(GRAPHQL_ENDPOINT).json(&json!({
			"query": query,
			"variables": variables,
		}));
		if let Some(token) = &self.access_token {
			request = request.bearer_auth(token);
		}

		let response =
			request.send().await.wrap_err("failed to contact AniList")?;
		let status = response.status();
		let text = response
			.text()
			.await
			.wrap_err("failed to read AniList response")?;
		if !status.is_success() {
			return Err(eyre!("AniList returned {status}: {text}"));
		}

		let response: GraphqlResponse<T> =
			serde_json::from_str(&text).wrap_err("invalid AniList JSON")?;
		if !response.errors.is_empty() {
			let messages = response
				.errors
				.into_iter()
				.map(|error| match error.status {
					Some(status) => format!("{} ({status})", error.message),
					None => error.message,
				})
				.collect::<Vec<_>>()
				.join("; ");
			return Err(eyre!("AniList GraphQL error: {messages}"));
		}
		response
			.data
			.ok_or_else(|| eyre!("AniList response did not include data"))
	}
}

impl AniListMediaListStatus {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Current => "CURRENT",
			Self::Planning => "PLANNING",
			Self::Completed => "COMPLETED",
			Self::Dropped => "DROPPED",
			Self::Paused => "PAUSED",
			Self::Repeating => "REPEATING",
		}
	}
}

impl std::fmt::Display for AniListMediaListStatus {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let label = match self {
			Self::Current => "watching",
			Self::Planning => "planning",
			Self::Completed => "completed",
			Self::Dropped => "dropped",
			Self::Paused => "paused",
			Self::Repeating => "rewatching",
		};
		f.write_str(label)
	}
}

impl AniListListEntry {
	pub fn display_title(&self) -> String {
		match self.episodes {
			Some(episodes) => format!(
				"{} - {} - episode {}/{}",
				self.title, self.status, self.progress, episodes
			),
			None => format!(
				"{} - {} - episode {}",
				self.title, self.status, self.progress
			),
		}
	}

	pub fn progress_ref(&self) -> AniListProgressRef {
		AniListProgressRef {
			media_id: self.media_id,
			progress: self.progress,
			title: self.title.clone(),
			status: self.status,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AniListProgressRef {
	pub media_id: u32,
	pub progress: u32,
	pub title: String,
	pub status: AniListMediaListStatus,
}

impl From<AniListMediaRaw> for AniListMedia {
	fn from(media: AniListMediaRaw) -> Self {
		let title = media.title.display_title();
		let titles = media.title.titles();
		Self {
			id: media.id,
			mal_id: media.mal_id,
			title,
			titles,
			episodes: media.episodes,
			list_entry: media.list_entry.map(Into::into),
		}
	}
}

impl From<MediaListEntryCompactRaw> for AniListMediaListEntry {
	fn from(entry: MediaListEntryCompactRaw) -> Self {
		Self {
			id: entry.id,
			media_id: entry.media_id,
			status: entry.status,
			progress: entry.progress,
		}
	}
}

impl AniListTitleRaw {
	fn display_title(&self) -> String {
		self.user_preferred
			.as_deref()
			.or(self.english.as_deref())
			.or(self.romaji.as_deref())
			.or(self.native.as_deref())
			.unwrap_or("Untitled")
			.to_owned()
	}

	fn titles(&self) -> Vec<String> {
		let mut titles = Vec::new();
		for title in [
			self.user_preferred.as_deref(),
			self.english.as_deref(),
			self.romaji.as_deref(),
			self.native.as_deref(),
		]
		.into_iter()
		.flatten()
		{
			let title = title.trim();
			if !title.is_empty()
				&& !titles.iter().any(|existing| existing == title)
			{
				titles.push(title.to_owned());
			}
		}
		titles
	}
}

pub fn authorization_url(client_id: &str) -> String {
	format!(
		"{AUTHORIZE_ENDPOINT}?client_id={}&response_type=token",
		urlencoding::encode(client_id.trim())
	)
}

pub fn open_browser(url: &str) -> Result<()> {
	let mut command = if cfg!(target_os = "macos") {
		let mut command = Command::new("open");
		command.arg(url);
		command
	} else if cfg!(target_os = "windows") {
		let mut command = Command::new("cmd");
		command.args(["/C", "start", "", url]);
		command
	} else {
		let mut command = Command::new("xdg-open");
		command.arg(url);
		command
	};
	command
		.spawn()
		.wrap_err("failed to open the AniList login URL in a browser")?;
	Ok(())
}

pub fn extract_access_token(input: &str) -> Option<String> {
	let trimmed = input.trim();
	if trimmed.is_empty() {
		return None;
	}
	for section in fragment_or_query_sections(trimmed) {
		for pair in section.split('&') {
			let mut fields = pair.splitn(2, '=');
			let key = fields.next().unwrap_or_default();
			let value = fields.next().unwrap_or_default();
			if key == "access_token" && !value.is_empty() {
				return Some(
					urlencoding::decode(value)
						.map(|value| value.into_owned())
						.unwrap_or_else(|_| value.to_owned()),
				);
			}
		}
	}
	Some(trimmed.to_owned())
}

fn fragment_or_query_sections(input: &str) -> Vec<&str> {
	let mut sections = Vec::new();
	if let Some((_, fragment)) = input.split_once('#') {
		sections.push(fragment);
	}
	if let Some((_, query)) = input.split_once('?') {
		sections.push(query);
	}
	sections.push(input);
	sections
}

fn clean_optional(value: Option<String>) -> Option<String> {
	value
		.map(|value| value.trim().to_owned())
		.filter(|value| !value.is_empty())
}

fn status_order(status: AniListMediaListStatus) -> usize {
	match status {
		AniListMediaListStatus::Current => 0,
		AniListMediaListStatus::Repeating => 1,
		AniListMediaListStatus::Planning => 2,
		AniListMediaListStatus::Paused => 3,
		AniListMediaListStatus::Dropped => 4,
		AniListMediaListStatus::Completed => 5,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn extracts_token_from_redirect_fragment() {
		let token = extract_access_token(
			"https://anilist.co/api/v2/oauth/pin#access_token=abc.def&token_type=Bearer",
		);

		assert_eq!(token.as_deref(), Some("abc.def"));
	}

	#[test]
	fn accepts_raw_token() {
		assert_eq!(
			extract_access_token("abc.def.ghi").as_deref(),
			Some("abc.def.ghi")
		);
	}
}
