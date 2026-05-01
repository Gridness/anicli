use std::{
	fs,
	io::{Read, Write},
	path::PathBuf,
};

use eyre::{Context, Result};

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

	pub fn upsert(&self, entry: HistoryEntry) -> Result<()> {
		let mut entries = self.load()?;
		if let Some(existing) = entries
			.iter_mut()
			.find(|existing| existing.anime_id == entry.anime_id)
		{
			*existing = entry;
		} else {
			entries.push(entry);
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
