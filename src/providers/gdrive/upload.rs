use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE};
use reqwest::{Client, StatusCode};
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{DriveFile, ROOT};
use crate::auth::get_google_access_token;
use crate::auth::token_store::{config_file, load_from, save_to};

const SESSIONS_FILE: &str = "uploads.json";

/// Google requires a multiple of 256 KiB. Every chunk costs a round trip, so
/// tiny chunks are murder on a high-latency link; large chunks mean a failure
/// throws away more work. 8 MiB is the middle ground, and is also how much
/// memory one chunk occupies.
const CHUNK: u64 = 8 * 1024 * 1024;
const MAX_RETRIES: u32 = 5;

pub async fn upload_file(path: &Path, parent: Option<&str>) -> Result<DriveFile> {
    let access_token = get_google_access_token().await?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("Failed to get file name"))?;

    let mut file = tokio::fs::File::open(path).await?;
    let meta = file.metadata().await?;
    let total = meta.len();

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let (account_id, _) = crate::auth::accounts::active_for("google")?;
    let key = session_key(&account_id, parent.unwrap_or(ROOT), path, &meta)?;

    let (session_uri, mut offset) = match saved_session(&key)? {
        Some(uri) => match server_offset(&client, &uri, total).await {
            Ok(Some(at)) => {
                println!("Resuming at {}%", at * 100 / total.max(1));
                (uri, at)
            }
            Ok(None) => (start_session(&client, &access_token, file_name, total, parent).await?, 0),
            Err(_) => (start_session(&client, &access_token, file_name, total, parent).await?, 0),
        },
        None => (start_session(&client, &access_token, file_name, total, parent).await?, 0),
    };
    remember_session(&key, &session_uri)?;

    let progress = ProgressBar::new(total);
    progress.set_style(
        ProgressStyle::with_template(
            "{bar:40.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
        )?
        .progress_chars("=>-"),
    );
    progress.set_position(offset);

    let mut attempt = 0;
    loop {
        let end = (offset + CHUNK).min(total);
        let mut body = vec![0u8; (end - offset) as usize];
        if !body.is_empty() {
            file.seek(SeekFrom::Start(offset)).await?;
            file.read_exact(&mut body).await?;
        }

        let mut request = client.put(&session_uri);
        request = if total == 0 {
            // A zero-byte file has no range to describe; an empty final PUT
            // closes the session.
            request
        } else {
            request.header(CONTENT_RANGE, format!("bytes {}-{}/{}", offset, end - 1, total))
        };

        match request.body(body).send().await {
            Ok(response) => match response.status() {
                // More to send. Trust the server's count, not ours.
                StatusCode::PERMANENT_REDIRECT => {
                    offset = acknowledged(response.headers().get(RANGE));
                    progress.set_position(offset);
                    attempt = 0;
                }
                StatusCode::OK | StatusCode::CREATED => {
                    let file = response.json::<DriveFile>().await?;
                    progress.finish_and_clear();
                    forget_session(&key)?;
                    return Ok(file);
                }
                
                StatusCode::NOT_FOUND | StatusCode::GONE => {
                    progress.abandon();
                    forget_session(&key)?;
                    anyhow::bail!("upload session expired - run the command again to restart");
                }
                status if status.is_server_error() => {
                    offset = retry(&client, &session_uri, total, &mut attempt, offset).await?;
                    progress.set_position(offset);
                }
                status => {
                    progress.abandon();
                    let body = response.text().await.unwrap_or_default();
                    anyhow::bail!("upload failed with {status}: {body}");
                }
            },
            
            Err(_) => {
                offset = retry(&client, &session_uri, total, &mut attempt, offset).await?;
                progress.set_position(offset);
            }
        }
    }
}

/// Backs off, then re-syncs the offset from the server. Returns the byte the
/// next chunk should start at.
async fn retry(
    client: &Client,
    session_uri: &str,
    total: u64,
    attempt: &mut u32,
    current: u64,
) -> Result<u64> {
    if *attempt >= MAX_RETRIES {
        anyhow::bail!(
            "upload failed after {MAX_RETRIES} retries - progress is saved, run the command again to resume"
        );
    }
    *attempt += 1;
    tokio::time::sleep(Duration::from_secs(1 << *attempt)).await;

    match server_offset(client, session_uri, total).await {
        Ok(Some(at)) => Ok(at),
        _ => Ok(current),
    }
}

async fn start_session(
    client: &Client,
    access_token: &str,
    file_name: &str,
    total: u64,
    parent: Option<&str>,
) -> Result<String> {
    let mut metadata = serde_json::json!({ "name": file_name });
    if let Some(parent) = parent {
        metadata["parents"] = serde_json::json!([parent]);
    }

    let response = client
        .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable&fields=id,name,webViewLink")
        .bearer_auth(access_token)
        .header(CONTENT_TYPE, "application/json; charset=UTF-8")
        .header("X-Upload-Content-Type", "application/octet-stream")
        .header("X-Upload-Content-Length", total.to_string())
        .body(metadata.to_string())
        .send()
        .await?
        .error_for_status()?;

    response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .context("Drive did not return an upload session URI")
}

/// Asks the server how much it holds. `Ok(None)` means the upload is already
/// complete; `Err` means the session is gone.
async fn server_offset(client: &Client, session_uri: &str, total: u64) -> Result<Option<u64>> {
    let response = client
        .put(session_uri)
        .header(CONTENT_RANGE, format!("bytes */{total}"))
        .send()
        .await?;

    match response.status() {
        StatusCode::PERMANENT_REDIRECT => Ok(Some(acknowledged(response.headers().get(RANGE)))),
        StatusCode::OK | StatusCode::CREATED => Ok(None),
        status => anyhow::bail!("upload session unusable: {status}"),
    }
}

/// `Range: bytes=0-8388607` means 8388608 bytes are stored. A missing header
/// means the server has nothing yet.
fn acknowledged(range: Option<&reqwest::header::HeaderValue>) -> u64 {
    range
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit('-').next())
        .and_then(|last| last.parse::<u64>().ok())
        .map_or(0, |last| last + 1)
}

// --- session persistence, so a crash or Ctrl-C can still resume -------------

fn session_key(
    account_id: &str,
    parent: &str,
    path: &Path,
    meta: &std::fs::Metadata,
) -> Result<String> {
    let modified = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    Ok(format!(
        "{account_id}|{parent}|{}:{}:{}",
        path.canonicalize()?.display(),
        meta.len(),
        modified
    ))
}

fn saved_session(key: &str) -> Result<Option<String>> {
    let mut sessions: HashMap<String, String> = load_from(&config_file(SESSIONS_FILE)?)?;
    Ok(sessions.remove(key))
}

fn remember_session(key: &str, session_uri: &str) -> Result<()> {
    let path = config_file(SESSIONS_FILE)?;
    let mut sessions: HashMap<String, String> = load_from(&path)?;
    sessions.insert(key.to_string(), session_uri.to_string());
    save_to(&path, &sessions)
}

fn forget_session(key: &str) -> Result<()> {
    let path = config_file(SESSIONS_FILE)?;
    let mut sessions: HashMap<String, String> = load_from(&path)?;
    if sessions.remove(key).is_some() {
        save_to(&path, &sessions)?;
    }
    Ok(())
}

pub fn forget_sessions_for(account_id: &str) -> Result<()> {
    let path = config_file(SESSIONS_FILE)?;
    let mut sessions: HashMap<String, String> = load_from(&path)?;
    let prefix = format!("{account_id}|");

    let before = sessions.len();
    sessions.retain(|key, _| !key.starts_with(&prefix));
    if sessions.len() != before {
        save_to(&path, &sessions)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn acknowledged_converts_last_byte_to_next_offset() {
        let range = HeaderValue::from_static("bytes=0-8388607");
        assert_eq!(acknowledged(Some(&range)), 8_388_608);

        let single = HeaderValue::from_static("bytes=0-0");
        assert_eq!(acknowledged(Some(&single)), 1, "one stored byte");

        assert_eq!(acknowledged(None), 0, "no Range header means server has nothing");

        let junk = HeaderValue::from_static("bytes=garbage");
        assert_eq!(acknowledged(Some(&junk)), 0, "unparseable falls back to restart");
    }
}
