use eyre::{Result, eyre};

pub fn episode_key(value: &str) -> f64 {
	value.trim().parse::<f64>().unwrap_or(f64::MAX)
}

pub fn next_episode<'a>(
	episodes: &'a [String],
	current: &str,
) -> Option<&'a str> {
	episodes
		.iter()
		.position(|episode| episode == current)
		.and_then(|index| episodes.get(index + 1))
		.map(String::as_str)
}

pub fn previous_episode<'a>(
	episodes: &'a [String],
	current: &str,
) -> Option<&'a str> {
	episodes
		.iter()
		.position(|episode| episode == current)
		.and_then(|index| index.checked_sub(1))
		.and_then(|index| episodes.get(index))
		.map(String::as_str)
}

pub fn parse_episode_range(
	selection: &str,
	episodes: &[String],
) -> Result<Vec<String>> {
	let selection = selection.trim();
	if selection.is_empty() {
		return Err(eyre!("episode selection is empty"));
	}

	if selection == "-1" {
		return episodes
			.last()
			.cloned()
			.map(|episode| vec![episode])
			.ok_or_else(|| eyre!("episode list is empty"));
	}

	let requested = selection
		.split_whitespace()
		.flat_map(|part| part.split(','))
		.filter(|part| !part.trim().is_empty())
		.collect::<Vec<_>>();

	if requested.len() > 1
		&& requested
			.iter()
			.all(|part| episodes.iter().any(|e| e == part))
	{
		return Ok(requested.into_iter().map(ToOwned::to_owned).collect());
	}

	let numbers = numeric_parts(selection);
	let start = numbers.first().map(String::as_str).unwrap_or(selection);
	let end = if selection.ends_with("-1") && selection != "-1" {
		"-1"
	} else {
		numbers.last().map(String::as_str).unwrap_or(start)
	};

	let start = if start == "-1" {
		episodes
			.last()
			.map(String::as_str)
			.ok_or_else(|| eyre!("episode list is empty"))?
	} else {
		start
	};
	let end = if end == "-1" {
		episodes
			.last()
			.map(String::as_str)
			.ok_or_else(|| eyre!("episode list is empty"))?
	} else {
		end
	};

	if start == end {
		return Ok(vec![start.to_owned()]);
	}

	let start_index = episodes
		.iter()
		.position(|episode| episode == start)
		.ok_or_else(|| eyre!("episode {start} is not available"))?;
	let end_index = episodes
		.iter()
		.position(|episode| episode == end)
		.ok_or_else(|| eyre!("episode {end} is not available"))?;

	if start_index > end_index {
		return Err(eyre!("episode range start must be before the end"));
	}

	Ok(episodes[start_index..=end_index].to_vec())
}

fn numeric_parts(selection: &str) -> Vec<String> {
	let mut parts = Vec::new();
	let mut current = String::new();
	for ch in selection.chars() {
		if ch.is_ascii_digit() || ch == '.' {
			current.push(ch);
		} else if !current.is_empty() {
			parts.push(std::mem::take(&mut current));
		}
	}
	if !current.is_empty() {
		parts.push(current);
	}
	parts
}

#[cfg(test)]
mod tests {
	use super::*;

	fn episodes() -> Vec<String> {
		["1", "2", "3", "4", "5", "6"]
			.into_iter()
			.map(ToOwned::to_owned)
			.collect()
	}

	#[test]
	fn parses_last_episode() {
		assert_eq!(parse_episode_range("-1", &episodes()).unwrap(), vec!["6"]);
	}

	#[test]
	fn parses_dash_range() {
		assert_eq!(
			parse_episode_range("2-4", &episodes()).unwrap(),
			vec!["2", "3", "4"]
		);
	}
}
