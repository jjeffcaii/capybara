#[macro_use]
extern crate log;

use capybara_core::pipeline::PipelineConf;
use capybara_core::proto::{Listener, Signal};
use capybara_core::protocol::http::HttpListener;
use capybara_core::transport::TlsAcceptorBuilder;

#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

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
        - route: httpbin.org
          matches:
          - location: host
            match: httpbin.org
        fallback: www.envoyproxy.io:443
        "#;
        serde_yaml::from_str(yaml).unwrap()
    };

    // You can generate pem files following the command below:
    //   mkcert -key-file localhost.key.pem -cert-file localhost.cert.pem localhost
    let tls_acceptor = TlsAcceptorBuilder::default()
        .cert(include_str!("localhost.cert.pem"))
        .key(include_str!("localhost.key.pem"))
        .build()?;

    // Test request when server is started:
    //   1. proxypass httpbin.org: curl -i -H 'Host: httpbin.org' https://localhost:8443/anything
    //   2. proxypass www.envoyproxy.io, just open link 'https://localhost:8443/' in your web browser
    let l = HttpListener::builder("127.0.0.1:8443".parse()?)
        .id("httpbin")
        .tls(tls_acceptor)
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
