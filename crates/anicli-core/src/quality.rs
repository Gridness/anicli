use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityPreference {
    #[default]
    Best,
    Worst,
    Exact(String),
}

impl QualityPreference {
    pub fn parse(value: impl AsRef<str>) -> Self {
        match value.as_ref().trim().to_ascii_lowercase().as_str() {
            "" | "best" => Self::Best,
            "worst" => Self::Worst,
            other => Self::Exact(other.to_owned()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Best => "best",
            Self::Worst => "worst",
            Self::Exact(value) => value,
        }
    }
}

impl std::fmt::Display for QualityPreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}
