use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hashbrown::HashMap;
use once_cell::sync::Lazy;
use parking_lot::RwLock;

use crate::pipeline::PipelineConf;

use super::pipeline::HttpPipeline;

static REGISTRY: Lazy<
    RwLock<
        HashMap<
            String,
            Arc<dyn Send + Sync + Fn(&PipelineConf) -> Result<Box<dyn HttpPipelineFactoryExt>>>,
        >,
    >,
> = Lazy::new(Default::default);

/// HttpPipelineFactory is aim at generating specified HttpPipeline.
/// It's designed for user-defined, which means you can customize your own HttpPipeline as an entrance.
#[async_trait]
pub trait HttpPipelineFactory: Send + Sync + 'static {
    type Item;

    /// setup will be called which a HttpPipelineFactory being registered, and notice it's a oneshot behavior.
    async fn setup() -> Result<()>
    where
        Self: Sized,
    {
        Ok(())
    }

    /// generate will generate a new HttpPipeline.
    fn generate(&self) -> Result<Self::Item>;
}

pub(crate) trait HttpPipelineFactoryExt: Send + Sync + 'static {
    fn generate_boxed(&self) -> Result<Box<dyn HttpPipeline>>;
}

impl<F, T> HttpPipelineFactoryExt for F
where
    T: HttpPipeline,
    F: HttpPipelineFactory<Item = T>,
{
    fn generate_boxed(&self) -> Result<Box<dyn HttpPipeline>> {
        let f = self.generate()?;
        Ok(Box::new(f))
    }
}

pub async fn register<I, T, F, G>(name: I, g: G) -> Result<()>
where
    I: Into<String>,
    T: HttpPipelineFactory<Item = F>,
    F: HttpPipeline,
    G: 'static + Sync + Send + Fn(&PipelineConf) -> Result<T>,
{
    let name = name.into();

    // 1. setup http filter
    {
        let res = T::setup().await;
        if let Err(e) = &res {
            error!("cannot setup http filter '{}': {:?}", &name, e);
        }
        res?;
    }

    // 2. wrap into generator function
    let wrapper = move |props: &PipelineConf| -> Result<Box<dyn HttpPipelineFactoryExt>> {
        let f = g(props)?;
        // convert to boxed trait
        let f: Box<dyn HttpPipelineFactoryExt> = Box::new(f);
        Ok(f)
    };

    // 3. register
    let mut lock = REGISTRY.write();
    lock.insert(name, Arc::new(wrapper));

    Ok(())
}

pub(crate) fn load(name: &str, props: &PipelineConf) -> Result<Box<dyn HttpPipelineFactoryExt>> {
    let g = {
        let lock = REGISTRY.read();
        lock.get(name).cloned()
    };

    match g {
        Some(g) => g(props),
        None => bail!("no such http filter '{}'", name),
    }
}
