use std::{fs, path::PathBuf, process::Command};

use eyre::{Context, Result};

#[derive(Debug, Clone)]
pub struct IinaPluginInstall {
	pub plugin_dir: PathBuf,
	pub enabled_plugin_system: bool,
}

pub fn install_iina_plugin() -> Result<IinaPluginInstall> {
	let plugin_dir = iina_plugin_dir()?.join("anicli-rs-aniskip.iinaplugin");
	fs::create_dir_all(&plugin_dir).wrap_err_with(|| {
		format!("failed to create {}", plugin_dir.display())
	})?;
	fs::write(plugin_dir.join("Info.json"), IINA_INFO_JSON)
		.wrap_err("failed to write IINA plugin Info.json")?;
	fs::write(plugin_dir.join("main.js"), IINA_MAIN_JS)
		.wrap_err("failed to write IINA plugin main.js")?;

	let enabled_plugin_system = Command::new("defaults")
		.args([
			"write",
			"com.colliderli.iina",
			"iinaEnablePluginSystem",
			"true",
		])
		.status()
		.map(|status| status.success())
		.unwrap_or(false);

	Ok(IinaPluginInstall {
		plugin_dir,
		enabled_plugin_system,
	})
}

fn iina_plugin_dir() -> Result<PathBuf> {
	let home = std::env::var_os("HOME")
		.map(PathBuf::from)
		.ok_or_else(|| eyre::eyre!("HOME is not set"))?;
	Ok(home.join("Library/Application Support/com.colliderli.iina/plugins"))
}

const IINA_INFO_JSON: &str = r#"{
  "name": "anicli-rs AniSkip",
  "identifier": "rs.anicli.aniskip",
  "version": "1.0.0",
  "description": "Automatically skips anime OP and ED ranges for anicli-rs playback in IINA.",
  "author": {
    "name": "anicli-rs",
    "url": "https://github.com/heeka/anicli-rs"
  },
  "entry": "main.js",
  "permissions": ["show-osd", "network-request"],
  "allowedDomains": ["*"],
  "preferenceDefaults": {}
}
"#;

const IINA_MAIN_JS: &str = r#"const { console, mpv, event, http, core } = iina;

let timestamps = null;
let loading = false;
let retries = 5;
let skipped = { op: false, ed: false };

const eventId = event.on("mpv.time-pos.changed", () => {
  if (loading) return;
  if (!timestamps) {
    if (retries-- <= 0) {
      event.off("mpv.time-pos.changed", eventId);
      return;
    }
    const info = animeInfo();
    if (!info) return;
    loading = true;
    loadTimestamps(info).then((loaded) => {
      timestamps = loaded;
      loading = false;
      if (!timestamps) event.off("mpv.time-pos.changed", eventId);
    });
    return;
  }
  const pos = mpv.getNumber("time-pos");
  trySkip("op", pos);
  trySkip("ed", pos);
});

function trySkip(type, pos) {
  if (!timestamps || !timestamps[type] || skipped[type]) return;
  const range = timestamps[type];
  if (pos >= range.start_time && pos < range.end_time) {
    core.seekTo(range.end_time);
    skipped[type] = true;
    core.osd("Skipped " + type.toUpperCase());
  }
}

async function loadTimestamps(info) {
  const malId = await malIdFor(info.name);
  if (!malId) return null;
  const res = await http.get("https://api.aniskip.com/v1/skip-times/" + malId + "/" + info.episode + "?types=op&types=ed", {
    headers: { "User-Agent": "Mozilla/5.0", "Content-Type": "application/json" },
    params: {},
    data: {}
  });
  const data = res.data;
  if (!data || !data.found) return null;
  const op = data.results.find((item) => item.skip_type === "op");
  const ed = data.results.find((item) => item.skip_type === "ed");
  return { op: op && op.interval, ed: ed && ed.interval };
}

async function malIdFor(name) {
  const res = await http.get("https://myanimelist.net/search/prefix.json?type=anime&keyword=" + encodeURIComponent(name), {
    headers: { "User-Agent": "Mozilla/5.0", "Content-Type": "application/json" },
    params: {},
    data: {}
  });
  const categories = (res.data && res.data.categories) || [];
  const anime = categories.find((category) => category.type === "anime");
  return anime && anime.items && anime.items[0] && anime.items[0].id;
}

function animeInfo() {
  const title = mpv.getString("media-title");
  if (!title) return null;
  const parts = title.split(" Episode ");
  if (parts.length < 2) return null;
  return { name: parts[0], episode: parseInt(parts[1], 10) };
}
"#;
