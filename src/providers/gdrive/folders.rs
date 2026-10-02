use anyhow::{Context, Result};
use reqwest::Client;

use super::{DriveEntry, FILE_FIELDS, FOLDER_MIME, ROOT, escape_query, looks_like_id, query_files};
use crate::auth::get_google_access_token;

pub async fn list_folder(parent: &str, limit: usize) -> Result<Vec<DriveEntry>> {
    query_files(
        &format!("'{}' in parents and trashed = false", escape_query(parent)),
        "folder,name",
        limit,
    )
    .await
}

async fn find_folder(name: &str, parent: &str) -> Result<Option<DriveEntry>> {
    let found = query_files(
        &format!(
            "name = '{}' and mimeType = '{FOLDER_MIME}' and '{}' in parents and trashed = false",
            escape_query(name),
            escape_query(parent)
        ),
        "name",
        1,
    )
    .await?;

    Ok(found.into_iter().next())
}

async fn create_folder(name: &str, parent: &str) -> Result<DriveEntry> {
    let access_token = get_google_access_token().await?;

    Ok(Client::new()
        .post("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(access_token)
        .query(&[("fields", FILE_FIELDS)])
        .json(&serde_json::json!({
            "name": name,
            "mimeType": FOLDER_MIME,
            "parents": [parent],
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<DriveEntry>()
        .await?)
}

/// `mkdir -p` for Drive: walks a slash-separated path, creating any segment
/// that does not exist. Returns the id of the last folder.
pub async fn ensure_folder_path(path: &str) -> Result<String> {
    let mut parent = ROOT.to_string();

    for segment in path.split('/').filter(|s| !s.trim().is_empty()) {
        parent = match find_folder(segment, &parent).await? {
            Some(found) => found.id,
            None => {
                let created = create_folder(segment, &parent).await?;
                println!("Created folder {segment}");
                created.id
            }
        };
    }

    Ok(parent)
}

/// Resolves an existing folder by id or by path. Unlike `ensure_folder_path`
/// this never creates anything - listing a typo'd folder should say so, not
/// silently make an empty one.
pub async fn resolve_folder(path: &str) -> Result<String> {
    if looks_like_id(path) {
        return Ok(path.to_string());
    }

    let mut parent = ROOT.to_string();
    for segment in path.split('/').filter(|s| !s.trim().is_empty()) {
        parent = find_folder(segment, &parent)
            .await?
            .with_context(|| format!("no folder named {segment:?} in {path:?}"))?
            .id;
    }

    Ok(parent)
}
