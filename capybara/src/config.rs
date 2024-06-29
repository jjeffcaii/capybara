use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct Config {
    pub listeners: HashMap<String, ListenerConfig>,
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
