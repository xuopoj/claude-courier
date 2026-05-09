use crate::config::{ConsumerConfig, PublisherConfig};
use crate::http::{FetchResult, auth_get, auth_post_json, client};
use crate::log::log;
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};

pub async fn publish(cfg: PublisherConfig) -> Result<()> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read stdin")?;
    if buf.trim().is_empty() {
        bail!("nothing to publish: stdin was empty");
    }
    let url = format!("{}/publish", cfg.broker_url.trim_end_matches('/'));
    auth_post_json(&client()?, &url, &cfg.publish_key, &buf).await?;
    log("published 1 message");
    Ok(())
}

pub async fn consume(cfg: ConsumerConfig) -> Result<()> {
    let url = format!("{}/consume", cfg.broker_url.trim_end_matches('/'));
    match auth_get(&client()?, &url, &cfg.consume_key).await? {
        FetchResult::Ok(body) => {
            std::io::stdout()
                .write_all(body.as_bytes())
                .context("write stdout")?;
            Ok(())
        }
        FetchResult::Gone(_) => bail!("no messages pending"),
    }
}
