use std::{
	collections::HashSet,
	fs,
	io::{Read, Write},
	path::PathBuf,
};

use eyre::{Context, Result};

const MAX_HISTORY_EPISODES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
	pub episode: String,
	pub anime_id: String,
	pub title: String,
}

#[derive(Debug, Clone)]
pub struct HistoryStore {
	path: PathBuf,
}

impl HistoryStore {
	pub fn new(history_dir: PathBuf) -> Self {
		Self {
			path: history_dir.join("ani-hsts"),
		}
	}

	pub fn path(&self) -> &PathBuf {
		&self.path
	}

	pub fn load(&self) -> Result<Vec<HistoryEntry>> {
		self.ensure_file()?;
		let mut file = fs::File::open(&self.path).wrap_err_with(|| {
			format!("failed to open {}", self.path.display())
		})?;
		let mut contents = String::new();
		file.read_to_string(&mut contents).wrap_err_with(|| {
			format!("failed to read {}", self.path.display())
		})?;

		Ok(contents
			.lines()
			.filter_map(|line| {
				let mut fields = line.splitn(3, '\t');
				Some(HistoryEntry {
					episode: fields.next()?.to_owned(),
					anime_id: fields.next()?.to_owned(),
					title: fields.next()?.to_owned(),
				})
			})
			.collect())
	}

	pub fn load_latest(&self) -> Result<Vec<HistoryEntry>> {
		let mut seen = HashSet::new();
		let mut latest = self
			.load()?
			.into_iter()
			.rev()
			.filter(|entry| seen.insert(entry.anime_id.clone()))
			.collect::<Vec<_>>();
		latest.reverse();
		Ok(latest)
	}

	pub fn upsert(&self, entry: HistoryEntry) -> Result<()> {
		let mut entries = self.load()?;
		entries.push(entry);
		if entries.len() > MAX_HISTORY_EPISODES {
			entries.drain(0..entries.len() - MAX_HISTORY_EPISODES);
		}
		self.write_entries(&entries)
	}

	pub fn clear(&self) -> Result<()> {
		self.ensure_file()?;
		fs::write(&self.path, "").wrap_err_with(|| {
			format!("failed to clear {}", self.path.display())
		})
	}

	fn write_entries(&self, entries: &[HistoryEntry]) -> Result<()> {
		self.ensure_file()?;
		let mut file = fs::File::create(&self.path).wrap_err_with(|| {
			format!("failed to write {}", self.path.display())
		})?;
		for entry in entries {
			writeln!(
				file,
				"{}\t{}\t{}",
				entry.episode, entry.anime_id, entry.title
			)
			.wrap_err("failed to write history entry")?;
		}
		Ok(())
	}

	fn ensure_file(&self) -> Result<()> {
		if let Some(parent) = self.path.parent() {
			fs::create_dir_all(parent).wrap_err_with(|| {
				format!("failed to create {}", parent.display())
			})?;
		}
		if !self.path.exists() {
			fs::File::create(&self.path).wrap_err_with(|| {
				format!("failed to create {}", self.path.display())
			})?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicU64, Ordering};

	use super::*;

	static TEST_ID: AtomicU64 = AtomicU64::new(0);

	fn test_store(name: &str) -> HistoryStore {
		let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
		HistoryStore::new(std::env::temp_dir().join(format!(
			"anicli-history-test-{}-{name}-{id}",
			std::process::id()
		)))
	}

	#[test]
	fn caps_history_to_recent_entries() {
		let store = test_store("caps");

		for index in 0..1005 {
			store
				.upsert(HistoryEntry {
					episode: index.to_string(),
					anime_id: format!("anime-{index}"),
					title: format!("Anime {index}"),
				})
				.unwrap();
		}

		let entries = store.load().unwrap();
		assert_eq!(entries.len(), MAX_HISTORY_EPISODES);
		assert_eq!(entries.first().unwrap().anime_id, "anime-5");
		assert_eq!(entries.last().unwrap().anime_id, "anime-1004");
	}

	#[test]
	fn latest_history_keeps_most_recent_entry_per_title() {
		let store = test_store("latest");

		store
			.upsert(HistoryEntry {
				episode: "1".to_owned(),
				anime_id: "one".to_owned(),
				title: "One".to_owned(),
			})
			.unwrap();
		store
			.upsert(HistoryEntry {
				episode: "1".to_owned(),
				anime_id: "two".to_owned(),
				title: "Two".to_owned(),
			})
			.unwrap();
		store
			.upsert(HistoryEntry {
				episode: "2".to_owned(),
				anime_id: "one".to_owned(),
				title: "One".to_owned(),
			})
			.unwrap();

		let raw_entries = store.load().unwrap();
		assert_eq!(raw_entries.len(), 3);

		let latest_entries = store.load_latest().unwrap();
		assert_eq!(
			latest_entries
				.iter()
				.map(|entry| entry.anime_id.as_str())
				.collect::<Vec<_>>(),
			vec!["two", "one"]
		);
		assert_eq!(latest_entries.last().unwrap().episode, "2");
	}
}
