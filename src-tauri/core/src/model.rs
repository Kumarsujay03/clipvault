use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    Text,
    Code,
    Image,
    Files,
}

impl ItemType {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemType::Text => "text",
            ItemType::Code => "code",
            ItemType::Image => "image",
            ItemType::Files => "files",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "text" => ItemType::Text,
            "code" => ItemType::Code,
            "image" => ItemType::Image,
            "files" => ItemType::Files,
            _ => return None,
        })
    }
}

/// A row ready to be inserted, produced by the ingest pipeline.
#[derive(Debug, Clone)]
pub struct NewItem {
    pub item_type: ItemType,
    pub content_text: Option<String>,
    /// Raw text/html as offered by the source; never rendered unsanitized.
    pub rich_html: Option<String>,
    pub preview_text: String,
    pub content_hash: String,
    pub mime_type: String,
    pub source_application: Option<String>,
    pub media_path: Option<String>,
    pub thumbnail_path: Option<String>,
    pub byte_size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub metadata: serde_json::Value,
    pub search_extra: String,
}

/// Compact representation used by the history list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemSummary {
    pub id: i64,
    pub item_type: ItemType,
    pub preview: String,
    pub mime_type: String,
    pub created_at: i64,
    pub copied_at: i64,
    pub last_used_at: Option<i64>,
    pub copy_count: i64,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub byte_size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub has_thumbnail: bool,
    pub file_count: i64,
    pub has_html: bool,
    /// Detected programming language for code items.
    pub language: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    #[serde(flatten)]
    pub summary: ItemSummary,
    /// Full text (truncated for display if extremely large; see `text_truncated`).
    pub content_text: Option<String>,
    pub text_truncated: bool,
    pub file_paths: Vec<String>,
    pub source_application: Option<String>,
    pub metadata: serde_json::Value,
    /// Source HTML (rich text). The UI sanitizes it before rendering.
    pub rich_html: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    #[default]
    All,
    Pinned,
    Favorites,
    Text,
    Code,
    Image,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    RecentlyUsed,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ListQuery {
    pub search: Option<String>,
    pub filter: Filter,
    pub sort: Sort,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub items: Vec<ItemSummary>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub all: i64,
    pub pinned: i64,
    pub favorites: i64,
    pub text: i64,
    pub code: i64,
    pub image: i64,
    pub files: i64,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
