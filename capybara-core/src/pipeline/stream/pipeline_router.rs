use crate::Error;
use anyhow::Result;
use async_trait::async_trait;

use crate::cachestr::Cachestr;
use crate::pipeline::stream::{StreamContext, StreamPipeline, StreamPipelineFactory};
use crate::pipeline::PipelineConf;

pub(crate) struct RouteStreamPipeline {
    upstream: Cachestr,
}

#[async_trait]
impl StreamPipeline for RouteStreamPipeline {
    async fn handle_connect(&self, ctx: &StreamContext) -> Result<()> {
        ctx.set_upstream(self.upstream.as_ref());
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_connect(ctx).await,
        }
    }
}

pub(crate) struct RouteStreamPipelineFactory {
    upstream: Cachestr,
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
        if let Some(val) = value.get("upstream") {
            if let Some(upstream) = val.as_str() {
                return Ok(Self {
                    upstream: Cachestr::from(upstream),
                });
            }
        }

        bail!(Error::InvalidConfig("upstream".into()))
    }
}
