mod delete;
mod download;
mod folders;
mod upload;

pub use delete::{delete_forever, describe_contents, trash};
pub use download::{download_file, resolve, resolve_all};
pub use folders::{ensure_folder_path, list_folder, resolve_folder};
pub use upload::{forget_sessions_for, upload_file};

use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;

use crate::auth::get_google_access_token;

pub(super) const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
/// Drive's alias for "my top level" - no need to look the real id up.
pub const ROOT: &str = "root";
const FILE_FIELDS: &str = "id,name,size,mimeType,modifiedTime,webViewLink";

#[derive(Debug, Deserialize)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    #[serde(rename = "webViewLink")]
    pub web_view_link: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DriveEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "modifiedTime")]
    pub modified_time: String,
    #[serde(rename = "webViewLink")]
    pub web_view_link: Option<String>,
}

impl DriveEntry {
    pub fn size_bytes(&self) -> Option<u64> {
        self.size.as_deref().and_then(|s| s.parse().ok())
    }

    pub fn is_folder(&self) -> bool {
        self.mime_type == FOLDER_MIME
    }
}

#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<DriveEntry>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

pub async fn list_files(limit: usize) -> Result<Vec<DriveEntry>> {
    let access_token = get_google_access_token().await?;
    let client = Client::new();

    let mut entries = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        let remaining = limit.saturating_sub(entries.len());
        if remaining == 0 {
            break;
        }

        let mut query = vec![
            ("fields", "nextPageToken,files(id,name,size,mimeType,modifiedTime,webViewLink)".to_string()),
            ("orderBy", "modifiedTime desc".to_string()),
            ("q", "trashed = false".to_string()),
            ("pageSize", remaining.min(100).to_string()),
        ];
        if let Some(token) = &page_token {
            query.push(("pageToken", token.clone()));
        }

        let page = client
            .get("https://www.googleapis.com/drive/v3/files")
            .bearer_auth(&access_token)
            .query(&query)
            .send()
            .await?
            .error_for_status()?
            .json::<FileList>()
            .await?;

        entries.extend(page.files);

        match page.next_page_token {
            Some(token) => page_token = Some(token),
            None => break,
        }
    }

    Ok(entries)
}

fn escape_query(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

async fn query_files(q: &str, order: &str, limit: usize) -> Result<Vec<DriveEntry>> {
    let access_token = get_google_access_token().await?;

    let page = Client::new()
        .get("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(access_token)
        .query(&[
            ("fields", format!("files({FILE_FIELDS})")),
            ("q", q.to_string()),
            ("orderBy", order.to_string()),
            ("pageSize", limit.min(100).to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<FileList>()
        .await?;

    Ok(page.files)
}

fn looks_like_id(input: &str) -> bool {
    input.len() >= 20
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
