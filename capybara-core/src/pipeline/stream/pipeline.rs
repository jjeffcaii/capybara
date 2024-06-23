use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use parking_lot::RwLock;

use crate::cachestr::Cachestr;
use crate::pipeline::misc;

pub(crate) struct StreamContextBuilder {
    client_addr: SocketAddr,
    pipelines: Vec<Box<dyn StreamPipeline>>,
}

impl StreamContextBuilder {
    pub(crate) fn pipeline<P>(self, pipeline: P) -> Self
    where
        P: StreamPipeline,
    {
        self.pipeline_boxed(Box::new(pipeline))
    }

    pub(crate) fn pipeline_boxed(mut self, pipeline: Box<dyn StreamPipeline>) -> Self {
        self.pipelines.push(pipeline);
        self
    }

    pub(crate) fn build(self) -> StreamContext {
        let Self {
            client_addr,
            pipelines,
        } = self;
        StreamContext {
            id: misc::sequence(),
            client_addr,
            upstream: RwLock::new(None),
            pipelines: (AtomicUsize::new(1), pipelines),
        }
    }
}

pub struct StreamContext {
    id: u64,
    client_addr: SocketAddr,
    upstream: RwLock<Option<Cachestr>>,
    pipelines: (AtomicUsize, Vec<Box<dyn StreamPipeline>>),
}

impl StreamContext {
    pub(crate) fn builder(client_addr: SocketAddr) -> StreamContextBuilder {
        StreamContextBuilder {
            client_addr,
            pipelines: vec![],
        }
    }

    pub fn client_addr(&self) -> SocketAddr {
        self.client_addr
    }

    pub(crate) fn upstream(&self) -> Option<Cachestr> {
        let r = self.upstream.read();
        Clone::clone(&r)
    }

    pub fn set_upstream<A>(&self, upstream: A)
    where
        A: AsRef<str>,
    {
        let mut w = self.upstream.write();
        w.replace(Cachestr::from(upstream.as_ref()));
    }

    pub(crate) fn reset_pipeline(&self) -> Option<&dyn StreamPipeline> {
        if let Some(first) = self.pipelines.1.first() {
            self.pipelines.0.store(1, Ordering::SeqCst);
            return Some(first.as_ref());
        }
        None
    }

    pub fn next(&self) -> Option<&dyn StreamPipeline> {
        let idx = self.pipelines.0.fetch_add(1, Ordering::SeqCst);
        match self.pipelines.1.get(idx) {
            None => None,
            Some(next) => Some(next.as_ref()),
        }
    }
}

#[async_trait::async_trait]
pub trait StreamPipeline: Send + Sync + 'static {
    async fn handle_connect(&self, ctx: &StreamContext) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_connect(ctx).await,
        }
    }
}
