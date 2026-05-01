use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TranslationMode {
    #[default]
    Sub,
    Dub,
}

impl TranslationMode {
    pub fn as_allanime(self) -> &'static str {
        match self {
            Self::Sub => "sub",
            Self::Dub => "dub",
        }
    }

    pub fn toggle(&mut self) {
        *self = match self {
            Self::Sub => Self::Dub,
            Self::Dub => Self::Sub,
        };
    }
}

impl std::fmt::Display for TranslationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_allanime())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimeSearchResult {
    pub id: String,
    pub title: String,
    pub episode_count: Option<u32>,
}

impl AnimeSearchResult {
    pub fn display_title(&self) -> String {
        match self.episode_count {
            Some(count) => format!("{} ({} episodes)", self.title, count),
            None => self.title.clone(),
        }
    }

    pub fn media_title_prefix(&self) -> String {
        self.title
            .split('(')
            .next()
            .unwrap_or(&self.title)
            .chars()
            .filter(|c| !c.is_ascii_punctuation())
            .collect::<String>()
            .trim()
            .to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamLink {
    pub quality: String,
    pub url: String,
    pub source: String,
    pub referrer: Option<String>,
    pub subtitle: Option<String>,
    pub soft_subbed: bool,
}

impl StreamLink {
    pub fn score(&self) -> i32 {
        self.quality
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedStream {
    pub quality: String,
    pub url: String,
    pub source: String,
    pub referrer: Option<String>,
    pub subtitle: Option<String>,
}

impl From<StreamLink> for SelectedStream {
    fn from(link: StreamLink) -> Self {
        Self {
            quality: link.quality,
            url: link.url,
            source: link.source,
            referrer: link.referrer,
            subtitle: link.subtitle,
        }
    }
}
