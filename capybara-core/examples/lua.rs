#[macro_use]
extern crate log;

use capybara_core::pipeline::PipelineConf;
use capybara_core::proto::{Listener, Signal};
use capybara_core::protocol::http::HttpListener;

async fn init() {
    pretty_env_logger::try_init_timed().ok();
    capybara_core::setup().await;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init().await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let c: PipelineConf = {
        // Example: a dynamic router from lua
        let yaml = r#"
        content: |
          function handle_request_headers(ctx,headers)
            local u = headers:get('x-upstream-key')
            if u ~= nil and u ~= '' then
              ctx:set_upstream(u)
            else
              ctx:respond({
                status=502,
                headers={
                  ['Content-Type']='application/json; charset=UTF-8'
                },
                body=json:encode({err='NO_ROUTE_FOUND',msg='Please set x-upstream-key!'})
              })
            end
          end

          function handle_status_line(ctx,status_line)
            ctx:replace_header('X-Powered-By','capybara')
          end
        "#;
        serde_yaml::from_str(yaml).unwrap()
    };

    let l = HttpListener::builder("127.0.0.1:15006".parse()?)
        .id("lua-example")
        .pipeline("capybara.pipelines.http.lua", &c)
        .build()?;

    tokio::spawn(async move {
        if let Err(e) = l.listen(&mut rx).await {
            error!("lua server is stopped: {}", e);
        }
    });

    tokio::signal::ctrl_c().await?;
    info!("ctrl_c received");

    tx.send(Signal::Shutdown).await?;

    Ok(())
}
