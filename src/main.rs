use anyhow::{Context, Result};
use claude_courier::config::{
    BrokerConfig, ConsumerConfig, ProxyConfig, PublisherConfig, RouterConfig, RouterKey,
    broker_path, consumer_path, load_broker, load_consumer, load_proxy, load_publisher,
    load_router, proxy_path, publisher_path, resolve_consumer, resolve_proxy, resolve_publisher,
    resolve_router, router_path, save_broker, save_consumer, save_proxy, save_publisher,
    save_router,
};
use claude_courier::{broker, client, proxy, router};
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
    /// Run an OAuth-injecting router: pulls fresh tokens from the broker and
    /// adds Authorization: Bearer to upstream requests so clients can use
    /// ANTHROPIC_BASE_URL + a local API key without ever touching the OAuth token.
    Router {
        #[arg(long)]
        listen: Option<String>,
        #[arg(long = "broker")]
        broker_url: Option<String>,
        #[arg(long = "consume-key")]
        consume_key: Option<String>,
        #[arg(long)]
        upstream: Option<String>,
    },
    /// Save router defaults (listen, broker, consume_key, upstream). Manage
    /// per-consumer keys with `router-key`.
    RouterConfigure {
        #[arg(long)]
        listen: Option<String>,
        #[arg(long = "broker")]
        broker_url: Option<String>,
        #[arg(long = "consume-key")]
        consume_key: Option<String>,
        #[arg(long)]
        upstream: Option<String>,
        #[arg(long)]
        expiry_buffer_secs: Option<u64>,
    },
    /// Manage per-consumer router keys (named, revocable, audit-logged).
    RouterKey {
        #[command(subcommand)]
        action: RouterKeyAction,
    },
}

#[derive(Subcommand)]
enum RouterKeyAction {
    /// Generate a new key with the given name and append it to router.toml.
    Add { name: String },
    /// List configured keys (names + disabled flags only — never the secrets).
    List,
    /// Disable a key by name (kept in the file for audit; reversible by editing).
    Revoke { name: String },
    /// Remove a key entirely from router.toml.
    Rm { name: String },
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

        Cmd::Router {
            listen,
            broker_url,
            consume_key,
            upstream,
        } => {
            router::run(resolve_router(
                listen,
                broker_url,
                consume_key,
                upstream,
            )?)
            .await
        }

        Cmd::RouterConfigure {
            listen,
            broker_url,
            consume_key,
            upstream,
            expiry_buffer_secs,
        } => {
            let existing = load_router().ok();
            let cfg = RouterConfig {
                listen: listen
                    .or_else(|| existing.as_ref().map(|c| c.listen.clone()))
                    .unwrap_or_else(|| "127.0.0.1:8788".into()),
                broker_url: broker_url
                    .or_else(|| existing.as_ref().map(|c| c.broker_url.clone()))
                    .context("--broker required (or set previously)")?,
                consume_key: consume_key
                    .or_else(|| existing.as_ref().map(|c| c.consume_key.clone()))
                    .context("--consume-key required (or set previously)")?,
                keys: existing.as_ref().map(|c| c.keys.clone()).unwrap_or_default(),
                upstream: upstream
                    .or_else(|| existing.as_ref().map(|c| c.upstream.clone()))
                    .unwrap_or_else(|| "https://api.anthropic.com".into()),
                expiry_buffer_secs: expiry_buffer_secs
                    .or_else(|| existing.as_ref().map(|c| c.expiry_buffer_secs))
                    .unwrap_or(300),
            };
            save_router(&cfg)?;
            println!("saved {}", router_path().display());
            Ok(())
        }

        Cmd::RouterKey { action } => router_key_action(action),
    }
}

fn router_key_action(action: RouterKeyAction) -> Result<()> {
    let mut cfg = load_router().context(
        "router.toml not found — run `claude-courier router-configure` first",
    )?;
    match action {
        RouterKeyAction::Add { name } => {
            if cfg.keys.iter().any(|k| k.name == name) {
                anyhow::bail!("key `{}` already exists", name);
            }
            let key = generate_key()?;
            cfg.keys.push(RouterKey {
                name: name.clone(),
                key: key.clone(),
                disabled: false,
            });
            save_router(&cfg)?;
            println!("added key `{name}` to {}", router_path().display());
            println!();
            println!("set on the consumer machine (shown once — copy now):");
            println!("  export ANTHROPIC_BASE_URL=https://router.aishipbox.com");
            println!("  export ANTHROPIC_API_KEY={key}");
        }
        RouterKeyAction::List => {
            if cfg.keys.is_empty() {
                println!("(no keys configured — `router-key add <name>` to provision one)");
            } else {
                println!("{:<24}  status", "name");
                for k in &cfg.keys {
                    let status = if k.disabled { "disabled" } else { "enabled" };
                    println!("{:<24}  {status}", k.name);
                }
            }
        }
        RouterKeyAction::Revoke { name } => {
            let entry = cfg
                .keys
                .iter_mut()
                .find(|k| k.name == name)
                .with_context(|| format!("no key named `{name}`"))?;
            if entry.disabled {
                println!("key `{name}` was already disabled");
            } else {
                entry.disabled = true;
                save_router(&cfg)?;
                println!("revoked key `{name}` (disabled flag set)");
            }
        }
        RouterKeyAction::Rm { name } => {
            let before = cfg.keys.len();
            cfg.keys.retain(|k| k.name != name);
            if cfg.keys.len() == before {
                anyhow::bail!("no key named `{name}`");
            }
            save_router(&cfg)?;
            println!("removed key `{name}`");
        }
    }
    Ok(())
}

fn generate_key() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).context("getrandom failed")?;
    Ok(hex::encode(bytes))
}
