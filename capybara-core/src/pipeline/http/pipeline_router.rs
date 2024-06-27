use std::convert::Infallible;
use std::net::SocketAddr;
use std::num::ParseIntError;
use std::sync::Arc;

use arc_swap::Cache;
use rustls::ServerName;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::cachestr::Cachestr;
use crate::error::CapybaraError;
use crate::pipeline::{HttpContext, HttpPipeline, HttpPipelineFactory, PipelineConf};
use crate::proto::UpstreamKey;
use crate::protocol::http::{Headers, HttpField, Queries, RequestLine};

struct Route {
    must: Vec<MatchRule>,
    upstream: UpstreamKey,
}

enum MatchRule {
    Path(glob::Pattern),
    Query(Cachestr, glob::Pattern),
    Header(Cachestr, glob::Pattern),
    Field(HttpField, glob::Pattern),
}

pub(crate) struct HttpPipelineRouter {
    request_line: RwLock<Option<RequestLine>>,
    routes: Arc<Vec<Route>>,
    fallback: Option<UpstreamKey>,
}

impl HttpPipelineRouter {
    #[inline]
    async fn compute_upstream(
        &self,
        request_line: &RequestLine,
        headers: &mut Headers,
    ) -> Option<UpstreamKey> {
        for mat in &*self.routes {
            let mut bingo = true;

            for rule in &mat.must {
                bingo = match &rule {
                    MatchRule::Path(pattern) => pattern.matches(&request_line.path_no_slash()),
                    MatchRule::Query(key, pattern) => match request_line.query_bytes() {
                        None => false,
                        Some(b) => {
                            let queries = Queries::from(b);
                            match queries.get_last(key.as_ref()) {
                                None => false,
                                Some(q) => {
                                    pattern.matches(unsafe { std::str::from_utf8_unchecked(q) })
                                }
                            }
                        }
                    },
                    MatchRule::Header(key, pattern) => match headers.get(key.as_ref()) {
                        None => false,
                        Some(v) => pattern.matches(&v),
                    },
                    MatchRule::Field(key, pattern) => match headers.get_by_field(*key) {
                        None => false,
                        Some(b) => pattern.matches(unsafe { std::str::from_utf8_unchecked(b) }),
                    },
                };

                if !bingo {
                    break;
                }
            }

            if bingo {
                return Some(Clone::clone(&mat.upstream));
            }
        }
        Clone::clone(&self.fallback)
    }
}

#[async_trait::async_trait]
impl HttpPipeline for HttpPipelineRouter {
    async fn handle_request_line(
        &self,
        ctx: &mut HttpContext,
        request_line: &mut RequestLine,
    ) -> anyhow::Result<()> {
        {
            let mut w = self.request_line.write().await;
            w.replace(Clone::clone(request_line));
        }

        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_request_line(ctx, request_line).await,
        }
    }

    async fn handle_request_headers(
        &self,
        ctx: &mut HttpContext,
        headers: &mut Headers,
    ) -> anyhow::Result<()> {
        let request_line = {
            let mut w = self.request_line.write().await;
            w.take().unwrap()
        };

        if let Some(upstream) = self.compute_upstream(&request_line, headers).await {
            ctx.set_upstream(upstream);
        }

        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_request_headers(ctx, headers).await,
        }
    }
}

pub(crate) struct HttpPipelineRouterFactory {
    routes: Arc<Vec<Route>>,
    fallback: Option<UpstreamKey>,
}

impl HttpPipelineFactory for HttpPipelineRouterFactory {
    type Item = HttpPipelineRouter;

    fn generate(&self) -> anyhow::Result<Self::Item> {
        Ok(HttpPipelineRouter {
            request_line: Default::default(),
            routes: Clone::clone(&self.routes),
            fallback: Clone::clone(&self.fallback),
        })
    }
}

impl TryFrom<&PipelineConf> for HttpPipelineRouterFactory {
    type Error = anyhow::Error;

    fn try_from(value: &PipelineConf) -> Result<Self, Self::Error> {
        let (routes, fallback) = {
            let s = serde_yaml::to_string(value)?;

            debug!("serde config: {}", &s);

            let rds: RoutesDefine = serde_yaml::from_str(&s)?;

            let mut routes = vec![];

            for rd in rds.routes {
                let mut must = vec![];
                for md in rd.matches {
                    if let Some(expr) = md.r#match {
                        let expr = expr.trim();
                        if expr.is_empty() || expr == "*" {
                            continue;
                        }

                        let pattern = glob::Pattern::new(expr)?;
                        match md.location {
                            Location::Host => {
                                must.push(MatchRule::Field(HttpField::Host, pattern));
                            }
                            Location::Path => {
                                must.push(MatchRule::Path(pattern));
                            }
                            Location::Query => {
                                if let Some(key) = md.key {
                                    must.push(MatchRule::Query(Cachestr::from(key), pattern));
                                }
                            }
                            Location::Header => {
                                if let Some(key) = md.key {
                                    match key.parse::<HttpField>() {
                                        Ok(f) => must.push(MatchRule::Field(f, pattern)),
                                        Err(_) => must
                                            .push(MatchRule::Header(Cachestr::from(key), pattern)),
                                    }
                                }
                            }
                        }
                    }
                }

                let upstream = rd.route.parse::<UpstreamKey>()?;

                routes.push(Route { upstream, must });
            }

            match rds.fallback {
                None => (routes, None),
                Some(s) => (routes, Some(s.parse::<UpstreamKey>()?)),
            }
        };

        if routes.is_empty() && fallback.is_none() {
            bail!(CapybaraError::InvalidConfig("routes'|'fallback".into()));
        }

        Ok(Self {
            routes: Arc::new(routes),
            fallback,
        })
    }
}

#[derive(Serialize, Deserialize)]
enum Location {
    #[serde(rename = "host")]
    Host,
    #[serde(rename = "path")]
    Path,
    #[serde(rename = "query")]
    Query,
    #[serde(rename = "header")]
    Header,
}

#[derive(Serialize, Deserialize)]
struct MatchDefine<'a> {
    location: Location,
    key: Option<&'a str>,
    r#match: Option<&'a str>,
}

#[derive(Serialize, Deserialize)]
struct RouteDefine<'a> {
    route: &'a str,
    #[serde(borrow, default)]
    matches: Vec<MatchDefine<'a>>,
}

#[derive(Serialize, Deserialize)]
struct RoutesDefine<'a> {
    #[serde(borrow, default)]
    routes: Vec<RouteDefine<'a>>,
    fallback: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use crate::pipeline::{HttpContext, HttpPipeline, HttpPipelineFactory, PipelineConf};
    use crate::protocol::http::{Headers, RequestLine};

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_http_pipeline_router() -> anyhow::Result<()> {
        init();

        let c: PipelineConf = {
            // language=yaml
            let s = r#"
            routes:
            - route: 127.0.0.1:80
              matches:
              - location: host
                match: httpbin.org
            - route: 127.0.0.2:80
              matches:
              - location: header
                key: x-your-service
                match: foo
            - route: 127.0.0.3:80
              matches:
              - location: path
                match: '/anything'
            - route: 127.0.0.4:80
              matches:
              - location: query
                key: srv
                match: '3'
            "#;
            serde_yaml::from_str(s).unwrap()
        };

        let factory = HttpPipelineRouterFactory::try_from(&c)?;

        let p = factory.generate()?;

        let gc = || HttpContext::builder("127.0.0.1:12345".parse().unwrap()).build();

        // route nothing
        {
            let mut ctx = gc();
            let mut rl = RequestLine::builder().uri("/nothing").build();
            let mut headers = Headers::builder().build();
            assert!(p.handle_request_line(&mut ctx, &mut rl).await.is_ok());
            assert!(p
                .handle_request_headers(&mut ctx, &mut headers)
                .await
                .is_ok());
            assert!(ctx.upstream().is_none());
        }

        // route with host
        {
            let mut ctx = gc();
            let mut rl = RequestLine::builder().uri("/anything").build();
            let mut headers = Headers::builder().put("Host", "httpbin.org").build();
            assert!(p.handle_request_line(&mut ctx, &mut rl).await.is_ok());
            assert!(p
                .handle_request_headers(&mut ctx, &mut headers)
                .await
                .is_ok());
            assert_eq!(
                Some("tcp://127.0.0.1:80"),
                ctx.upstream().map(|it| it.to_string()).as_deref()
            );
        }

        // route with header
        {
            let mut ctx = gc();
            let mut rl = RequestLine::builder().uri("/anything").build();
            let mut headers = Headers::builder().put("x-your-service", "foo").build();
            assert!(p.handle_request_line(&mut ctx, &mut rl).await.is_ok());
            assert!(p
                .handle_request_headers(&mut ctx, &mut headers)
                .await
                .is_ok());
            assert_eq!(
                Some("tcp://127.0.0.2:80"),
                ctx.upstream().map(|it| it.to_string()).as_deref()
            );
        }

        // route with path
        {
            let mut ctx = gc();
            let mut rl = RequestLine::builder().uri("/anything").build();
            let mut headers = Headers::builder().build();
            assert!(p.handle_request_line(&mut ctx, &mut rl).await.is_ok());
            assert!(p
                .handle_request_headers(&mut ctx, &mut headers)
                .await
                .is_ok());
            assert_eq!(
                Some("tcp://127.0.0.3:80"),
                ctx.upstream().map(|it| it.to_string()).as_deref()
            );
        }

        // route with query
        {
            let mut ctx = gc();
            let mut rl = RequestLine::builder().uri("/hello?srv=3").build();
            let mut headers = Headers::builder().build();
            assert!(p.handle_request_line(&mut ctx, &mut rl).await.is_ok());
            assert!(p
                .handle_request_headers(&mut ctx, &mut headers)
                .await
                .is_ok());
            assert_eq!(
                Some("tcp://127.0.0.4:80"),
                ctx.upstream().map(|it| it.to_string()).as_deref()
            );
        }

        Ok(())
    }
}
