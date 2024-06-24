use anyhow::Result;
use slog::Value;
use tokio::sync::OnceCell;

static ONCE: OnceCell<()> = OnceCell::const_new();

pub async fn setup() {
    ONCE.get_or_init(|| async {
        register_stream_pipeline().await;
        register_http_pipeline().await;
    })
    .await;
}

#[inline(always)]
async fn register_http_pipeline() {
    use crate::pipeline::http::register;

    {
        use crate::pipeline::http::NoopHttpPipelineFactory as Factory;
        let name = "capybara.pipelines.http.noop";
        match register(name, |c| Factory::try_from(c)).await {
            Ok(()) => info!("register '{}' ok", name),
            Err(e) => error!("register '{}' occurs an error: {}", name, e),
        }
    }

    {
        use crate::pipeline::http::HttpPipelineRouterFactory as Factory;
        let name = "capybara.pipelines.http.router";
        match register(name, |c| Factory::try_from(c)).await {
            Ok(()) => info!("register '{}' ok", name),
            Err(e) => error!("register '{}' occurs an error: {}", name, e),
        }
    }
}

#[inline(always)]
async fn register_stream_pipeline() {
    use crate::pipeline::stream::register;

    {
        use crate::pipeline::stream::RouteStreamPipelineFactory as Factory;
        let name = "capybara.pipelines.stream.router";
        match register(name, |c| Factory::try_from(c)).await {
            Ok(()) => info!("register '{}' ok", name),
            Err(e) => error!("register '{}' occurs an error: {}", name, e),
        }
    }
}
