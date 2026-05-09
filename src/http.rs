use anyhow::{Context, Result, bail};
use std::time::Duration;

pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .context("build reqwest client")
}

/// Authenticated GET. Returns body String, errors on non-2xx (except 410 which
/// is returned via the `Gone` variant so callers can tell expired-token apart
/// from real errors).
pub enum FetchResult {
    Ok(String),
    Gone(String),
}

pub async fn auth_get(client: &reqwest::Client, url: &str, key: &str) -> Result<FetchResult> {
    let resp = client
        .get(url)
        .header("x-api-key", key)
        .send()
        .await
        .with_context(|| format!("GET {} failed", url))?;
    let status = resp.status();
    let body = resp.text().await.context("read body")?;
    if status.as_u16() == 410 {
        return Ok(FetchResult::Gone(body));
    }
    if !status.is_success() {
        bail!("HTTP {} from {}: {}", status.as_u16(), url, body);
    }
    Ok(FetchResult::Ok(body))
}

pub async fn auth_post_json(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    body: &str,
) -> Result<()> {
    let resp = client
        .post(url)
        .header("x-api-key", key)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .with_context(|| format!("POST {} failed", url))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("HTTP {} from POST {}: {}", status.as_u16(), url, text);
    }
    Ok(())
}
