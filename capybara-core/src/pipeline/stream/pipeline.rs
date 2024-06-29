use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use smallvec::{smallvec, SmallVec};

use crate::pipeline::misc;
use crate::proto::UpstreamKey;

type Pipelines = SmallVec<[Arc<dyn StreamPipeline>; 8]>;

pub(crate) struct StreamContextBuilder {
    client_addr: SocketAddr,
    pipelines: Pipelines,
}

impl StreamContextBuilder {
    pub(crate) fn pipeline<P>(self, pipeline: P) -> Self
    where
        P: StreamPipeline,
    {
        self.pipeline_arc(Arc::new(pipeline))
    }

    pub(crate) fn pipeline_arc(mut self, pipeline: Arc<dyn StreamPipeline>) -> Self {
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
            upstream: None,
            pipelines: (0, pipelines),
        }
    }
}

pub struct StreamContext {
    id: u64,
    client_addr: SocketAddr,
    upstream: Option<Arc<UpstreamKey>>,
    pipelines: (usize, Pipelines),
}

impl StreamContext {
    pub(crate) fn builder(client_addr: SocketAddr) -> StreamContextBuilder {
        StreamContextBuilder {
            client_addr,
            pipelines: smallvec![],
        }
    }

    pub fn client_addr(&self) -> SocketAddr {
        self.client_addr
    }

    pub(crate) fn upstream(&self) -> Option<Arc<UpstreamKey>> {
        Clone::clone(&self.upstream)
    }

    pub fn set_upstream(&mut self, upstream: Arc<UpstreamKey>) {
        self.upstream.replace(upstream);
    }

    pub(crate) fn pipeline(&mut self) -> Option<Arc<dyn StreamPipeline>> {
        if let Some(first) = self.pipelines.1.first() {
            self.pipelines.0 = 1;
            return Some(Clone::clone(first));
        }
        None
    }

    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Arc<dyn StreamPipeline>> {
        match self.pipelines.1.get(self.pipelines.0) {
            None => None,
            Some(next) => {
                self.pipelines.0 += 1;
                Some(Clone::clone(next))
            }
        }
    }
}

#[async_trait::async_trait]
pub trait StreamPipeline: Send + Sync + 'static {
    async fn handle_connect(&self, ctx: &mut StreamContext) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_connect(ctx).await,
        }
    }
}
