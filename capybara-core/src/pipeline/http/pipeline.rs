use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use bitflags::bitflags;
use hashbrown::HashMap;
use smallvec::{smallvec, SmallVec};

use crate::cachestr::Cachestr;
use crate::pipeline::misc;
use crate::proto::UpstreamKey;
use crate::protocol::http::{Headers, RequestLine, StatusLine};

type Pipelines = SmallVec<[Arc<dyn HttpPipeline>; 8]>;

pub(crate) struct HttpContextBuilder {
    client_addr: SocketAddr,
    flags: HttpContextFlags,
    pipelines: Pipelines,
}

impl HttpContextBuilder {
    #[inline]
    pub(crate) fn pipeline<P>(self, pipeline: P) -> Self
    where
        P: HttpPipeline,
    {
        self.pipeline_arc(Arc::new(pipeline))
    }

    #[inline]
    pub(crate) fn pipeline_arc(mut self, pipeline: Arc<dyn HttpPipeline>) -> Self {
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
            pipelines: (0, pipelines),
            upstream: None,
            reqctx: Default::default(),
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HttpContextFlags(u32);

bitflags! {
    impl HttpContextFlags: u32 {
        const DOWNSTREAM_EXHAUSTED = 1 << 0;
    }
}

pub(crate) enum AnyString {
    Cache(Cachestr),
    String(String),
    Cow(Cow<'static, str>),
    Arc(Arc<String>),
}

impl AsRef<str> for AnyString {
    fn as_ref(&self) -> &str {
        match self {
            AnyString::Cache(c) => c.as_ref(),
            AnyString::String(s) => s.as_ref(),
            AnyString::Cow(c) => c.as_ref(),
            AnyString::Arc(a) => a.as_ref(),
        }
    }
}

pub(crate) enum HeaderOperator {
    Drop,
    Add(AnyString),
}

#[derive(Default)]
pub struct HeadersContext {
    pub(crate) inner: HashMap<Cachestr, SmallVec<[HeaderOperator; 8]>>,
}

impl HeadersContext {
    pub fn reset(&mut self) {
        self.inner.clear();
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn drop<A>(&mut self, header: A)
    where
        A: AsRef<str>,
    {
        let k = Cachestr::from(header.as_ref());
        let v = smallvec![HeaderOperator::Drop];
        self.inner.insert(k, v);
    }

    pub fn replace<K, V>(&mut self, header: K, value: V)
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let k = Cachestr::from(header.as_ref());
        let v = smallvec![
            HeaderOperator::Drop,
            HeaderOperator::Add(AnyString::Cache(Cachestr::from(value.as_ref())))
        ];

        self.inner.insert(k, v);
    }
}

#[derive(Default)]
pub struct RequestContext {
    pub(crate) headers: HeadersContext,
}

impl RequestContext {
    pub fn headers(&mut self) -> &mut HeadersContext {
        &mut self.headers
    }

    pub fn reset(&mut self) {
        self.headers.reset();
    }
}

pub struct HttpContext {
    pub(crate) id: u64,
    pub(crate) flags: HttpContextFlags,
    pub(crate) client_addr: SocketAddr,
    pub(crate) upstream: Option<Arc<UpstreamKey>>,
    pub(crate) pipelines: (usize, SmallVec<[Arc<dyn HttpPipeline>; 8]>),
    pub(crate) reqctx: RequestContext,
}

impl HttpContext {
    pub(crate) fn builder(client_addr: SocketAddr) -> HttpContextBuilder {
        HttpContextBuilder {
            client_addr,
            flags: Default::default(),
            pipelines: smallvec![],
        }
    }

    #[inline]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub fn client_addr(&self) -> SocketAddr {
        self.client_addr
    }

    #[inline]
    pub fn request(&mut self) -> &mut RequestContext {
        &mut self.reqctx
    }

    #[inline]
    pub(crate) fn flags(&self) -> HttpContextFlags {
        self.flags
    }

    #[inline]
    pub(crate) fn flags_mut(&mut self) -> &mut HttpContextFlags {
        &mut self.flags
    }

    #[inline]
    pub(crate) fn pipeline(&mut self) -> Option<Arc<dyn HttpPipeline>> {
        if let Some(root) = self.pipelines.1.first() {
            self.pipelines.0 = 1;
            return Some(Clone::clone(root));
        }
        None
    }

    #[inline]
    pub(crate) fn upstream(&self) -> Option<Arc<UpstreamKey>> {
        self.upstream.clone()
    }

    #[inline]
    pub fn set_upstream(&mut self, upstream: UpstreamKey) {
        self.upstream.replace(upstream.into());
    }

    #[inline]
    pub(crate) fn reset(&mut self) {
        self.reqctx.reset();
        self.pipelines.0 = 0;
        self.upstream.take();
        self.flags = HttpContextFlags::default();
    }

    /// Returns the next http pipeline.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Arc<dyn HttpPipeline>> {
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
pub trait HttpPipeline: Send + Sync + 'static {
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    async fn handle_request_line(
        &self,
        ctx: &mut HttpContext,
        request_line: &mut RequestLine,
    ) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_request_line(ctx, request_line).await,
        }
    }

    async fn handle_request_headers(
        &self,
        ctx: &mut HttpContext,
        headers: &mut Headers,
    ) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_request_headers(ctx, headers).await,
        }
    }

    async fn handle_status_line(
        &self,
        ctx: &mut HttpContext,
        status_line: &mut StatusLine,
    ) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_status_line(ctx, status_line).await,
        }
    }

    async fn handle_response_headers(
        &self,
        ctx: &mut HttpContext,
        headers: &mut Headers,
    ) -> Result<()> {
        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_response_headers(ctx, headers).await,
        }
    }
}
