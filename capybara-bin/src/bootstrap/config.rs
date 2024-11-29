use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConf {
    #[serde(default)]
    pub resolvers: HashMap<String, ResolverConf>,
    #[serde(default)]
    pub loggers: HashMap<String, LoggerConf>,
    #[serde(default)]
    pub providers: Vec<ProviderConf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolverConf {
    pub kind: String,
    pub props: HashMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggerConf {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConf {
    pub kind: String,
    pub props: HashMap<String, serde_yaml::Value>,
}
