use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path;
use std::time::Duration;
use futures_util::TryStreamExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::auth::get_google_access_token;
use crate::auth::token_store::{config_file, load_from, save_to};

const SESSIONS_FILE: &str = "uploads.json";

/// Google requires a multiple of 256 KiB. Every chunk costs a round trip, so
/// tiny chunks are murder on a high-latency link; large chunks mean a failure
/// throws away more work. 8 MiB is the middle ground, and is also how much
/// memory one chunk occupies.
const CHUNK: u64 = 8 * 1024 * 1024;
const MAX_RETRIES: u32 = 5;

#[derive(Debug, Deserialize)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    #[serde(rename = "webViewLink")]
    pub web_view_link: Option<String>,
}

#[derive(Debug, Deserialize)]
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
        self.mime_type == "application/vnd.google-apps.folder"
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

pub async fn upload_file(path: &Path) -> Result<DriveFile> {
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
    let key = session_key(&account_id, path, &meta)?;

    let (session_uri, mut offset) = match saved_session(&key)? {
        Some(uri) => match server_offset(&client, &uri, total).await {
            Ok(Some(at)) => {
                println!("Resuming at {}%", at * 100 / total.max(1));
                (uri, at)
            }
            Ok(None) => (start_session(&client, &access_token, file_name, total).await?, 0),
            Err(_) => (start_session(&client, &access_token, file_name, total).await?, 0),
        },
        None => (start_session(&client, &access_token, file_name, total).await?, 0),
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
                // The session is gone; nothing to resume onto.
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
            // Connection dropped mid-chunk. Ask the server what it kept.
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
        // Complete, or unreachable: keep going from where we were and let the
        // next round decide.
        _ => Ok(current),
    }
}

async fn start_session(
    client: &Client,
    access_token: &str,
    file_name: &str,
    total: u64,
) -> Result<String> {
    let response = client
        .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable&fields=id,name,webViewLink")
        .bearer_auth(access_token)
        .header(CONTENT_TYPE, "application/json; charset=UTF-8")
        .header("X-Upload-Content-Type", "application/octet-stream")
        .header("X-Upload-Content-Length", total.to_string())
        .body(serde_json::json!({ "name": file_name }).to_string())
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
//
// The session URI is a capability: anyone holding it can write into this
// upload. It is stored alongside the tokens at 0600 for that reason.

/// Size and mtime are part of the key so an edited file starts a fresh upload
/// rather than resuming onto stale bytes.
fn session_key(account_id: &str, path: &Path, meta: &std::fs::Metadata) -> Result<String> {
    let modified = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    Ok(format!(
        "{account_id}|{}:{}:{}",
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
    // ponytail: abandoned uploads leave a row here forever. Prune on age if it
    // ever grows enough to notice - Google expires sessions after ~a week.
    if sessions.remove(key).is_some() {
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
        // "bytes=0-8388607" means bytes 0..=8388607 are stored, so the next
        // chunk starts at 8388608. Off by one here either re-sends a byte or
        // skips one, and Drive would reject or corrupt the upload.
        let range = HeaderValue::from_static("bytes=0-8388607");
        assert_eq!(acknowledged(Some(&range)), 8_388_608);

        let single = HeaderValue::from_static("bytes=0-0");
        assert_eq!(acknowledged(Some(&single)), 1, "one stored byte");

        assert_eq!(acknowledged(None), 0, "no Range header means server has nothing");

        let junk = HeaderValue::from_static("bytes=garbage");
        assert_eq!(acknowledged(Some(&junk)), 0, "unparseable falls back to restart");
    }
}

const FILE_FIELDS: &str = "id,name,size,mimeType,modifiedTime,webViewLink";

pub async fn file_metadata(id: &str) -> Result<DriveEntry> {
    let access_token = get_google_access_token().await?;

    Ok(Client::new()
        .get(format!("https://www.googleapis.com/drive/v3/files/{id}"))
        .bearer_auth(access_token)
        .query(&[("fields", FILE_FIELDS)])
        .send()
        .await?
        .error_for_status()?
        .json::<DriveEntry>()
        .await?)
}

/// Accepts a Drive link, a bare file id, or a file name.
pub async fn resolve(input: &str) -> Result<DriveEntry> {
    if let Some(id) = id_from_link(input) {
        return file_metadata(&id).await;
    }
    if looks_like_id(input) {
        return file_metadata(input).await;
    }

    let mut matches: Vec<DriveEntry> = list_files(500)
        .await?
        .into_iter()
        .filter(|entry| entry.name == input)
        .collect();

    match matches.len() {
        0 => anyhow::bail!("no file named {input:?} - run `transit list` to see what is there"),
        1 => Ok(matches.remove(0)),
        n => {
            let links: Vec<String> = matches
                .iter()
                .map(|e| format!("  {} {}", e.modified_time.get(..16).unwrap_or(""), e.id))
                .collect();
            anyhow::bail!(
                "{n} files named {input:?} - pass an id or link instead:\n{}",
                links.join("\n")
            )
        }
    }
}

fn id_from_link(input: &str) -> Option<String> {
    if !input.starts_with("http") {
        return None;
    }
    let parts: Vec<&str> = input.split('/').collect();
    parts
        .iter()
        .position(|part| *part == "d" || *part == "folders")
        .and_then(|at| parts.get(at + 1))
        .map(|id| id.split('?').next().unwrap_or(id).to_string())
}

fn looks_like_id(input: &str) -> bool {
    input.len() >= 20
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub async fn download_file(entry: &DriveEntry, dest: &Path) -> Result<()> {
    anyhow::ensure!(
        !entry.mime_type.starts_with("application/vnd.google-apps"),
        "{} is a Google {} and has no binary form to download",
        entry.name,
        entry.mime_type.rsplit('.').next().unwrap_or("document")
    );

    let access_token = get_google_access_token().await?;
    let total = entry.size_bytes();

    let have = tokio::fs::metadata(dest).await.map(|m| m.len()).unwrap_or(0);
    if Some(have) == total && have > 0 {
        println!("Already downloaded: {}", dest.display());
        return Ok(());
    }

    let mut request = Client::new()
        .get(format!(
            "https://www.googleapis.com/drive/v3/files/{}?alt=media",
            entry.id
        ))
        .bearer_auth(access_token);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }

    let response = request.send().await?.error_for_status()?;
    let resuming = response.status() == StatusCode::PARTIAL_CONTENT;
    let start = if resuming { have } else { 0 };
    if have > 0 && !resuming {
        println!("Server sent the whole file; restarting the download.");
    }

    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resuming)
        .truncate(!resuming)
        .open(dest)
        .await?;

    let progress = ProgressBar::new(total.unwrap_or(0));
    progress.set_style(
        ProgressStyle::with_template(
            "{bar:40.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
        )?
        .progress_chars("=>-"),
    );
    progress.set_position(start);

    let stream = response
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let mut reader = progress.wrap_async_read(tokio_util::io::StreamReader::new(stream));
    let mut writer = file;
    tokio::io::copy(&mut reader, &mut writer).await?;
    writer.flush().await?;
    progress.finish_and_clear();

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
