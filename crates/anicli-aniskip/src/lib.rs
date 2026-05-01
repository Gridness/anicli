mod client;
mod iina;
mod mpv;

pub use client::{AniSkipClient, SkipSegment, SkipSource, SkipTimes};
pub use iina::{IinaPluginInstall, install_iina_plugin};
pub use mpv::{MpvSkipOptions, SkipLaunch, build_mpv_skip_launch};
