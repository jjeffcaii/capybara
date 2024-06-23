use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use bitflags::bitflags;

use crate::pipeline::misc;
use crate::protocol::http::{Headers, RequestLine, StatusLine};

pub(crate) struct HttpContextBuilder {
    client_addr: SocketAddr,
    flags: HttpContextFlags,
    pipelines: Vec<Box<dyn HttpPipeline>>,
}

impl HttpContextBuilder {
    #[inline]
    pub(crate) fn pipeline<P>(self, pipeline: P) -> Self
    where
        P: HttpPipeline,
    {
        self.pipeline_box(Box::new(pipeline))
    }

    #[inline]
    pub(crate) fn pipeline_box(mut self, pipeline: Box<dyn HttpPipeline>) -> Self {
        self.pipelines.push(pipeline);
        self
    }

    pub(crate) fn flags(mut self, flags: HttpContextFlags) -> Self {
        self.flags = flags;
        self
    }

    pub(crate) fn build(self) -> HttpContext {
        let Self {
            client_addr,
            pipelines,
            flags,
        } = self;
        HttpContext {
            id: misc::sequence(),
            flags,
            client_addr,
            pipelines: (AtomicUsize::new(1), pipelines),
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HttpContextFlags(u32);

bitflags! {
    impl HttpContextFlags: u32 {
        const _ = 1 << 0;
    }
}

pub struct HttpContext {
    id: u64,
    flags: HttpContextFlags,
    client_addr: SocketAddr,
    pipelines: (AtomicUsize, Vec<Box<dyn HttpPipeline>>),
}

impl HttpContext {
    pub(crate) fn builder(client_addr: SocketAddr) -> HttpContextBuilder {
        HttpContextBuilder {
            client_addr,
            flags: Default::default(),
            pipelines: vec![],
        }
    }

    pub(crate) fn flags(&self) -> HttpContextFlags {
        self.flags
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn client_addr(&self) -> SocketAddr {
        self.client_addr
    }

    pub(crate) fn reset_pipeline(&self) -> Option<&dyn HttpPipeline> {
        if let Some(root) = self.pipelines.1.first() {
            self.pipelines.0.store(1, Ordering::SeqCst);
            return Some(root.as_ref());
        }
        None
    }

    /// Returns the next http pipeline.
    pub fn next(&self) -> Option<&dyn HttpPipeline> {
        let seq = self.pipelines.0.fetch_add(1, Ordering::SeqCst);
        match self.pipelines.1.get(seq) {
            None => None,
            Some(next) => Some(next.as_ref()),
        }
    }
}

#[async_trait::async_trait]
pub trait HttpPipeline: Send + Sync + 'static {
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    async fn handle_request_line(
        &self,
        ctx: &HttpContext,
        request_line: &mut RequestLine,
    ) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_request_line(ctx, request_line).await,
        }
    }

    async fn handle_request_headers(&self, ctx: &HttpContext, headers: &mut Headers) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_request_headers(ctx, headers).await,
        }
    }

    async fn handle_status_line(
        &self,
        ctx: &HttpContext,
        status_line: &mut StatusLine,
    ) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_status_line(ctx, status_line).await,
        }
    }

    async fn handle_response_headers(
        &self,
        ctx: &HttpContext,
        headers: &mut Headers,
    ) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_response_headers(ctx, headers).await,
        }
    }
}
