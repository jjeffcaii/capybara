pub(crate) use noop::NoopHttpPipelineFactory;
pub(crate) use pipeline::{AnyString, HeaderOperator, HttpContextFlags};
pub use pipeline::{HeadersContext, HttpContext, HttpPipeline};
pub(crate) use pipeline_lua::LuaHttpPipelineFactory;
pub(crate) use pipeline_router::HttpPipelineRouterFactory;
pub(crate) use registry::{load, HttpPipelineFactoryExt};
pub use registry::{register, HttpPipelineFactory};

mod noop;
mod pipeline;
mod pipeline_lua;
mod pipeline_router;
mod registry;

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use crate::protocol::http::RequestLine;

    use super::noop::NoopHttpPipeline;
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_pipelines() -> Result<()> {
        init();

        let mut ctx = HttpContext::builder("127.0.0.1:12345".parse().unwrap())
            .pipeline(NoopHttpPipeline::from(1))
            .pipeline(NoopHttpPipeline::from(2))
            .pipeline(NoopHttpPipeline::from(3))
            .build();

        let mut rl = RequestLine::builder().uri("/ping").build();

        if let Some(first) = ctx.pipeline() {
            let _ = first.handle_request_line(&mut ctx, &mut rl).await;
        }

        Ok(())
    }
}
