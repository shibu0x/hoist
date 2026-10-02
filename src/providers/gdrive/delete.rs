use anyhow::Result;
use reqwest::{Client, StatusCode};

use super::{DriveEntry, list_folder};
use crate::auth::get_google_access_token;

const CHILD_PROBE: usize = 100;

pub async fn trash(id: &str) -> Result<()> {
    let access_token = get_google_access_token().await?;

    Client::new()
        .patch(format!("https://www.googleapis.com/drive/v3/files/{id}"))
        .bearer_auth(access_token)
        .json(&serde_json::json!({ "trashed": true }))
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

pub async fn delete_forever(id: &str) -> Result<()> {
    let access_token = get_google_access_token().await?;

    let response = Client::new()
        .delete(format!("https://www.googleapis.com/drive/v3/files/{id}"))
        .bearer_auth(access_token)
        .send()
        .await?;

    if response.status() == StatusCode::FORBIDDEN {
        anyhow::bail!(
            "not allowed to delete {id} - transit can only delete files it created"
        );
    }

    response.error_for_status()?;
    Ok(())
}

/// How many items a folder holds, for the confirmation prompt. Deleting a
/// folder takes its contents with it, so the count has to be shown before
/// anyone agrees to it. Capped - the exact number past 100 does not change the
/// decision.
pub async fn describe_contents(entry: &DriveEntry) -> Result<Option<String>> {
    if !entry.is_folder() {
        return Ok(None);
    }

    let children = list_folder(&entry.id, CHILD_PROBE).await?;
    Ok(Some(match children.len() {
        0 => "empty".to_string(),
        n if n >= CHILD_PROBE => format!("{CHILD_PROBE}+ items inside"),
        1 => "1 item inside".to_string(),
        n => format!("{n} items inside"),
    }))
}
