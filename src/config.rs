use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PublisherConfig {
    pub broker_url: String,
    pub publish_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BrokerConfig {
    pub bind: String,
    pub publish_key: String,
    pub consume_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConsumerConfig {
    pub broker_url: String,
    pub consume_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProxyConfig {
    pub listen: String,
    pub upstream: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_key: Option<String>,
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .expect("no config dir")
        .join("claude-courier")
}

pub fn publisher_path() -> PathBuf {
    config_dir().join("publisher.toml")
}
pub fn broker_path() -> PathBuf {
    config_dir().join("broker.toml")
}
pub fn consumer_path() -> PathBuf {
    config_dir().join("consumer.toml")
}
pub fn proxy_path() -> PathBuf {
    config_dir().join("proxy.toml")
}

fn write_secure(path: &PathBuf, contents: &str) -> Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn save_publisher(cfg: &PublisherConfig) -> Result<()> {
    write_secure(&publisher_path(), &toml::to_string_pretty(cfg)?)
}
pub fn save_broker(cfg: &BrokerConfig) -> Result<()> {
    write_secure(&broker_path(), &toml::to_string_pretty(cfg)?)
}
pub fn save_consumer(cfg: &ConsumerConfig) -> Result<()> {
    write_secure(&consumer_path(), &toml::to_string_pretty(cfg)?)
}
pub fn save_proxy(cfg: &ProxyConfig) -> Result<()> {
    write_secure(&proxy_path(), &toml::to_string_pretty(cfg)?)
}

pub fn load_publisher() -> Result<PublisherConfig> {
    let p = publisher_path();
    let s = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    toml::from_str(&s).context("parse publisher.toml")
}
pub fn load_broker() -> Result<BrokerConfig> {
    let p = broker_path();
    let s = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    toml::from_str(&s).context("parse broker.toml")
}
pub fn load_consumer() -> Result<ConsumerConfig> {
    let p = consumer_path();
    let s = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    toml::from_str(&s).context("parse consumer.toml")
}
pub fn load_proxy() -> Result<ProxyConfig> {
    let p = proxy_path();
    let s = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    toml::from_str(&s).context("parse proxy.toml")
}

pub fn resolve_publisher(
    broker_url: Option<String>,
    publish_key: Option<String>,
) -> Result<PublisherConfig> {
    let f = load_publisher().ok();
    Ok(PublisherConfig {
        broker_url: broker_url
            .or_else(|| f.as_ref().map(|c| c.broker_url.clone()))
            .context("no broker_url: provide --broker or run `publish-configure` first")?,
        publish_key: publish_key
            .or_else(|| f.as_ref().map(|c| c.publish_key.clone()))
            .context("no publish_key: provide --key or run `publish-configure` first")?,
    })
}

pub fn resolve_proxy(
    listen: Option<String>,
    upstream: Option<String>,
) -> Result<ProxyConfig> {
    let f = load_proxy().ok();
    Ok(ProxyConfig {
        listen: listen
            .or_else(|| f.as_ref().map(|c| c.listen.clone()))
            .unwrap_or_else(|| "127.0.0.1:8787".into()),
        upstream: upstream
            .or_else(|| f.as_ref().map(|c| c.upstream.clone()))
            .context("no upstream: provide --upstream or run `proxy-configure` first")?,
        inject_api_key: f.as_ref().and_then(|c| c.inject_api_key.clone()),
        listen_key: f.as_ref().and_then(|c| c.listen_key.clone()),
    })
}

pub fn resolve_consumer(
    broker_url: Option<String>,
    consume_key: Option<String>,
) -> Result<ConsumerConfig> {
    let f = load_consumer().ok();
    Ok(ConsumerConfig {
        broker_url: broker_url
            .or_else(|| f.as_ref().map(|c| c.broker_url.clone()))
            .context("no broker_url: provide --broker or run `consume-configure` first")?,
        consume_key: consume_key
            .or_else(|| f.as_ref().map(|c| c.consume_key.clone()))
            .context("no consume_key: provide --key or run `consume-configure` first")?,
    })
}
