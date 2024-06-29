#[macro_use]
extern crate log;

use capybara_core::pipeline::PipelineConf;
use capybara_core::proto::{Listener, Signal};
use capybara_core::protocol::http::HttpListener;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

async fn init() {
    pretty_env_logger::try_init_timed().ok();
    capybara_core::setup().await;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init().await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let c: PipelineConf = {
        // language=yaml
        let yaml = r#"
         routes:
         - route: http://httpbin.org
        "#;
        serde_yaml::from_str(yaml).unwrap()
    };

    // Test request when server is started:
    //   1. proxypass httpbin.org: curl -i http://127.0.0.1:15006/anything
    let l = HttpListener::builder("127.0.0.1:15006".parse()?)
        .id("httpbin-example")
        .pipeline("capybara.pipelines.http.router", &c)
        .build()?;

    tokio::spawn(async move {
        if let Err(e) = l.listen(&mut rx).await {
            error!("httpbin server is stopped: {}", e);
        }
    });

    tokio::signal::ctrl_c().await?;

    tx.send(Signal::Shutdown).await?;

    Ok(())
}
