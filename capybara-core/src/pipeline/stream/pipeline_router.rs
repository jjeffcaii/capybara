use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::pipeline::stream::{StreamContext, StreamPipeline, StreamPipelineFactory};
use crate::pipeline::PipelineConf;
use crate::proto::UpstreamKey;
use crate::CapybaraError;

pub(crate) struct RouteStreamPipeline {
    upstream: Arc<UpstreamKey>,
}

#[async_trait]
impl StreamPipeline for RouteStreamPipeline {
    async fn handle_connect(&self, ctx: &mut StreamContext) -> Result<()> {
        ctx.set_upstream(Clone::clone(&self.upstream));
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_connect(ctx).await,
        }
    }
}

pub(crate) struct RouteStreamPipelineFactory {
    upstream: Arc<UpstreamKey>,
}

impl StreamPipelineFactory for RouteStreamPipelineFactory {
    type Item = RouteStreamPipeline;

    fn generate(&self) -> Result<Self::Item> {
        Ok(RouteStreamPipeline {
            upstream: Clone::clone(&self.upstream),
        })
    }
}

impl TryFrom<&PipelineConf> for RouteStreamPipelineFactory {
    type Error = anyhow::Error;

    fn try_from(value: &PipelineConf) -> std::result::Result<Self, Self::Error> {
        const KEY_UPSTREAM: &str = "upstream";

        if let Some(val) = value.get(KEY_UPSTREAM) {
            if let Some(upstream) = val.as_str() {
                let upstream = upstream.parse::<UpstreamKey>()?;

                return Ok(Self {
                    upstream: Arc::new(upstream),
                });
            }
        }
        bail!(CapybaraError::InvalidConfig(KEY_UPSTREAM.into()))
    }
}
