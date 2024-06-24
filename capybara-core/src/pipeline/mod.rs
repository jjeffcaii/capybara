use std::collections::HashMap;

pub use http::{
    register as register_http_pipeline, HttpContext, HttpPipeline, HttpPipelineFactory,
};
pub use stream::{
    register as register_stream_pipeline, StreamContext, StreamPipeline, StreamPipelineFactory,
};

// TODO: implement pipeline
pub(crate) mod http;
mod misc;
pub(crate) mod stream;

pub type PipelineConf = HashMap<String, serde_yaml::Value>;
