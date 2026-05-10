use anyhow::{Context, Result};
use claude_courier::config::{
    BrokerConfig, ConsumerConfig, ProxyConfig, PublisherConfig, broker_path, consumer_path,
    load_broker, load_consumer, load_proxy, load_publisher, proxy_path, publisher_path,
    resolve_consumer, resolve_proxy, resolve_publisher, save_broker, save_consumer, save_proxy,
    save_publisher,
};
use claude_courier::{broker, client, proxy};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "claude-courier", version, about = "publisher / broker / consumer + reverse proxy")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Publish a message read from stdin to the broker.
    Publish {
        #[arg(long = "broker")]
        broker_url: Option<String>,
        #[arg(long = "key")]
        publish_key: Option<String>,
    },
    /// Save publisher defaults (broker URL, publish key).
    PublishConfigure {
        #[arg(long = "broker")]
        broker_url: Option<String>,
        #[arg(long = "key")]
        publish_key: Option<String>,
    },
    /// Run the broker server.
    Broker {
        #[arg(long)]
        bind: Option<String>,
        #[arg(long)]
        publish_key: Option<String>,
        #[arg(long)]
        consume_key: Option<String>,
    },
    /// Save broker defaults (bind, publish key, consume key).
    BrokerConfigure {
        #[arg(long)]
        bind: Option<String>,
        #[arg(long)]
        publish_key: Option<String>,
        #[arg(long)]
        consume_key: Option<String>,
    },
    /// Consume the next pending message from the broker and write it to stdout.
    Consume {
        #[arg(long = "broker")]
        broker_url: Option<String>,
        #[arg(long = "key")]
        consume_key: Option<String>,
    },
    /// Save consumer defaults (broker URL, consume key).
    ConsumeConfigure {
        #[arg(long = "broker")]
        broker_url: Option<String>,
        #[arg(long = "key")]
        consume_key: Option<String>,
    },
    /// Run a reverse proxy in front of an LLM API (anthropic/openai/gemini or a URL).
    Proxy {
        #[arg(long)]
        listen: Option<String>,
        #[arg(long)]
        upstream: Option<String>,
    },
    /// Save proxy defaults (listen, upstream, inject_api_key, listen_key).
    ProxyConfigure {
        #[arg(long)]
        listen: Option<String>,
        #[arg(long)]
        upstream: Option<String>,
        #[arg(long)]
        inject_key: Option<String>,
        #[arg(long)]
        listen_key: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Publish {
            broker_url,
            publish_key,
        } => client::publish(resolve_publisher(broker_url, publish_key)?).await,

        Cmd::PublishConfigure {
            broker_url,
            publish_key,
        } => {
            let existing = load_publisher().ok();
            let cfg = PublisherConfig {
                broker_url: broker_url
                    .or_else(|| existing.as_ref().map(|c| c.broker_url.clone()))
                    .context("--broker required (or set previously)")?,
                publish_key: publish_key
                    .or_else(|| existing.as_ref().map(|c| c.publish_key.clone()))
                    .context("--key required (or set previously)")?,
            };
            save_publisher(&cfg)?;
            println!("saved {}", publisher_path().display());
            Ok(())
        }

        Cmd::Broker {
            bind,
            publish_key,
            consume_key,
        } => {
            let existing = load_broker().ok();
            let cfg = BrokerConfig {
                bind: bind
                    .or_else(|| existing.as_ref().map(|c| c.bind.clone()))
                    .unwrap_or_else(|| "127.0.0.1:3007".into()),
                publish_key: publish_key
                    .or_else(|| existing.as_ref().map(|c| c.publish_key.clone()))
                    .context("--publish-key required (or run `broker-configure` first)")?,
                consume_key: consume_key
                    .or_else(|| existing.as_ref().map(|c| c.consume_key.clone()))
                    .context("--consume-key required (or run `broker-configure` first)")?,
            };
            broker::run(cfg).await
        }

        Cmd::BrokerConfigure {
            bind,
            publish_key,
            consume_key,
        } => {
            let existing = load_broker().ok();
            let cfg = BrokerConfig {
                bind: bind
                    .or_else(|| existing.as_ref().map(|c| c.bind.clone()))
                    .unwrap_or_else(|| "127.0.0.1:3007".into()),
                publish_key: publish_key
                    .or_else(|| existing.as_ref().map(|c| c.publish_key.clone()))
                    .context("--publish-key required (or set previously)")?,
                consume_key: consume_key
                    .or_else(|| existing.as_ref().map(|c| c.consume_key.clone()))
                    .context("--consume-key required (or set previously)")?,
            };
            save_broker(&cfg)?;
            println!("saved {}", broker_path().display());
            Ok(())
        }

        Cmd::Consume {
            broker_url,
            consume_key,
        } => client::consume(resolve_consumer(broker_url, consume_key)?).await,

        Cmd::ConsumeConfigure {
            broker_url,
            consume_key,
        } => {
            let existing = load_consumer().ok();
            let cfg = ConsumerConfig {
                broker_url: broker_url
                    .or_else(|| existing.as_ref().map(|c| c.broker_url.clone()))
                    .context("--broker required (or set previously)")?,
                consume_key: consume_key
                    .or_else(|| existing.as_ref().map(|c| c.consume_key.clone()))
                    .context("--key required (or set previously)")?,
            };
            save_consumer(&cfg)?;
            println!("saved {}", consumer_path().display());
            Ok(())
        }

        Cmd::Proxy { listen, upstream } => proxy::run(resolve_proxy(listen, upstream)?).await,

        Cmd::ProxyConfigure {
            listen,
            upstream,
            inject_key,
            listen_key,
        } => {
            let existing = load_proxy().ok();
            let cfg = ProxyConfig {
                listen: listen
                    .or_else(|| existing.as_ref().map(|c| c.listen.clone()))
                    .unwrap_or_else(|| "127.0.0.1:8787".into()),
                upstream: upstream
                    .or_else(|| existing.as_ref().map(|c| c.upstream.clone()))
                    .unwrap_or_else(|| "anthropic".into()),
                inject_api_key: inject_key
                    .or_else(|| existing.as_ref().and_then(|c| c.inject_api_key.clone())),
                listen_key: listen_key
                    .or_else(|| existing.as_ref().and_then(|c| c.listen_key.clone())),
            };
            save_proxy(&cfg)?;
            println!("saved {}", proxy_path().display());
            Ok(())
        }
    }
}
