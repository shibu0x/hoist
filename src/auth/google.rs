use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

use super::credentials::OAuthClient;
use sha2::{Digest, Sha256};
use std::io::Read;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const AUTH_URI: &str = "https://accounts.google.com/o/oauth2/auth";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

pub async fn gdrive_auth() -> anyhow::Result<()> {
    let oauth = super::credentials::configure("google")?;

    println!("Starting gdrive authentication");

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let redirect_uri = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());

    let state = random_urlsafe()?;
    let verifier = random_urlsafe()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let auth_url = build_auth_url(&oauth, &redirect_uri, &state, &challenge);

    println!("opening browser...");

    webbrowser::open(&auth_url).expect("failed to open the browser");

    println!(
        "Complete the authentication, if browser doesn't open paste this link and open in your browser : {:?}",
        auth_url
    );

    let code = wait_for_callback(listener, &state).await?;

    println!("Authorization code received!");

    let tokens = exchange_code(&code, &oauth, &redirect_uri, &verifier).await?;

    if let Some(refresh_token) = &tokens.refresh_token {
        crate::auth::token_store::save_refresh_token("google", refresh_token)?;
    } else {
        anyhow::bail!("Google did not return a refresh token");
    }

    println!("Google Auth is Successfull!");
    Ok(())
}

fn random_urlsafe() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn build_auth_url(
    oauth: &OAuthClient,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}&code_challenge={}&code_challenge_method=S256",
        AUTH_URI,
        urlencoding::encode(&oauth.client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(SCOPE),
        urlencoding::encode(state),
        urlencoding::encode(code_challenge),
    )
}

async fn wait_for_callback(listener: TcpListener, expected_state: &str) -> anyhow::Result<String> {
    println!("Waiting for Google authorization...");

    let (mut socket, _) = listener.accept().await?;

    let mut buffer = [0u8; 4096];

    let bytes_read = socket.read(&mut buffer).await?;

    let request = String::from_utf8_lossy(&buffer[..bytes_read]);

    let request_line = request
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Invalid HTTP request"))?;

    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Invalid HTTP request Line"))?;

    let url = url::Url::parse(&format!("http://localhost{}", path))?;

    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };

    let state_ok = param("state").as_deref() == Some(expected_state);

    let response = if state_ok {
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nTransit authorization successful. You can close this window."
    } else {
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nTransit rejected this callback: state mismatch."
    };
    socket.write_all(response.as_bytes()).await?;

    if !state_ok {
        anyhow::bail!("OAuth state mismatch - callback did not come from the request transit started");
    }

    if let Some(error) = param("error") {
        anyhow::bail!("Google denied authorization: {error}");
    }

    param("code").ok_or_else(|| anyhow::anyhow!("Authorization code not found"))
}

async fn exchange_code(
    code: &str,
    oauth: &OAuthClient,
    redirect_uri: &str,
    code_verifier: &str,
) -> anyhow::Result<TokenResponse> {
    let client = reqwest::Client::new();

    let params = [
        ("code", code),
        ("client_id", oauth.client_id.as_str()),
        (
            "client_secret",
            oauth.client_secret.as_str(),
        ),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
        ("code_verifier", code_verifier),
    ];

    let response = client
        .post(TOKEN_URI)
        .form(&params)
        .send()
        .await?;

    let tokens = response.error_for_status()?.json::<TokenResponse>().await?;

    Ok(tokens)
}

async fn refresh_access_token(
    refresh_token: &str,
    oauth: &OAuthClient,
) -> anyhow::Result<TokenResponse> {
    let client = reqwest::Client::new();

    let params = [
        ("client_id", oauth.client_id.as_str()),
        (
            "client_secret",
            oauth.client_secret.as_str(),
        ),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    let response = client
        .post(TOKEN_URI)
        .form(&params)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        anyhow::bail!("Failed to refresh access token");
    }

    let tokens: TokenResponse = serde_json::from_str(&body)?;

    Ok(tokens)
}

pub async fn get_google_access_token() -> anyhow::Result<String> {
    if let Some(access_token) = crate::auth::token_store::get_cached_access_token("google")? {
        return Ok(access_token);
    }

    let oauth = super::credentials::get("google")?
        .ok_or_else(|| anyhow::anyhow!("not connected to google - run `transit config` first"))?;

    let refresh_token = crate::auth::token_store::get_refresh_token("google")?;

    let tokens = refresh_access_token(&refresh_token, &oauth).await?;

    crate::auth::token_store::cache_access_token(
        "google",
        &tokens.access_token,
        tokens.expires_in,
    )?;

    Ok(tokens.access_token)
}
