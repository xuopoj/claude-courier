use crate::config::{ConsumerConfig, PublisherConfig};
use crate::http::{FetchResult, auth_get, auth_post_json, client};
use crate::log::log;
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn home() -> Result<PathBuf> {
    dirs::home_dir().context("no home dir")
}

fn creds_path() -> Result<PathBuf> {
    Ok(home()?.join(".claude/.credentials.json"))
}

fn claude_json_path() -> Result<PathBuf> {
    Ok(home()?.join(".claude.json"))
}

fn read_keychain_credentials() -> Result<String> {
    if !cfg!(target_os = "macos") {
        bail!("publishing requires macOS Keychain — run on a Mac that's logged into Claude Code");
    }
    let output = Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .context("run `security` failed")?;
    if !output.status.success() {
        bail!(
            "Claude credentials not found in Keychain — make sure Claude Code is logged in on this Mac"
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn read_identity() -> Result<Value> {
    let path = claude_json_path()?;
    let data = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let src: Value = serde_json::from_str(&data).context("parse ~/.claude.json")?;
    let mut out = Map::new();
    for key in ["userID", "oauthAccount", "hasCompletedOnboarding", "firstStartTime"] {
        if let Some(v) = src.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    if !out.contains_key("userID") || !out.contains_key("oauthAccount") {
        bail!(
            "~/.claude.json missing userID or oauthAccount — is Claude Code logged in on this Mac?"
        );
    }
    out.insert("hasCompletedOnboarding".into(), Value::Bool(true));
    Ok(Value::Object(out))
}

pub async fn publish(cfg: PublisherConfig) -> Result<()> {
    let credentials_raw = read_keychain_credentials()?;
    let mut credentials: Value =
        serde_json::from_str(&credentials_raw).context("Keychain payload is not valid JSON")?;
    let token_present = credentials
        .get("claudeAiOauth")
        .and_then(|v| v.get("accessToken"))
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if !token_present {
        bail!("Keychain credentials missing claudeAiOauth.accessToken");
    }

    // Strip refreshToken before publishing — keeping the long-lived secret out
    // of the broker's slot. Consumers will lose access when the access token
    // expires until the publisher publishes again.
    if let Some(oauth) = credentials
        .get_mut("claudeAiOauth")
        .and_then(|v| v.as_object_mut())
    {
        oauth.remove("refreshToken");
    }

    let identity = read_identity()?;
    let envelope = json!({"credentials": credentials, "identity": identity});
    let body = serde_json::to_string(&envelope)?;

    let url = format!("{}/publish", cfg.broker_url.trim_end_matches('/'));
    auth_post_json(&client()?, &url, &cfg.publish_key, &body).await?;
    log("published Claude credentials + identity");
    Ok(())
}

fn atomic_write(dest: &PathBuf, contents: &str) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = dest.with_extension("tmp");
    {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)
            .with_context(|| format!("open {}", tmp_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, dest)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), dest.display()))?;
    Ok(())
}

fn merge_identity(identity: &Value) -> Result<Vec<String>> {
    let identity_obj = identity
        .as_object()
        .context("identity was not a JSON object")?;
    let path = claude_json_path()?;
    let mut merged: Map<String, Value> = if path.exists() {
        let existing = fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let parsed: Value = serde_json::from_str(&existing)
            .with_context(|| format!("parse {}", path.display()))?;
        parsed
            .as_object()
            .context("~/.claude.json is not a JSON object")?
            .clone()
    } else {
        Map::new()
    };
    let mut added = Vec::new();
    for (k, v) in identity_obj {
        merged.insert(k.clone(), v.clone());
        added.push(k.clone());
    }
    let serialized = serde_json::to_string_pretty(&Value::Object(merged))?;
    atomic_write(&path, &serialized)?;
    Ok(added)
}

pub async fn consume(cfg: ConsumerConfig) -> Result<()> {
    let url = format!("{}/consume", cfg.broker_url.trim_end_matches('/'));
    let body = match auth_get(&client()?, &url, &cfg.consume_key).await? {
        FetchResult::Ok(b) => b,
        FetchResult::Gone(_) => bail!("broker has no token published yet"),
    };

    let envelope: Value =
        serde_json::from_str(&body).context("broker response was not valid JSON")?;
    let credentials = envelope
        .get("credentials")
        .context("envelope missing `credentials`")?;
    let identity = envelope
        .get("identity")
        .context("envelope missing `identity`")?;

    let token_present = credentials
        .get("claudeAiOauth")
        .and_then(|v| v.get("accessToken"))
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if !token_present {
        bail!("credentials missing claudeAiOauth.accessToken");
    }

    let creds_dest = creds_path()?;
    atomic_write(&creds_dest, &serde_json::to_string(credentials)?)?;
    log(&format!("wrote {}", creds_dest.display()));

    let added = merge_identity(identity)?;
    log(&format!(
        "merged {} identity field(s) into ~/.claude.json: {}",
        added.len(),
        added.join(", ")
    ));
    Ok(())
}
