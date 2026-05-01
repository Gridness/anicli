use std::{cmp::Reverse, collections::HashMap};

use aes::Aes256;
use aes_gcm::{
	Aes256Gcm, Nonce,
	aead::{Aead, KeyInit},
};
use anicli_core::{
	AnimeSearchResult, QualityPreference, SelectedStream, StreamLink,
	SubtitleTrack, TranslationMode, episode_key,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ctr::cipher::{KeyIvInit, StreamCipher};
use eyre::{Context, Result, eyre};
use regex::Regex;
use reqwest::{Client, header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Aes256Ctr = ctr::Ctr128BE<Aes256>;

const DEFAULT_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/121.0";
const DEFAULT_REFERER: &str = "https://allmanga.to";
const EPISODE_REFERER: &str = "https://youtu-chan.com/";
const DEFAULT_BASE: &str = "allanime.day";
const ALLANIME_KEY_SEED: &str = "Xot36i3lK3:v1";

#[derive(Debug, Clone)]
pub struct AllAnimeEndpoints {
	pub referer: String,
	pub base: String,
	pub api: String,
}

impl Default for AllAnimeEndpoints {
	fn default() -> Self {
		Self {
			referer: DEFAULT_REFERER.to_owned(),
			base: DEFAULT_BASE.to_owned(),
			api: format!("https://api.{DEFAULT_BASE}"),
		}
	}
}

#[derive(Debug, Clone)]
pub struct AllAnimeClient {
	http: Client,
	endpoints: AllAnimeEndpoints,
}

#[derive(Debug, Clone)]
struct SourceRef {
	name: String,
	path: String,
}

#[derive(Debug, Clone, Default)]
struct ProviderLinkMeta {
	quality: Option<String>,
	url: String,
	referrer: Option<String>,
	subtitles: Vec<SubtitleTrack>,
	hardsub_language: Option<String>,
	audio_language: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EpisodeSources {
	pub links: Vec<StreamLink>,
	pub selected: SelectedStream,
}

impl AllAnimeClient {
	pub fn new() -> Result<Self> {
		let mut headers = header::HeaderMap::new();
		headers.insert(
			header::USER_AGENT,
			header::HeaderValue::from_static(DEFAULT_AGENT),
		);
		let http = Client::builder()
			.default_headers(headers)
			.build()
			.wrap_err("failed to build AllAnime HTTP client")?;
		Ok(Self {
			http,
			endpoints: AllAnimeEndpoints::default(),
		})
	}

	pub fn with_endpoints(endpoints: AllAnimeEndpoints) -> Result<Self> {
		let mut client = Self::new()?;
		client.endpoints = endpoints;
		Ok(client)
	}

	pub async fn search(
		&self,
		query: &str,
		mode: TranslationMode,
	) -> Result<Vec<AnimeSearchResult>> {
		let query_text = r#"query( $search: SearchInput $limit: Int $page: Int $translationType: VaildTranslationTypeEnumType $countryOrigin: VaildCountryOriginEnumType ) { shows( search: $search limit: $limit page: $page translationType: $translationType countryOrigin: $countryOrigin ) { edges { _id name availableEpisodes __typename } }}"#;
		let body = json!({
			"variables": {
				"search": {
					"allowAdult": false,
					"allowUnknown": false,
					"query": query,
				},
				"limit": 40,
				"page": 1,
				"translationType": mode.as_allanime(),
				"countryOrigin": "ALL",
			},
			"query": query_text,
		});

		let value = self.post_graphql(body).await?;
		let edges = value
			.pointer("/data/shows/edges")
			.and_then(Value::as_array)
			.ok_or_else(|| {
				eyre!("AllAnime search response did not include show edges")
			})?;

		Ok(edges
			.iter()
			.filter_map(|edge| {
				let id = edge.get("_id")?.as_str()?.to_owned();
				let title = edge.get("name")?.as_str()?.replace("\\\"", "");
				let episode_count = edge
					.pointer(&format!(
						"/availableEpisodes/{}",
						mode.as_allanime()
					))
					.and_then(Value::as_u64)
					.and_then(|count| u32::try_from(count).ok());
				Some(AnimeSearchResult {
					id,
					title,
					episode_count,
				})
			})
			.collect())
	}

	pub async fn episodes(
		&self,
		show_id: &str,
		mode: TranslationMode,
	) -> Result<Vec<String>> {
		let query_text = r#"query ($showId: String!) { show( _id: $showId ) { _id availableEpisodesDetail }}"#;
		let body = json!({
			"variables": {
				"showId": show_id,
			},
			"query": query_text,
		});

		let value = self.post_graphql(body).await?;
		let episodes = value
			.pointer(&format!(
				"/data/show/availableEpisodesDetail/{}",
				mode.as_allanime()
			))
			.and_then(Value::as_array)
			.ok_or_else(|| {
				eyre!("AllAnime response did not include episode details")
			})?;

		let mut result = episodes
			.iter()
			.filter_map(|episode| match episode {
				Value::Number(number) => Some(number.to_string()),
				Value::String(value) => Some(value.clone()),
				_ => None,
			})
			.collect::<Vec<_>>();

		result.sort_by(|a, b| episode_key(a).total_cmp(&episode_key(b)));
		Ok(result)
	}

	pub async fn mal_id(&self, show_id: &str) -> Result<Option<u64>> {
		let body = json!({
			"query": format!("{{ show(_id: \"{}\") {{ malId }} }}", show_id.replace('"', "\\\"")),
		});
		let value = self.post_graphql(body).await?;
		Ok(value.pointer("/data/show/malId").and_then(|value| {
			value
				.as_u64()
				.or_else(|| value.as_str().and_then(|value| value.parse().ok()))
		}))
	}

	pub async fn episode_sources(
		&self,
		show_id: &str,
		mode: TranslationMode,
		episode: &str,
		quality: &QualityPreference,
		filter_soft_subs: bool,
	) -> Result<EpisodeSources> {
		let refs = self.source_refs(show_id, mode, episode).await?;
		let mut links = Vec::new();
		for source_ref in refs {
			match self.fetch_provider_links(&source_ref).await {
				Ok(mut provider_links) => links.append(&mut provider_links),
				Err(err) => {
					let fallback = StreamLink {
						quality: "error".to_owned(),
						url: format!("{err:#}"),
						source: source_ref.name,
						referrer: None,
						subtitle: None,
						subtitles: Vec::new(),
						hardsub_language: None,
						audio_language: None,
						soft_subbed: false,
					};
					links.push(fallback);
				}
			}
		}

		links.retain(|link| link.quality != "error");
		if filter_soft_subs {
			links.retain(|link| !link.soft_subbed);
		}
		links.sort_by_key(|link| (Reverse(link.score()), link.quality.clone()));

		let selected = select_quality(&links, quality).ok_or_else(|| {
			eyre!("episode is released, but no valid sources were found")
		})?;

		Ok(EpisodeSources { links, selected })
	}

	async fn source_refs(
		&self,
		show_id: &str,
		mode: TranslationMode,
		episode: &str,
	) -> Result<Vec<SourceRef>> {
		let query_text = r#"query ($showId: String!, $translationType: VaildTranslationTypeEnumType!, $episodeString: String!) { episode( showId: $showId translationType: $translationType episodeString: $episodeString ) { episodeString sourceUrls }}"#;
		let variables = json!({
				"showId": show_id,
				"translationType": mode.as_allanime(),
				"episodeString": episode,
		});
		let body = json!({
			"variables": variables.to_string(),
			"query": query_text,
		});

		let response = self
			.post_graphql_text_with_referer(body, EPISODE_REFERER)
			.await?;
		if response.contains("\"tobeparsed\"") {
			let refs = decode_tobeparsed_from_response(&response)?;
			if !refs.is_empty() {
				return Ok(refs);
			}
		}

		let value: Value = serde_json::from_str(&response)
			.wrap_err("invalid AllAnime JSON")?;
		let source_refs = value
			.pointer("/data/episode/sourceUrls")
			.or_else(|| find_key(&value, "sourceUrls"))
			.map(source_refs_from_value)
			.unwrap_or_default();

		if !source_refs.is_empty() {
			return Ok(source_refs);
		}

		let source_refs = source_refs_from_text(&response);
		if !source_refs.is_empty() {
			return Ok(source_refs);
		}

		if let Some(errors) = graphql_errors(&value) {
			return Err(eyre!("AllAnime source lookup failed: {errors}"));
		}

		Err(eyre!(
			"AllAnime did not return provider sources for episode {episode}"
		))
	}

	async fn fetch_provider_links(
		&self,
		source_ref: &SourceRef,
	) -> Result<Vec<StreamLink>> {
		let source = normalize_provider_name(&source_ref.name);
		let url = if source_ref.path.starts_with("http") {
			source_ref.path.clone()
		} else {
			format!("https://{}{}", self.endpoints.base, source_ref.path)
		};

		let response = self
			.http
			.get(&url)
			.header(header::REFERER, &self.endpoints.referer)
			.send()
			.await
			.wrap_err_with(|| {
				format!("failed to fetch provider endpoint {url}")
			})?
			.error_for_status()
			.wrap_err_with(|| {
				format!("provider endpoint returned an error for {url}")
			})?
			.text()
			.await
			.wrap_err("failed to read provider response")?
			.replace("\\u002F", "/")
			.replace("\\/", "/")
			.replace("\\\"", "\"");

		if response.contains("repackager.wixmp.com") {
			return Ok(expand_wixmp_links(&response, &source));
		}
		if response.contains("master.m3u8") {
			return self.expand_hls_links(&response, &source).await;
		}

		let mut links = parse_direct_links(&response, &source);
		if source_ref.path.contains("tools.fast4speed.rsvp") {
			links.push(StreamLink {
				quality: "Yt".to_owned(),
				url: source_ref.path.clone(),
				source,
				referrer: Some(self.endpoints.referer.clone()),
				subtitle: None,
				subtitles: Vec::new(),
				hardsub_language: None,
				audio_language: None,
				soft_subbed: false,
			});
		}

		Ok(links)
	}

	async fn expand_hls_links(
		&self,
		response: &str,
		source: &str,
	) -> Result<Vec<StreamLink>> {
		let mut links = Vec::new();
		let mut metas = provider_link_metadata(response)
			.into_iter()
			.filter(|meta| meta.url.contains(".m3u8"))
			.collect::<Vec<_>>();
		if metas.is_empty() {
			metas.push(ProviderLinkMeta {
				url: hls_url(response)
					.ok_or_else(|| eyre!("HLS URL was not found"))?,
				referrer: referer_from_text(response),
				subtitles: subtitle_tracks_from_text(response),
				..ProviderLinkMeta::default()
			});
		}

		let resolution_re =
			Regex::new(r#"RESOLUTION=\d+x(\d+)"#).expect("valid regex");
		for meta in metas {
			let playlist = self
				.http
				.get(&meta.url)
				.header(
					header::REFERER,
					meta.referrer.as_deref().unwrap_or(&self.endpoints.referer),
				)
				.send()
				.await
				.wrap_err_with(|| {
					format!("failed to fetch playlist {}", meta.url)
				})?
				.error_for_status()
				.wrap_err("playlist request failed")?
				.text()
				.await
				.wrap_err("failed to read playlist")?;

			if !playlist.contains("#EXTM3U") {
				links.push(stream_from_provider_meta(meta, source));
				continue;
			}

			let base_url = meta
				.url
				.rsplit_once('/')
				.map(|(prefix, _)| format!("{prefix}/"))
				.unwrap_or_default();
			let mut pending_quality = None::<String>;
			for line in playlist.lines() {
				if line.starts_with("#EXT-X-STREAM") {
					pending_quality = Some(
						resolution_re
							.captures(line)
							.and_then(|captures| captures.get(1))
							.map(|capture| format!("{}p", capture.as_str()))
							.unwrap_or_else(|| {
								meta.quality
									.clone()
									.unwrap_or_else(|| "hls".to_owned())
							}),
					);
					continue;
				}
				if line.starts_with('#')
					|| line.trim().is_empty()
					|| line.contains("I-FRAME")
				{
					continue;
				}
				if let Some(quality) = pending_quality.take() {
					let url = if line.starts_with("http") {
						line.to_owned()
					} else {
						format!("{base_url}{line}")
					};
					links.push(StreamLink {
						quality,
						url,
						source: source.to_owned(),
						referrer: meta.referrer.clone(),
						subtitle: meta
							.subtitles
							.first()
							.map(|track| track.url.clone()),
						subtitles: meta.subtitles.clone(),
						hardsub_language: meta.hardsub_language.clone(),
						audio_language: meta.audio_language.clone(),
						soft_subbed: !meta.subtitles.is_empty(),
					});
				}
			}
		}

		Ok(links)
	}

	async fn post_graphql(&self, body: Value) -> Result<Value> {
		let text = self.post_graphql_text(body).await?;
		serde_json::from_str(&text).wrap_err("invalid AllAnime JSON response")
	}

	async fn post_graphql_text(&self, body: Value) -> Result<String> {
		self.post_graphql_text_with_referer(body, &self.endpoints.referer)
			.await
	}

	async fn post_graphql_text_with_referer(
		&self,
		body: Value,
		referer: &str,
	) -> Result<String> {
		self.http
			.post(format!("{}/api", self.endpoints.api))
			.header(header::REFERER, referer)
			.header(header::CONTENT_TYPE, "application/json")
			.json(&body)
			.send()
			.await
			.wrap_err("failed to contact AllAnime")?
			.error_for_status()
			.wrap_err("AllAnime returned an error")?
			.text()
			.await
			.wrap_err("failed to read AllAnime response")
	}
}

pub fn select_quality(
	links: &[StreamLink],
	quality: &QualityPreference,
) -> Option<SelectedStream> {
	match quality {
		QualityPreference::Best => links.first().cloned(),
		QualityPreference::Worst => links
			.iter()
			.rev()
			.find(|link| link.score() > 0)
			.cloned()
			.or_else(|| links.last().cloned()),
		QualityPreference::Exact(value) => links
			.iter()
			.find(|link| link.quality.contains(value))
			.cloned()
			.or_else(|| links.first().cloned()),
	}
	.map(SelectedStream::from)
}

fn decode_tobeparsed_from_response(response: &str) -> Result<Vec<SourceRef>> {
	let blob = Regex::new(r#""tobeparsed":"([^"]+)""#)
		.expect("valid regex")
		.captures(response)
		.and_then(|captures| captures.get(1))
		.map(|capture| capture.as_str())
		.ok_or_else(|| eyre!("encrypted source blob was not found"))?;
	let bytes = STANDARD
		.decode(blob)
		.wrap_err("failed to base64-decode encrypted source blob")?;
	if bytes.len() <= 29 {
		return Err(eyre!("encrypted source blob is too short"));
	}

	let plain = decode_tobeparsed_gcm(&bytes)
		.or_else(|_| decode_tobeparsed_ctr(&bytes))?;

	if let Ok(value) = serde_json::from_str::<Value>(&plain) {
		let refs = source_refs_from_value(&value);
		if !refs.is_empty() {
			return Ok(refs);
		}
	}

	Ok(source_refs_from_text(&plain))
}

fn decode_tobeparsed_gcm(bytes: &[u8]) -> Result<String> {
	let key = Sha256::digest(ALLANIME_KEY_SEED.as_bytes());
	let cipher = Aes256Gcm::new_from_slice(&key)
		.wrap_err("failed to initialize AES-GCM")?;
	let nonce = Nonce::from_slice(&bytes[1..13]);
	let decrypted = cipher.decrypt(nonce, &bytes[13..]).map_err(|_| {
		eyre!("failed to decrypt AllAnime source blob as AES-GCM")
	})?;
	String::from_utf8(decrypted)
		.wrap_err("decrypted AllAnime source blob is not UTF-8")
}

fn decode_tobeparsed_ctr(bytes: &[u8]) -> Result<String> {
	let key = Sha256::digest(ALLANIME_KEY_SEED.as_bytes());
	let mut iv = [0u8; 16];
	iv[..12].copy_from_slice(&bytes[1..13]);
	iv[15] = 2;

	let end = bytes.len() - 16;
	let mut ciphertext = bytes[13..end].to_vec();
	let mut cipher = Aes256Ctr::new(&key, &iv.into());
	cipher.apply_keystream(&mut ciphertext);
	String::from_utf8(ciphertext)
		.wrap_err("decrypted AllAnime source blob is not UTF-8")
}

pub fn decode_provider_path(encoded: &str) -> Result<String> {
	let mut decoded = String::with_capacity(encoded.len() / 2);
	for pair in encoded.as_bytes().chunks(2) {
		if pair.len() != 2 {
			return Err(eyre!("provider path has an odd number of hex digits"));
		}
		let hex = std::str::from_utf8(pair)
			.wrap_err("provider path is not valid UTF-8")?;
		let byte = u8::from_str_radix(hex, 16)
			.wrap_err("provider path is not hex encoded")?;
		decoded.push((byte ^ 0x38) as char);
	}
	Ok(decoded.replace("/clock", "/clock.json"))
}

fn graphql_errors(value: &Value) -> Option<String> {
	let errors = value.get("errors")?.as_array()?;
	let messages = errors
		.iter()
		.filter_map(|error| error.get("message").and_then(Value::as_str))
		.collect::<Vec<_>>();
	(!messages.is_empty()).then(|| messages.join(", "))
}

fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
	match value {
		Value::Object(map) => map
			.get(key)
			.or_else(|| map.values().find_map(|value| find_key(value, key))),
		Value::Array(values) => {
			values.iter().find_map(|value| find_key(value, key))
		}
		_ => None,
	}
}

fn source_refs_from_value(value: &Value) -> Vec<SourceRef> {
	match value {
		Value::Array(values) => {
			values.iter().flat_map(source_refs_from_value).collect()
		}
		Value::Object(map) => {
			if let (Some(name), Some(source_url)) = (
				map.get("sourceName")
					.or_else(|| map.get("name"))
					.and_then(Value::as_str),
				map.get("sourceUrl")
					.or_else(|| map.get("url"))
					.and_then(Value::as_str),
			) {
				return source_ref_from_fields(name, source_url)
					.into_iter()
					.collect();
			}

			map.values().flat_map(source_refs_from_value).collect()
		}
		Value::String(value) => serde_json::from_str::<Value>(value)
			.map(|value| source_refs_from_value(&value))
			.unwrap_or_else(|_| source_refs_from_text(value)),
		_ => Vec::new(),
	}
}

fn source_refs_from_text(value: &str) -> Vec<SourceRef> {
	let normalized = value
		.replace("\\u002F", "/")
		.replace("\\/", "/")
		.replace("\\\"", "\"");
	let forward_re =
		Regex::new(r#""sourceUrl":"([^"]+)".*?"sourceName":"([^"]+)""#)
			.expect("valid regex");
	let reverse_re =
		Regex::new(r#""sourceName":"([^"]+)".*?"sourceUrl":"([^"]+)""#)
			.expect("valid regex");
	let mut refs = Vec::new();

	for captures in forward_re.captures_iter(&normalized) {
		if let Some(source_ref) =
			source_ref_from_fields(&captures[2], &captures[1])
		{
			refs.push(source_ref);
		}
	}
	for captures in reverse_re.captures_iter(&normalized) {
		if let Some(source_ref) =
			source_ref_from_fields(&captures[1], &captures[2])
		{
			refs.push(source_ref);
		}
	}

	refs
}

fn source_ref_from_fields(name: &str, source_url: &str) -> Option<SourceRef> {
	let raw = source_url.trim_start_matches("--");
	if raw.is_empty() {
		return None;
	}
	let path = decode_provider_path(raw).unwrap_or_else(|_| raw.to_owned());
	Some(SourceRef {
		name: name.to_owned(),
		path,
	})
}

fn normalize_provider_name(name: &str) -> String {
	match name {
		"Default" => "wixmp",
		"Yt-mp4" => "youtube",
		"S-mp4" => "sharepoint",
		"Luf-Mp4" => "hianime",
		other => other,
	}
	.to_owned()
}

fn parse_direct_links(response: &str, source: &str) -> Vec<StreamLink> {
	let mut links = provider_link_metadata(response)
		.into_iter()
		.filter(|meta| !meta.url.contains(".m3u8"))
		.map(|meta| stream_from_provider_meta(meta, source))
		.collect::<Vec<_>>();
	if !links.is_empty() {
		return dedupe_links(links);
	}

	let link_re = Regex::new(r#""link":"([^"]+)".*?"resolutionStr":"([^"]+)""#)
		.expect("valid regex");
	for captures in link_re.captures_iter(response) {
		let url = captures
			.get(1)
			.map(|capture| capture.as_str())
			.unwrap_or("");
		let quality = captures
			.get(2)
			.map(|capture| capture.as_str())
			.unwrap_or("");
		links.push(StreamLink {
			quality: quality.to_owned(),
			url: url.to_owned(),
			source: source.to_owned(),
			referrer: None,
			subtitle: None,
			subtitles: Vec::new(),
			hardsub_language: None,
			audio_language: None,
			soft_subbed: false,
		});
	}

	let hls_re =
		Regex::new(r#""hls","url":"([^"]+)".*?"hardsub_lang":"([^"]+)""#)
			.expect("valid regex");
	for captures in hls_re.captures_iter(response) {
		let url = captures
			.get(1)
			.map(|capture| capture.as_str())
			.unwrap_or("");
		let hardsub_language =
			captures.get(2).map(|capture| capture.as_str().to_owned());
		links.push(StreamLink {
			quality: "hls".to_owned(),
			url: url.to_owned(),
			source: source.to_owned(),
			referrer: None,
			subtitle: None,
			subtitles: Vec::new(),
			hardsub_language,
			audio_language: None,
			soft_subbed: false,
		});
	}

	dedupe_links(links)
}

fn provider_link_metadata(response: &str) -> Vec<ProviderLinkMeta> {
	let normalized = response
		.replace("\\u002F", "/")
		.replace("\\/", "/")
		.replace("\\\"", "\"");
	serde_json::from_str::<Value>(&normalized)
		.map(|value| collect_provider_link_metadata(&value))
		.unwrap_or_default()
}

fn collect_provider_link_metadata(value: &Value) -> Vec<ProviderLinkMeta> {
	match value {
		Value::Array(values) => values
			.iter()
			.flat_map(collect_provider_link_metadata)
			.collect(),
		Value::Object(map) => {
			if let Some(url) = map
				.get("link")
				.or_else(|| map.get("url"))
				.or_else(|| map.get("file"))
				.and_then(Value::as_str)
			{
				return vec![ProviderLinkMeta {
					quality: string_field(
						value,
						&["resolutionStr", "resolution", "quality"],
					)
					.map(normalize_quality),
					url: url.to_owned(),
					referrer: value
						.pointer("/headers/Referer")
						.and_then(Value::as_str)
						.map(ToOwned::to_owned)
						.or_else(|| {
							string_field(value, &["Referer", "referrer"])
						}),
					subtitles: subtitle_tracks_from_value(
						map.get("subtitles")
							.or_else(|| map.get("tracks"))
							.unwrap_or(&Value::Null),
					),
					hardsub_language: string_field(
						value,
						&["hardsub_lang", "hardsubLang"],
					),
					audio_language: string_field(
						value,
						&[
							"audio_lang",
							"audioLang",
							"dub_lang",
							"dubLang",
							"audio_language",
							"audioLanguage",
						],
					),
				}];
			}

			map.values()
				.flat_map(collect_provider_link_metadata)
				.collect()
		}
		_ => Vec::new(),
	}
}

fn stream_from_provider_meta(
	meta: ProviderLinkMeta,
	source: &str,
) -> StreamLink {
	StreamLink {
		quality: meta.quality.unwrap_or_else(|| "hls".to_owned()),
		url: meta.url,
		source: source.to_owned(),
		referrer: meta.referrer,
		subtitle: meta.subtitles.first().map(|track| track.url.clone()),
		soft_subbed: !meta.subtitles.is_empty(),
		subtitles: meta.subtitles,
		hardsub_language: meta.hardsub_language,
		audio_language: meta.audio_language,
	}
}

fn subtitle_tracks_from_value(value: &Value) -> Vec<SubtitleTrack> {
	match value {
		Value::Array(values) => values
			.iter()
			.filter_map(|value| {
				let url = string_field(value, &["src", "url", "file"])?;
				let lang = string_field(
					value,
					&["lang", "shortcode", "srclang", "language"],
				)
				.unwrap_or_else(|| "unknown".to_owned());
				let label = string_field(value, &["label", "name"])
					.filter(|label| !label.is_empty())
					.unwrap_or_else(|| lang.clone());
				Some(SubtitleTrack { lang, label, url })
			})
			.collect(),
		_ => Vec::new(),
	}
}

fn subtitle_tracks_from_text(value: &str) -> Vec<SubtitleTrack> {
	match serde_json::from_str::<Value>(value) {
		Ok(value) => find_key(&value, "subtitles")
			.map(subtitle_tracks_from_value)
			.unwrap_or_default(),
		Err(_) => Vec::new(),
	}
}

fn referer_from_text(value: &str) -> Option<String> {
	Regex::new(r#""Referer":"([^"]+)""#)
		.expect("valid regex")
		.captures(value)
		.and_then(|captures| captures.get(1))
		.map(|capture| capture.as_str().to_owned())
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
	keys.iter().find_map(|key| {
		value.get(*key).and_then(|value| {
			value
				.as_str()
				.map(ToOwned::to_owned)
				.or_else(|| value.as_u64().map(|value| value.to_string()))
		})
	})
}

fn normalize_quality(value: String) -> String {
	if value.chars().all(|ch| ch.is_ascii_digit()) {
		format!("{value}p")
	} else {
		value
	}
}

fn expand_wixmp_links(response: &str, source: &str) -> Vec<StreamLink> {
	let mut links = Vec::new();
	let options_re = Regex::new(r#"/([^/]*),/mp4"#).expect("valid regex");
	let replace_re = Regex::new(r#",[^/]*"#).expect("valid regex");
	for link in parse_direct_links(response, source) {
		if !link.url.contains("repackager.wixmp.com") {
			links.push(link);
			continue;
		}
		let template = link
			.url
			.replace("repackager.wixmp.com/", "")
			.split(".urlset")
			.next()
			.unwrap_or(&link.url)
			.to_owned();

		let options = options_re
			.captures(&link.url)
			.and_then(|captures| captures.get(1))
			.map(|capture| capture.as_str().to_owned());
		if let Some(options) = options {
			for quality in options.split(',').filter(|part| !part.is_empty()) {
				let url = replace_re.replace(&template, quality).into_owned();
				links.push(StreamLink {
					quality: quality.to_owned(),
					url,
					source: source.to_owned(),
					referrer: link.referrer.clone(),
					subtitle: link.subtitle.clone(),
					subtitles: link.subtitles.clone(),
					hardsub_language: link.hardsub_language.clone(),
					audio_language: link.audio_language.clone(),
					soft_subbed: !link.subtitles.is_empty(),
				});
			}
		} else {
			links.push(link);
		}
	}
	dedupe_links(links)
}

fn hls_url(response: &str) -> Option<String> {
	Regex::new(r#""url":"([^"]*master\.m3u8[^"]*)""#)
		.expect("valid regex")
		.captures(response)
		.and_then(|captures| captures.get(1))
		.map(|capture| capture.as_str().to_owned())
		.or_else(|| {
			Regex::new(r#"(https?://[^"]*master\.m3u8[^"]*)"#)
				.expect("valid regex")
				.captures(response)
				.and_then(|captures| captures.get(1))
				.map(|capture| capture.as_str().to_owned())
		})
}

fn dedupe_links(links: Vec<StreamLink>) -> Vec<StreamLink> {
	let mut seen = HashMap::<
		(String, String, Option<String>, Option<String>),
		StreamLink,
	>::new();
	for link in links {
		seen.entry((
			link.quality.clone(),
			link.url.clone(),
			link.hardsub_language.clone(),
			link.audio_language.clone(),
		))
		.or_insert(link);
	}
	seen.into_values().collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn decodes_allanime_provider_path() {
		let encoded = encode_provider_path("/clock");
		assert_eq!(decode_provider_path(&encoded).unwrap(), "/clock.json");
	}

	#[test]
	fn extracts_source_refs_from_source_urls_array() {
		let encoded = encode_provider_path("/provider/clock");
		let value = serde_json::json!([
			{
				"sourceName": "Default",
				"sourceUrl": format!("--{encoded}")
			}
		]);

		let refs = source_refs_from_value(&value);

		assert_eq!(refs.len(), 1);
		assert_eq!(refs[0].name, "Default");
		assert_eq!(refs[0].path, "/provider/clock.json");
	}

	#[test]
	fn reads_graphql_error_messages() {
		let value = serde_json::json!({
			"errors": [{ "message": "NEED_CAPTCHA" }],
			"data": { "episode": null }
		});

		assert_eq!(graphql_errors(&value).as_deref(), Some("NEED_CAPTCHA"));
	}

	#[test]
	fn extracts_multiple_subtitle_tracks_from_provider_links() {
		let response = r#"{
            "links": [{
                "link": "https://example.test/master.m3u8",
                "resolutionStr": "1080p",
                "headers": { "Referer": "https://example.test/" },
                "subtitles": [
                    { "lang": "en", "label": "English", "src": "https://example.test/en.vtt" },
                    { "lang": "ru", "label": "Russian", "src": "https://example.test/ru.vtt" }
                ]
            }]
        }"#;

		let links = provider_link_metadata(response);

		assert_eq!(links.len(), 1);
		assert_eq!(links[0].subtitles.len(), 2);
		assert_eq!(links[0].subtitles[1].lang, "ru");
	}

	#[tokio::test]
	#[ignore = "hits the live AllAnime API"]
	async fn fetches_live_episode_source_refs() {
		let client = AllAnimeClient::new().unwrap();
		let refs = client
			.source_refs("B6AMhLy6EQHDgYgBF", TranslationMode::Sub, "1")
			.await
			.unwrap();

		assert!(!refs.is_empty());
	}

	#[tokio::test]
	#[ignore = "hits the live AllAnime API and stream providers"]
	async fn fetches_live_episode_streams() {
		let client = AllAnimeClient::new().unwrap();
		let sources = client
			.episode_sources(
				"B6AMhLy6EQHDgYgBF",
				TranslationMode::Sub,
				"1",
				&QualityPreference::Best,
				false,
			)
			.await
			.unwrap();

		assert!(!sources.links.is_empty());
		assert!(!sources.selected.url.is_empty());
	}

	#[test]
	fn selects_best_quality_by_sorted_order() {
		let links = vec![
			StreamLink {
				quality: "1080p".to_owned(),
				url: "a".to_owned(),
				source: "test".to_owned(),
				referrer: None,
				subtitle: None,
				subtitles: Vec::new(),
				hardsub_language: None,
				audio_language: None,
				soft_subbed: false,
			},
			StreamLink {
				quality: "720p".to_owned(),
				url: "b".to_owned(),
				source: "test".to_owned(),
				referrer: None,
				subtitle: None,
				subtitles: Vec::new(),
				hardsub_language: None,
				audio_language: None,
				soft_subbed: false,
			},
		];
		assert_eq!(
			select_quality(&links, &QualityPreference::Best)
				.unwrap()
				.quality,
			"1080p"
		);
	}

	fn encode_provider_path(value: &str) -> String {
		value
			.bytes()
			.map(|byte| format!("{:02x}", byte ^ 0x38))
			.collect::<String>()
	}
}
