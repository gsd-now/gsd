//! Generic type for content that can be inline or linked to a file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Content that can be inline or linked to a file.
///
/// In config files:
/// - `{"inline": <value>}` → inline content
/// - `{"link": "path"}` → link to file
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MaybeLinked<T> {
    /// Inline content.
    Inline {
        /// The inline content.
        inline: T,
    },
    /// Link to a file.
    Link {
        /// Path to the file.
        link: String,
    },
}

impl<T: Default> Default for MaybeLinked<T> {
    fn default() -> Self {
        Self::Inline {
            inline: T::default(),
        }
    }
}

impl<T> MaybeLinked<T> {
    /// Get the inline value if this is inline content.
    #[must_use]
    pub const fn as_inline(&self) -> Option<&T> {
        match self {
            Self::Inline { inline } => Some(inline),
            Self::Link { .. } => None,
        }
    }

    /// Get the link path if this is a link.
    #[must_use]
    pub fn as_link(&self) -> Option<&str> {
        match self {
            Self::Inline { .. } => None,
            Self::Link { link } => Some(link),
        }
    }
}
