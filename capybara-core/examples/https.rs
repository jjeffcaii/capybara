#[macro_use]
extern crate log;

use capybara_core::pipeline::PipelineConf;
use capybara_core::proto::{Listener, Signal};
use capybara_core::protocol::http::HttpListener;
use capybara_core::transport::TlsAcceptorBuilder;

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
        fallback: https://letsencrypt.org
        "#;
        serde_yaml::from_str(yaml).unwrap()
    };

    // You can generate pem files following the command below:
    //   $ mkcert -key-file localhost.key.pem -cert-file localhost.cert.pem localhost
    let tls_acceptor = TlsAcceptorBuilder::default()
        .cert(include_str!("localhost.cert.pem"))
        .key(include_str!("localhost.key.pem"))
        .build()?;

    // Test request when server is started, open link 'https://localhost:8443/' in your web browser
    let l = HttpListener::builder("127.0.0.1:8443".parse()?)
        .id("https-example")
        .tls(tls_acceptor)
        .pipeline("capybara.pipelines.http.router", &c)
        .build()?;

    tokio::spawn(async move {
        if let Err(e) = l.listen(&mut rx).await {
            error!("https server is stopped: {}", e);
        }
    });

    tokio::signal::ctrl_c().await?;

    tx.send(Signal::Shutdown).await?;

    Ok(())
}
