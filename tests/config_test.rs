use claude_courier::config::{BrokerConfig, ConsumerConfig, PublisherConfig};

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
