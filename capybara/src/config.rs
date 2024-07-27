use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct Config {
    pub listeners: HashMap<String, ListenerConfig>,
    pub upstreams: HashMap<String, UpstreamConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ListenerConfig {
    pub listen: String,
    pub protocol: ProtocolConfig,
    #[serde(default)]
    pub pipelines: Vec<PipelineConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProtocolConfig {
    pub name: String,
    #[serde(default)]
    pub props: capybara_core::pipeline::PipelineConf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct PipelineConfig {
    pub name: String,
    #[serde(default)]
    pub props: capybara_core::pipeline::PipelineConf,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub enum TransportKind {
    #[serde(rename = "tcp")]
    Tcp,
    #[serde(rename = "udp")]
    Udp,
}

impl Default for TransportKind {
    fn default() -> Self {
        Self::Tcp
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub enum BalanceStrategy {
    #[serde(rename = "weighted")]
    Weighted,
    #[serde(rename = "ip-hash")]
    IpHash,
    #[serde(rename = "round-robin")]
    RoundRobin,
}

impl Default for BalanceStrategy {
    fn default() -> Self {
        Self::Weighted
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct UpstreamConfig {
    #[serde(default)]
    pub transport: TransportKind,
    pub resolver: Option<String>,
    pub balancer: BalanceStrategy,
    pub pool: Option<PoolConfig>,
    pub endpoints: Vec<EndpointConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct EndpointConfig {
    pub resolver: Option<String>,
    pub addr: String,
    pub weight: Option<u32>,
    pub pool: Option<PoolConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct PoolConfig {
    pub idle: u32,
    pub max: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_read_config_from_yaml() {
        init();

        let b = std::fs::read("../testdata/config.yaml").unwrap();
        let c = serde_yaml::from_slice::<'_, Config>(&b[..]);
        assert!(c.is_ok_and(|c| {
            info!("read: {:?}", &c);
            true
        }));
    }
}
