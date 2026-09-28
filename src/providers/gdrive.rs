use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;
use std::path::Path;

use crate::auth::get_google_access_token;


#[derive(Debug, Deserialize)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
}

pub async fn upload_file(path: &Path) -> Result<DriveFile> {
    let access_token = get_google_access_token().await?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("Failed to get file name"))?;

    let file = tokio::fs::File::open(path).await?;
    let file_len = file.metadata().await?.len();
    let file_stream = tokio_util::io::ReaderStream::new(file);

    let client = Client::new();

    let metadata = serde_json::json!({
        "name": file_name
    });

    let metadata_part =
        reqwest::multipart::Part::text(metadata.to_string()).mime_str("application/json")?;

    let file_part = reqwest::multipart::Part::stream_with_length(
        reqwest::Body::wrap_stream(file_stream),
        file_len,
    )
    .mime_str("application/octet-stream")?;

    let form = reqwest::multipart::Form::new()
        .part("metadata", metadata_part)
        .part("file", file_part);

    let response = client
        .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart")
        .bearer_auth(access_token)
        .multipart(form)
        .send()
        .await?;

    let file = response.error_for_status()?.json::<DriveFile>().await?;

    Ok(file)
}
