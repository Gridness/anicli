use eyre::{Context, Result, eyre};
use regex::Regex;
use reqwest::{Client, header};
use serde::Deserialize;
use serde_json::{Value, json};
use urlencoding::encode;

const AGENT: &str = "Mozilla/5.0 (Windows NT 6.1; Win64; rv:109.0) Gecko/20100101 Firefox/109.0";
const ALLANIME_API: &str = "https://api.allanime.day/api";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipSource {
    MyAnimeList,
    AllAnime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkipSegment {
    pub skip_type: String,
    pub start_time: f64,
    pub end_time: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkipTimes {
    pub segments: Vec<SkipSegment>,
}

impl SkipTimes {
    pub fn opening(&self) -> Option<&SkipSegment> {
        self.segments
            .iter()
            .find(|segment| segment.skip_type == "op")
    }

    pub fn ending(&self) -> Option<&SkipSegment> {
        self.segments
            .iter()
            .find(|segment| segment.skip_type == "ed")
    }
}

#[derive(Debug, Clone)]
pub struct AniSkipClient {
    http: Client,
}

impl AniSkipClient {
    pub fn new() -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::USER_AGENT, header::HeaderValue::from_static(AGENT));
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        let http = Client::builder()
            .default_headers(headers)
            .build()
            .wrap_err("failed to build AniSkip HTTP client")?;
        Ok(Self { http })
    }

    pub async fn resolve_mal_id(
        &self,
        query_or_id: &str,
        source: SkipSource,
        filter: Option<&str>,
    ) -> Result<u64> {
        if query_or_id.chars().all(|c| c.is_ascii_digit()) {
            return query_or_id
                .parse()
                .wrap_err("failed to parse MyAnimeList id");
        }

        match source {
            SkipSource::MyAnimeList => self.fetch_mal_id_myanimelist(query_or_id, filter).await,
            SkipSource::AllAnime => self.fetch_mal_id_allanime(query_or_id, filter).await,
        }
    }

    pub async fn resolve_mal_id_from_allanime_id(&self, allanime_id: &str) -> Result<u64> {
        let value = self
            .http
            .post(ALLANIME_API)
            .json(&json!({
                "query": format!("{{ show(_id: \"{}\") {{ malId }} }}", allanime_id.replace('"', "\\\"")),
            }))
            .send()
            .await
            .wrap_err("failed to resolve AllAnime id")?
            .error_for_status()
            .wrap_err("AllAnime MAL id lookup failed")?
            .json::<Value>()
            .await
            .wrap_err("failed to decode AllAnime MAL id response")?;

        value
            .pointer("/data/show/malId")
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            })
            .ok_or_else(|| eyre!("AllAnime show did not include a MAL id"))
    }

    pub async fn skip_times(&self, mal_id: u64, episode: &str) -> Result<SkipTimes> {
        let episode = episode
            .split('.')
            .next()
            .unwrap_or(episode)
            .parse::<u64>()
            .wrap_err("AniSkip only supports integer episode numbers")?;
        let url =
            format!("https://api.aniskip.com/v1/skip-times/{mal_id}/{episode}?types=op&types=ed");
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .wrap_err("failed to fetch AniSkip timestamps")?
            .error_for_status()
            .wrap_err("AniSkip timestamp lookup failed")?
            .json::<SkipTimesResponse>()
            .await
            .wrap_err("failed to decode AniSkip response")?;

        if !response.found {
            return Err(eyre!("skip times not found"));
        }

        Ok(SkipTimes {
            segments: response
                .results
                .into_iter()
                .map(|result| SkipSegment {
                    skip_type: result.skip_type,
                    start_time: result.interval.start_time,
                    end_time: result.interval.end_time,
                })
                .collect(),
        })
    }

    async fn fetch_mal_id_myanimelist(&self, query: &str, filter: Option<&str>) -> Result<u64> {
        let name = strip_episode_count(query);
        let keyword = encode(&name);
        let value = self
            .http
            .get(format!(
                "https://myanimelist.net/search/prefix.json?type=anime&keyword={keyword}"
            ))
            .send()
            .await
            .wrap_err("failed to search MyAnimeList")?
            .error_for_status()
            .wrap_err("MyAnimeList search failed")?
            .json::<Value>()
            .await
            .wrap_err("failed to decode MyAnimeList response")?;

        let items = value
            .get("categories")
            .and_then(Value::as_array)
            .and_then(|categories| {
                categories
                    .iter()
                    .find(|category| category.get("type").and_then(Value::as_str) == Some("anime"))
            })
            .and_then(|category| category.get("items"))
            .and_then(Value::as_array)
            .ok_or_else(|| eyre!("MyAnimeList returned no anime results"))?;

        select_mal_item(items, &name, filter)
    }

    async fn fetch_mal_id_allanime(&self, query: &str, filter: Option<&str>) -> Result<u64> {
        let keyword = strip_episode_count(query);
        let search_query = "query($search:SearchInput $limit:Int $page:Int $translationType:VaildTranslationTypeEnumType $countryOrigin:VaildCountryOriginEnumType){shows(search:$search limit:$limit page:$page translationType:$translationType countryOrigin:$countryOrigin){edges{_id name}}}";
        let value = self
            .http
            .post(ALLANIME_API)
            .json(&json!({
                "query": search_query,
                "variables": {
                    "search": { "query": keyword },
                    "limit": 10,
                    "page": 1,
                    "translationType": "sub",
                    "countryOrigin": "ALL",
                },
            }))
            .send()
            .await
            .wrap_err("failed to search AllAnime for MAL id")?
            .error_for_status()
            .wrap_err("AllAnime MAL search failed")?
            .json::<Value>()
            .await
            .wrap_err("failed to decode AllAnime MAL search response")?;

        let edges = value
            .pointer("/data/shows/edges")
            .and_then(Value::as_array)
            .ok_or_else(|| eyre!("AllAnime returned no search results"))?;
        let filter_re = filter
            .map(Regex::new)
            .transpose()
            .wrap_err("invalid filter regex")?;
        let selected = edges
            .iter()
            .find(|edge| {
                let title = edge.get("name").and_then(Value::as_str).unwrap_or("");
                filter_re
                    .as_ref()
                    .map(|regex| regex.is_match(title))
                    .unwrap_or(false)
            })
            .or_else(|| {
                edges.iter().find(|edge| {
                    let title = edge
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    title.contains(&keyword.to_ascii_lowercase())
                })
            })
            .or_else(|| edges.first())
            .ok_or_else(|| eyre!("AllAnime returned no search results"))?;

        let id = selected
            .get("_id")
            .and_then(Value::as_str)
            .ok_or_else(|| eyre!("selected AllAnime result did not include an id"))?;
        self.resolve_mal_id_from_allanime_id(id).await
    }
}

fn select_mal_item(items: &[Value], query: &str, filter: Option<&str>) -> Result<u64> {
    let filter_re = filter
        .map(Regex::new)
        .transpose()
        .wrap_err("invalid filter regex")?;
    let query = normalize(query);
    let selected = items
        .iter()
        .find(|item| {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            filter_re
                .as_ref()
                .map(|regex| regex.is_match(name))
                .unwrap_or(false)
        })
        .or_else(|| {
            items.iter().find(|item| {
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                normalize(name).contains(&query)
            })
        })
        .or_else(|| items.first())
        .ok_or_else(|| eyre!("MyAnimeList returned no anime results"))?;

    selected
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| eyre!("selected MyAnimeList result did not include an id"))
}

fn strip_episode_count(query: &str) -> String {
    Regex::new(r#" \([0-9]+ episodes\)"#)
        .expect("valid regex")
        .replace(query, "")
        .trim()
        .to_owned()
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || c.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

#[derive(Debug, Deserialize)]
struct SkipTimesResponse {
    found: bool,
    results: Vec<SkipTimesResult>,
}

#[derive(Debug, Deserialize)]
struct SkipTimesResult {
    skip_type: String,
    interval: Timestamp,
}

#[derive(Debug, Deserialize)]
struct Timestamp {
    start_time: f64,
    end_time: f64,
}
