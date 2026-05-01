use eyre::{Context, Result};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use urlencoding::encode;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextEpisodeStatus {
	pub english_title: Option<String>,
	pub japanese_title: Option<String>,
	pub next_raw_release: Option<String>,
	pub next_sub_release: Option<String>,
	pub status: String,
}

pub async fn fetch_next_episode_status(
	query: &str,
) -> Result<Vec<NextEpisodeStatus>> {
	let client = Client::new();
	let search_url =
		format!("https://animeschedule.net/api/v3/anime?q={}", encode(query));
	let search = client
		.get(&search_url)
		.send()
		.await
		.wrap_err("failed to search AnimeSchedule")?
		.error_for_status()
		.wrap_err("AnimeSchedule search failed")?
		.text()
		.await
		.wrap_err("failed to read AnimeSchedule search response")?;

	let routes = Regex::new(r#""route":"([^"]+)""#)
		.expect("valid regex")
		.captures_iter(&search)
		.filter_map(|captures| {
			captures.get(1).map(|capture| capture.as_str().to_owned())
		})
		.take(10)
		.collect::<Vec<_>>();

	let mut statuses = Vec::new();
	for route in routes {
		let page = client
			.get(format!("https://animeschedule.net/anime/{route}"))
			.send()
			.await
			.wrap_err("failed to fetch AnimeSchedule page")?
			.error_for_status()
			.wrap_err("AnimeSchedule page failed")?
			.text()
			.await
			.wrap_err("failed to read AnimeSchedule page")?;
		statuses.push(parse_status(&page));
	}

	Ok(statuses)
}

fn parse_status(page: &str) -> NextEpisodeStatus {
	let capture = |pattern: &str| {
		Regex::new(pattern)
			.expect("valid regex")
			.captures(page)
			.and_then(|captures| captures.get(1))
			.map(|capture| html_unescape(capture.as_str()))
	};
	let next_raw_release = capture(r#"countdown-time-raw" datetime="([^"]+)""#);
	let next_sub_release = capture(r#"countdown-time" datetime="([^"]+)""#);
	let english_title = capture(r#"english-title">([^<]+)<"#);
	let japanese_title = capture(r#"main-title".*>([^<]+)<"#);
	let status = if next_raw_release.is_some() || next_sub_release.is_some() {
		"Ongoing"
	} else {
		"Finished"
	}
	.to_owned();

	NextEpisodeStatus {
		english_title,
		japanese_title,
		next_raw_release,
		next_sub_release,
		status,
	}
}

fn html_unescape(value: &str) -> String {
	value
		.replace("&amp;", "&")
		.replace("&quot;", "\"")
		.replace("&#39;", "'")
		.replace("&lt;", "<")
		.replace("&gt;", ">")
}
