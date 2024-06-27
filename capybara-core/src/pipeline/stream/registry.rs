use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hashbrown::HashMap;
use once_cell::sync::Lazy;
use parking_lot::RwLock;

use crate::pipeline::PipelineConf;

use super::pipeline::StreamPipeline;

static REGISTRY: Lazy<
    RwLock<
        HashMap<
            String,
            Arc<dyn Send + Sync + Fn(&PipelineConf) -> Result<Box<dyn StreamPipelineFactoryExt>>>,
        >,
    >,
> = Lazy::new(Default::default);

/// StreamPipelineFactory is aim at generating specified StreamPipeline.
/// It's designed for user-defined, which means you can customize your own StreamPipeline as an entrance.
#[async_trait]
pub trait StreamPipelineFactory: Send + Sync + 'static {
    type Item;

    /// setup will be called which a StreamPipelineFactory being registered, and notice it's a oneshot behavior.
    async fn setup() -> Result<()>
    where
        Self: Sized,
    {
        Ok(())
    }

    /// generate will generate a new StreamPipeline.
    fn generate(&self) -> Result<Self::Item>;
}

pub(crate) trait StreamPipelineFactoryExt: Send + Sync + 'static {
    fn generate_boxed(&self) -> Result<Box<dyn StreamPipeline>>;
    fn generate_arc(&self) -> Result<Arc<dyn StreamPipeline>>;
}

impl<F, T> StreamPipelineFactoryExt for F
where
    T: StreamPipeline,
    F: StreamPipelineFactory<Item = T>,
{
    fn generate_boxed(&self) -> Result<Box<dyn StreamPipeline>> {
        let f = self.generate()?;
        Ok(Box::new(f))
    }

    fn generate_arc(&self) -> Result<Arc<dyn StreamPipeline>> {
        let f = self.generate()?;
        Ok(Arc::new(f))
    }
}

pub async fn register<I, T, F, G>(name: I, g: G) -> Result<()>
where
    I: Into<String>,
    T: StreamPipelineFactory<Item = F>,
    F: StreamPipeline,
    G: 'static + Sync + Send + Fn(&PipelineConf) -> Result<T>,
{
    let name = name.into();

    // 1. setup stream filter
    {
        let res = T::setup().await;
        if let Err(e) = &res {
            error!("cannot setup stream filter '{}': {:?}", &name, e);
        }
        res?;
    }

    // 2. wrap into generator function
    let wrapper = move |props: &PipelineConf| -> Result<Box<dyn StreamPipelineFactoryExt>> {
        let f = g(props)?;
        // convert to boxed trait
        let f: Box<dyn StreamPipelineFactoryExt> = Box::new(f);
        Ok(f)
    };

    // 3. register
    let mut lock = REGISTRY.write();
    lock.insert(name, Arc::new(wrapper));

    Ok(())
}

pub(crate) fn load(name: &str, props: &PipelineConf) -> Result<Box<dyn StreamPipelineFactoryExt>> {
    let g = {
        let lock = REGISTRY.read();
        lock.get(name).cloned()
    };

    match g {
        Some(g) => g(props),
        None => bail!("no such stream filter '{}'", name),
    }
}
