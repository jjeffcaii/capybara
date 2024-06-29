use anyhow::Result;

use crate::error::CapybaraError;
use crate::pipeline::{HttpPipelineFactory, PipelineConf};
use crate::protocol::http::RequestLine;

use super::{HttpContext, HttpPipeline};

pub(crate) struct NoopHttpPipeline {
    id: u64,
}

#[async_trait::async_trait]
impl HttpPipeline for NoopHttpPipeline {
    async fn handle_request_line(
        &self,
        ctx: &mut HttpContext,
        request_line: &mut RequestLine,
    ) -> Result<()> {
        if log_enabled!(log::Level::Debug) {
            let path = request_line.path();
            debug!("#{}: path={}", self.id, &path);
        }

        match ctx.next() {
            None => Ok(()),
            Some(pipeline) => pipeline.handle_request_line(ctx, request_line).await,
        }
    }
}

impl From<u64> for NoopHttpPipeline {
    fn from(value: u64) -> Self {
        Self { id: value }
    }
}

pub(crate) struct NoopHttpPipelineFactory {
    id: u64,
}

impl HttpPipelineFactory for NoopHttpPipelineFactory {
    type Item = NoopHttpPipeline;

    fn generate(&self) -> Result<Self::Item> {
        Ok(NoopHttpPipeline::from(self.id))
    }
}

impl TryFrom<&PipelineConf> for NoopHttpPipelineFactory {
    type Error = anyhow::Error;

    fn try_from(value: &PipelineConf) -> std::result::Result<Self, Self::Error> {
        const KEY_ID: &str = "id";

        if let Some(v) = value.get(KEY_ID) {
            if let Some(id) = v.as_u64() {
                return Ok(Self { id });
            }
        }

        bail!(CapybaraError::InvalidConfig(KEY_ID.into()))
    }
}
