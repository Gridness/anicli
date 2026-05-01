mod schedule;
mod source;

pub use schedule::{NextEpisodeStatus, fetch_next_episode_status};
pub use source::{AllAnimeClient, AllAnimeEndpoints, EpisodeSources};
