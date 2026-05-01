use std::{
	fs,
	path::PathBuf,
	time::{SystemTime, UNIX_EPOCH},
};

use eyre::{Context, Result, eyre};

use crate::{AniSkipClient, SkipTimes};

const MPV_SKIP_LUA: &str = r#"local mp = require('mp')
local mp_options = require('mp.options')

local options = {
  op_start = 0, op_end = 0, ed_start = 0, ed_end = 0,
  toggle = false, toggle_key = "T", offset = 0,
}
mp_options.read_options(options, "skip")

local skipped_op = false
local skipped_ed = false
local skip_enabled = true

local function skip()
  if not skip_enabled then return end
  local current_time = mp.get_property_number("time-pos")
  if not current_time then return end

  local op_target = options.op_end - options.offset
  local ed_target = options.ed_end - options.offset

  if current_time >= options.op_start and current_time < op_target then
    if options.toggle or not skipped_op then
      mp.set_property_number("time-pos", op_target)
      skipped_op = true
    end
  end

  if current_time >= options.ed_start and current_time < ed_target then
    if options.toggle or not skipped_ed then
      mp.set_property_number("time-pos", ed_target)
      skipped_ed = true
    end
  end
end

local function toggle_skip()
  skip_enabled = not skip_enabled
  if skip_enabled then
    skipped_op = false
    skipped_ed = false
  end
  mp.osd_message("Skip: " .. (skip_enabled and "ON" or "OFF"), 2)
end

if options.toggle then
  mp.add_key_binding(options.toggle_key, "toggle-skip", toggle_skip)
end

mp.observe_property("time-pos", "number", skip)
"#;

#[derive(Debug, Clone)]
pub struct MpvSkipOptions {
	pub toggle: bool,
	pub toggle_key: String,
	pub offset: u8,
}

impl Default for MpvSkipOptions {
	fn default() -> Self {
		Self {
			toggle: false,
			toggle_key: std::env::var("ANI_SKIP_TOGGLE_KEY")
				.unwrap_or_else(|_| "T".to_owned()),
			offset: 0,
		}
	}
}

#[derive(Debug, Clone)]
pub struct SkipLaunch {
	pub script_path: PathBuf,
	pub chapters_path: PathBuf,
	pub script_opts: String,
}

impl SkipLaunch {
	pub fn mpv_args(&self) -> Vec<String> {
		vec![
			format!("--script={}", self.script_path.display()),
			format!("--chapters-file={}", self.chapters_path.display()),
			format!("--script-opts={}", self.script_opts),
		]
	}

	pub fn iina_args(&self) -> Vec<String> {
		vec![
			format!("--mpv-script={}", self.script_path.display()),
			format!("--mpv-chapters-file={}", self.chapters_path.display()),
			format!("--mpv-script-opts={}", self.script_opts),
		]
	}
}

pub async fn build_mpv_skip_launch(
	client: &AniSkipClient,
	mal_id: u64,
	episode: &str,
	options: &MpvSkipOptions,
) -> Result<SkipLaunch> {
	if options.offset > 5 {
		return Err(eyre!("AniSkip offset cannot be more than 5 seconds"));
	}

	let skip_times = client.skip_times(mal_id, episode).await?;
	build_launch_files(&skip_times, options)
}

pub fn build_launch_files(
	skip_times: &SkipTimes,
	options: &MpvSkipOptions,
) -> Result<SkipLaunch> {
	let dir = std::env::temp_dir().join("anicli-rs");
	fs::create_dir_all(&dir)
		.wrap_err("failed to create temporary AniSkip directory")?;
	let stamp = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.wrap_err("system clock is before UNIX_EPOCH")?
		.as_millis();
	let script_path = dir.join(format!("skip-{stamp}.lua"));
	let chapters_path = dir.join(format!("chapters-{stamp}.ffmetadata"));
	fs::write(&script_path, MPV_SKIP_LUA)
		.wrap_err("failed to write mpv skip script")?;
	fs::write(&chapters_path, chapters_metadata(skip_times))
		.wrap_err("failed to write mpv chapters file")?;

	let mut opts = Vec::new();
	if let Some(op) = skip_times.opening() {
		opts.push(format!("skip-op_start={}", op.start_time));
		opts.push(format!("skip-op_end={}", op.end_time));
	}
	if let Some(ed) = skip_times.ending() {
		opts.push(format!("skip-ed_start={}", ed.start_time));
		opts.push(format!("skip-ed_end={}", ed.end_time));
	}
	if options.toggle {
		opts.push("skip-toggle=yes".to_owned());
		opts.push(format!("skip-toggle_key={}", options.toggle_key));
	}
	if options.offset > 0 {
		opts.push(format!("skip-offset={}", options.offset));
	}

	Ok(SkipLaunch {
		script_path,
		chapters_path,
		script_opts: opts.join(","),
	})
}

fn chapters_metadata(skip_times: &SkipTimes) -> String {
	let mut out = String::from(";FFMETADATA1");
	let mut op_end = None;
	let mut ed_start = None;
	for segment in &skip_times.segments {
		let title = match segment.skip_type.as_str() {
			"op" => {
				op_end = Some(segment.end_time);
				"Opening"
			}
			"ed" => {
				ed_start = Some(segment.start_time);
				"Ending"
			}
			other => other,
		};
		push_chapter(&mut out, segment.start_time, segment.end_time, title);
	}
	if let Some(op_end) = op_end {
		push_chapter(&mut out, op_end, ed_start.unwrap_or(op_end), "Episode");
	}
	out
}

fn push_chapter(out: &mut String, start: f64, end: f64, title: &str) {
	out.push_str("\n[CHAPTER]\nTIMEBASE=1/1000\n");
	out.push_str(&format!(
		"START={}\nEND={}\nTITLE={title}\n",
		ftoi(start),
		ftoi(end)
	));
}

fn ftoi(value: f64) -> u64 {
	(value * 1000.0).round() as u64
}
