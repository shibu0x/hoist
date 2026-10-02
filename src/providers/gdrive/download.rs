use anyhow::Result;
use futures_util::TryStreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::{Client, StatusCode};
use std::path::Path;
use tokio::io::AsyncWriteExt;

use super::{DriveEntry, FILE_FIELDS, list_files, looks_like_id};
use crate::auth::get_google_access_token;

pub async fn file_metadata(id: &str) -> Result<DriveEntry> {
    let access_token = get_google_access_token().await?;

    let response = Client::new()
        .get(format!("https://www.googleapis.com/drive/v3/files/{id}"))
        .bearer_auth(access_token)
        .query(&[("fields", FILE_FIELDS)])
        .send()
        .await?;

    if response.status() == StatusCode::NOT_FOUND {
        anyhow::bail!(
            "no file with id {id} - it may not exist, or was not created by transit \
             (the drive.file scope cannot see the rest of your Drive)"
        );
    }

    Ok(response.error_for_status()?.json::<DriveEntry>().await?)
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
    } else if resuming {
        println!(
            "Resuming from {}",
            indicatif::HumanBytes(have)
        );
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
