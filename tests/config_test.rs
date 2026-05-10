use claude_courier::config::{
    BrokerConfig, ConsumerConfig, PublisherConfig, RouterConfig, RouterKey,
};

#[test]
fn publisher_config_roundtrip() {
    let cfg = PublisherConfig {
        broker_url: "https://courier.example.com".into(),
        publish_key: "secret".into(),
    };
    let s = toml::to_string(&cfg).unwrap();
    let back: PublisherConfig = toml::from_str(&s).unwrap();
    assert_eq!(back.broker_url, cfg.broker_url);
    assert_eq!(back.publish_key, cfg.publish_key);
}

#[test]
fn broker_config_roundtrip() {
    let cfg = BrokerConfig {
        bind: "0.0.0.0:3007".into(),
        publish_key: "p".into(),
        consume_key: "c".into(),
    };
    let s = toml::to_string(&cfg).unwrap();
    let back: BrokerConfig = toml::from_str(&s).unwrap();
    assert_eq!(back.bind, cfg.bind);
}

#[test]
fn consumer_config_roundtrip() {
    let cfg = ConsumerConfig {
        broker_url: "https://courier.example.com".into(),
        consume_key: "c".into(),
    };
    let s = toml::to_string(&cfg).unwrap();
    let back: ConsumerConfig = toml::from_str(&s).unwrap();
    assert_eq!(back.broker_url, cfg.broker_url);
}

#[test]
fn router_config_roundtrip() {
    let cfg = RouterConfig {
        listen: "127.0.0.1:8788".into(),
        broker_url: "https://courier.example.com".into(),
        consume_key: "c".into(),
        keys: vec![
            RouterKey {
                name: "alice".into(),
                key: "deadbeef".into(),
                disabled: false,
            },
            RouterKey {
                name: "bob".into(),
                key: "cafef00d".into(),
                disabled: true,
            },
        ],
        upstream: "https://api.anthropic.com".into(),
        expiry_buffer_secs: 300,
    };
    let s = toml::to_string(&cfg).unwrap();
    let back: RouterConfig = toml::from_str(&s).unwrap();
    assert_eq!(back.listen, cfg.listen);
    assert_eq!(back.broker_url, cfg.broker_url);
    assert_eq!(back.keys.len(), 2);
    assert_eq!(back.keys[0].name, "alice");
    assert!(!back.keys[0].disabled);
    assert_eq!(back.keys[1].name, "bob");
    assert!(back.keys[1].disabled);
    assert_eq!(back.expiry_buffer_secs, cfg.expiry_buffer_secs);
}

#[test]
fn router_config_accepts_missing_keys_field() {
    // older configs (or freshly-configured routers with no keys yet) should parse
    let toml_src = r#"
listen = "127.0.0.1:8788"
broker_url = "https://courier.example.com"
consume_key = "c"
upstream = "https://api.anthropic.com"
expiry_buffer_secs = 300
"#;
    let cfg: RouterConfig = toml::from_str(toml_src).unwrap();
    assert!(cfg.keys.is_empty());
}
