use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Result;
use bitflags::bitflags;
use hashbrown::HashMap;
use parking_lot::{Mutex, RwLock};
use smallvec::{smallvec, SmallVec};

use crate::cachestr::Cachestr;
use crate::pipeline::misc;
use crate::proto::UpstreamKind;
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
            upstream: RwLock::new(None),
            reqctx: Default::default(),
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

pub(crate) enum StringX {
    Cache(Cachestr),
    String(String),
    Cow(Cow<'static, str>),
    Arc(Arc<String>),
}

impl AsRef<str> for StringX {
    fn as_ref(&self) -> &str {
        match self {
            StringX::Cache(c) => c.as_ref(),
            StringX::String(s) => s.as_ref(),
            StringX::Cow(c) => c.as_ref(),
            StringX::Arc(a) => a.as_ref(),
        }
    }
}

pub(crate) enum HeaderOperator {
    Drop,
    Add(StringX),
}

#[derive(Default)]
pub struct HeadersContext {
    pub(crate) inner: Mutex<HashMap<Cachestr, SmallVec<[HeaderOperator; 8]>>>,
}

impl HeadersContext {
    pub fn drop<A>(&self, header: A)
    where
        A: AsRef<str>,
    {
        let k = Cachestr::from(header.as_ref());
        let v = smallvec![HeaderOperator::Drop];
        let mut w = self.inner.lock();
        w.insert(k, v);
    }

    pub fn replace<K, V>(&self, header: K, value: V)
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let k = Cachestr::from(header.as_ref());
        let v = smallvec![
            HeaderOperator::Drop,
            HeaderOperator::Add(StringX::Cache(Cachestr::from(value.as_ref())))
        ];

        let mut w = self.inner.lock();
        w.insert(k, v);
    }
}

#[derive(Default)]
pub struct RequestContext {
    pub(crate) headers: HeadersContext,
}

impl RequestContext {
    pub fn headers(&self) -> &HeadersContext {
        &self.headers
    }
}

pub struct HttpContext {
    id: u64,
    flags: HttpContextFlags,
    client_addr: SocketAddr,
    upstream: RwLock<Option<Arc<UpstreamKind>>>,
    pipelines: (AtomicUsize, Vec<Box<dyn HttpPipeline>>),
    pub(crate) reqctx: RequestContext,
}

impl HttpContext {
    pub(crate) fn builder(client_addr: SocketAddr) -> HttpContextBuilder {
        HttpContextBuilder {
            client_addr,
            flags: Default::default(),
            pipelines: vec![],
        }
    }

    pub(crate) fn request(&self) -> &RequestContext {
        &self.reqctx
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

    pub(crate) fn upstream(&self) -> Option<Arc<UpstreamKind>> {
        let r = self.upstream.read();
        Clone::clone(&r)
    }

    pub fn set_upstream(&self, upstream: UpstreamKind) {
        let mut w = self.upstream.write();
        w.replace(Arc::new(upstream));
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
