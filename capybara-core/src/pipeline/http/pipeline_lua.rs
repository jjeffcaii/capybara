use std::sync::Arc;

use async_trait::async_trait;
use mlua::prelude::*;
use mlua::{Function, Lua, UserData, UserDataMethods};
use serde::Deserialize;
use serde_yaml::Value;
use tokio::sync::Mutex;

use crate::pipeline::{HttpContext, HttpPipeline, HttpPipelineFactory, PipelineConf};
use crate::protocol::http::{Headers, Method, RequestLine, StatusLine};
use crate::CapybaraError;

struct Module;

impl UserData for Module {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("urldecode", |lua, this, value: LuaString| {
            let b = urlencoding::decode_binary(value.as_bytes());
            lua.create_string(b)
        });
        methods.add_method("urlencode", |lua, this, value: LuaString| {
            let b = value.as_bytes();
            let encoded = urlencoding::encode_binary(b);
            lua.create_string(encoded.as_bytes())
        });
    }
}

struct LuaHttpRequestContext(*mut HttpContext);

impl UserData for LuaHttpRequestContext {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("client_addr", |_, this, ()| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            Ok(ctx.client_addr().to_string())
        });
        methods.add_method("id", |lua, this, ()| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            Ok(ctx.id())
        });

        methods.add_method("replace_header", |_, this, args: (LuaString, LuaString)| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            let key = args.0.to_string_lossy();
            let val = args.1.to_string_lossy();
            ctx.request().headers().replace(key, val);
            Ok(())
        });
        methods.add_method("remove_header", |_, this, name: LuaString| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            let key = name.to_string_lossy();
            ctx.request().headers().remove(key);
            Ok(())
        });
        methods.add_method("append_header", |_, this, args: (LuaString, LuaString)| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            let key = args.0.to_string_lossy();
            let val = args.1.to_string_lossy();
            ctx.request().headers().append(key, val);
            Ok(())
        });

        methods.add_method("set_method", |_, this, method: LuaString| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            let s = method.to_str()?;
            let m = Method::try_from(s)
                .map_err(|e| LuaError::ExternalError(Arc::new(CapybaraError::Other(e))))?;
            ctx.request().method(m);
            Ok(())
        });
    }
}

struct LuaHttpResponseContext(*mut HttpContext);

impl UserData for LuaHttpResponseContext {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("client_addr", |_, this, ()| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            Ok(ctx.client_addr().to_string())
        });
        methods.add_method("id", |lua, this, ()| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            Ok(ctx.id())
        });

        methods.add_method("replace_header", |_, this, args: (LuaString, LuaString)| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            let key = args.0.to_string_lossy();
            let val = args.1.to_string_lossy();
            ctx.response().headers().replace(key, val);
            Ok(())
        });
        methods.add_method("remove_header", |_, this, name: LuaString| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            let key = name.to_string_lossy();
            ctx.response().headers().remove(key);
            Ok(())
        });
        methods.add_method("append_header", |_, this, args: (LuaString, LuaString)| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            let key = args.0.to_string_lossy();
            let val = args.1.to_string_lossy();
            ctx.response().headers().append(key, val);
            Ok(())
        });
    }
}

struct LuaRequestLine(*mut RequestLine);

impl UserData for LuaRequestLine {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("uri", |lua, this, ()| {
            let request_line = unsafe { this.0.as_mut() }.unwrap();
            let uri = request_line.uri();
            lua.create_string(uri)
        });
        methods.add_method("path", |lua, this, ()| {
            let request_line = unsafe { this.0.as_mut() }.unwrap();
            lua.create_string(request_line.path_bytes())
        });
        methods.add_method("path_no_slash", |lua, this, ()| {
            let request_line = unsafe { this.0.as_mut() }.unwrap();
            let path = request_line.path_no_slash();
            lua.create_string(path.as_bytes())
        });
        methods.add_method("method", |lua, this, ()| {
            let request_line = unsafe { this.0.as_mut() }.unwrap();
            let method = request_line.method();
            lua.create_string(method.as_bytes())
        });
        methods.add_method("query", |lua, this, ()| {
            let request_line = unsafe { this.0.as_mut() }.unwrap();
            match request_line.query_bytes() {
                None => Ok(None),
                Some(b) => lua.create_string(b).map(Some),
            }
        });
        methods.add_method("version", |lua, this, ()| {
            let request_line = unsafe { this.0.as_mut() }.unwrap();
            let version = request_line.version();
            lua.create_string(version)
        });
    }
}

struct LuaStatusLine(*mut StatusLine);

impl UserData for LuaStatusLine {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("status_code", |_, this, ()| {
            let status_line = unsafe { this.0.as_mut() }.unwrap();
            Ok(status_line.status_code())
        });
        methods.add_method("version", |_, this, ()| {
            let status_line = unsafe { this.0.as_mut() }.unwrap();
            Ok(status_line.version())
        });
        methods.add_method("reason_phrase", |_, this, ()| {
            let status_line = unsafe { this.0.as_mut() }.unwrap();
            Ok(status_line.reason_phrase())
        });
    }
}

struct LuaHeaders(*mut Headers);

impl UserData for LuaHeaders {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("has", |_, this, name: LuaString| {
            let headers = unsafe { this.0.as_mut() }.unwrap();
            let name = name.to_str()?;
            Ok(headers.position(name).is_some())
        });

        methods.add_method("get", |lua, this, name: LuaString| {
            let headers = unsafe { this.0.as_mut() }.unwrap();
            match headers.get_bytes(name.to_str()?) {
                None => Ok(None),
                Some(b) => lua.create_string(b).map(Some),
            }
        });
        methods.add_method("size", |_, this, ()| {
            let headers = unsafe { this.0.as_mut() }.unwrap();
            Ok(headers.len())
        });
        methods.add_method("nth", |lua, this, i: isize| {
            if i < 1 {
                return Ok(None);
            }
            let headers = unsafe { this.0.as_mut() }.unwrap();
            let nth = headers.nth(i as usize - 1);
            match nth {
                Some((k, v)) => {
                    let key = unsafe { std::str::from_utf8_unchecked(k) };
                    let val = unsafe { std::str::from_utf8_unchecked(v) };
                    let tbl = lua.create_table()?;
                    tbl.push(lua.create_string(key)?)?;
                    tbl.push(lua.create_string(val)?)?;
                    Ok(Some(tbl))
                }
                None => Ok(None),
            }
        });

        methods.add_method("gets", |lua, this, name: LuaString| {
            let headers = unsafe { this.0.as_mut() }.unwrap();
            let positions =
                headers.positions(unsafe { std::str::from_utf8_unchecked(name.as_ref()) });
            if positions.is_empty() {
                return Ok(None);
            }
            let tbl = lua.create_table()?;
            for pos in positions {
                if let Some((_, v)) = headers.nth(pos) {
                    tbl.push(lua.create_string(v)?)?;
                }
            }
            Ok(Some(tbl))
        });
    }
}

pub(crate) struct LuaHttpPipeline {
    vm: Arc<Mutex<Lua>>,
}

#[async_trait]
impl HttpPipeline for LuaHttpPipeline {
    async fn handle_request_line(
        &self,
        ctx: &mut HttpContext,
        request_line: &mut RequestLine,
    ) -> anyhow::Result<()> {
        {
            let vm = self.vm.lock().await;
            let globals = vm.globals();
            globals.set("capybara", vm.create_userdata(Module)?)?;

            let handler = globals.get::<_, Function>("handle_request_line");
            if let Ok(fun) = handler {
                vm.scope(|scope| {
                    let ctx = scope.create_userdata(LuaHttpRequestContext(ctx))?;
                    let request_line = scope.create_userdata(LuaRequestLine(request_line))?;
                    fun.call::<_, Option<LuaValue>>((ctx, request_line))?;
                    Ok(())
                })?;
            }
        }

        match ctx.next() {
            Some(next) => next.handle_request_line(ctx, request_line).await,
            None => Ok(()),
        }
    }

    async fn handle_request_headers(
        &self,
        ctx: &mut HttpContext,
        headers: &mut Headers,
    ) -> anyhow::Result<()> {
        {
            let vm = self.vm.lock().await;
            let globals = vm.globals();
            globals.set("capybara", vm.create_userdata(Module)?)?;

            let handler = globals.get::<_, Function>("handle_request_headers");
            if let Ok(fun) = handler {
                vm.scope(|scope| {
                    let ctx = scope.create_userdata(LuaHttpRequestContext(ctx))?;
                    let headers = scope.create_userdata(LuaHeaders(headers))?;
                    fun.call::<_, Option<LuaValue>>((ctx, headers))?;
                    Ok(())
                })?;
            }
        }

        match ctx.next() {
            Some(next) => next.handle_request_headers(ctx, headers).await,
            None => Ok(()),
        }
    }

    async fn handle_status_line(
        &self,
        ctx: &mut HttpContext,
        status_line: &mut StatusLine,
    ) -> anyhow::Result<()> {
        {
            let vm = self.vm.lock().await;
            let globals = vm.globals();
            globals.set("capybara", vm.create_userdata(Module)?)?;

            let handler = globals.get::<_, Function>("handle_status_line");
            if let Ok(fun) = handler {
                vm.scope(|scope| {
                    let ctx = scope.create_userdata(LuaHttpResponseContext(ctx))?;
                    let status_line = scope.create_userdata(LuaStatusLine(status_line))?;
                    fun.call::<_, Option<LuaValue>>((ctx, status_line))?;
                    Ok(())
                })?;
            }
        }
        match ctx.next() {
            Some(next) => next.handle_status_line(ctx, status_line).await,
            None => Ok(()),
        }
    }

    async fn handle_response_headers(
        &self,
        ctx: &mut HttpContext,
        headers: &mut Headers,
    ) -> anyhow::Result<()> {
        {
            let vm = self.vm.lock().await;
            let globals = vm.globals();
            let handler = globals.get::<_, Function>("handle_response_headers");
            if let Ok(fun) = handler {
                vm.scope(|scope| {
                    let ctx = scope.create_userdata(LuaHttpResponseContext(ctx))?;
                    let headers = scope.create_userdata(LuaHeaders(headers))?;
                    fun.call::<_, Option<LuaValue>>((ctx, headers))?;
                    Ok(())
                })?;
            }
        }
        match ctx.next() {
            Some(next) => next.handle_response_headers(ctx, headers).await,
            None => Ok(()),
        }
    }
}

#[derive(Deserialize)]
struct LuaHttpPipelineConfig<'a> {
    content: &'a str,
}

pub(crate) struct LuaHttpPipelineFactory {
    vm: Arc<Mutex<Lua>>,
}

impl HttpPipelineFactory for LuaHttpPipelineFactory {
    type Item = LuaHttpPipeline;

    fn generate(&self) -> anyhow::Result<Self::Item> {
        Ok(LuaHttpPipeline {
            vm: Clone::clone(&self.vm),
        })
    }
}

impl TryFrom<&PipelineConf> for LuaHttpPipelineFactory {
    type Error = anyhow::Error;

    fn try_from(value: &PipelineConf) -> Result<Self, Self::Error> {
        const KEY_CONTENT: &str = "content";
        match value
            .get(KEY_CONTENT)
            .ok_or_else(|| CapybaraError::InvalidConfig(KEY_CONTENT.into()))?
        {
            Value::String(s) => {
                let vm = {
                    let vm = Lua::new();
                    vm.load(s).exec()?;
                    Arc::new(Mutex::new(vm))
                };
                Ok(Self { vm })
            }
            _ => bail!(CapybaraError::InvalidConfig(KEY_CONTENT.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::BytesMut;
    use mlua::Lua;
    use tokio::sync::Mutex;

    use crate::pipeline::http::pipeline_lua::LuaHttpPipeline;
    use crate::pipeline::{HttpContext, HttpPipeline};
    use crate::protocol::http::{Headers, RequestLine, StatusLine};

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_lua_pipeline() -> anyhow::Result<()> {
        init();

        // language=lua
        let script = r#"

cnt = 0

function handle_request_line(ctx,request_line)
  print('===== begin handle_request_line =====')
  print('client_addr: '..ctx:client_addr())
  print('uri: '..request_line:uri())
  print('path: '..request_line:path())
  print('path_no_slash: '..request_line:path_no_slash())
  print('method: '..request_line:method())
  print('version: '..request_line:version())
  print('query: '..request_line:query())

  print('urlencode: '..capybara:urlencode('我们'))

  cnt = cnt+1
end

function handle_request_headers(ctx,headers)
  print('===== begin handle_request_headers =====')

  print('-------- request headers --------')
  print('Host: '..headers:get('host'))
  print('Accept: '..headers:get('accept'))

  print('----- foreach header -----')
  for i=1,headers:size() do
    local pair = headers:nth(i)
    print(pair[1]..': '..pair[2])
  end

  print('----- iter x-forwarded-for -----')
  for i,v in ipairs(headers:gets('X-Forwarded-For')) do
    print('X-Forwarded-For#'..tostring(i)..': '..v)
  end

  cnt = cnt+1

end

function handle_status_line(ctx,status_line)
  print('===== begin handle_status_line =====')
  print('status_code: '..tostring(status_line:status_code()))
  print('version: '..status_line:version())
  print('reason_phrase: '..status_line:reason_phrase())
  cnt = cnt+1
end

        "#;

        let vm = {
            let vm = Lua::new();
            vm.load(script).exec()?;
            Arc::new(Mutex::new(vm))
        };

        let p = LuaHttpPipeline {
            vm: Clone::clone(&vm),
        };

        let mut ctx = HttpContext::default();

        // request line
        {
            ctx.reset();
            let mut request_line = RequestLine::builder()
                .uri("//anything/?foo=1&bar=2")
                .build();
            p.handle_request_line(&mut ctx, &mut request_line).await?;
        }

        // request headers
        {
            ctx.reset();
            let mut headers = Headers::builder()
                .put("Host", "www.example.com")
                .put("Accept", "*")
                .put("X-Forwarded-For", "127.0.0.1")
                .put("X-Forwarded-For", "127.0.0.2")
                .put("X-Forwarded-For", "127.0.0.3")
                .build();
            p.handle_request_headers(&mut ctx, &mut headers).await?;
        }

        // status line
        {
            ctx.reset();
            let raw = b"HTTP/1.1 200 OK\r\n";
            let mut b = BytesMut::from(&raw[..]);
            let mut status_line = StatusLine::read(&mut b)?.unwrap();
            p.handle_status_line(&mut ctx, &mut status_line).await?;
        }

        // response headers
        {
            ctx.reset();
            let mut headers = Headers::builder()
                .put("Content-Type", "application/json")
                .put("Server", "FakeServer")
                .build();
            p.handle_response_headers(&mut ctx, &mut headers).await?;
        }

        {
            let w = vm.lock().await;
            w.load("print('cnt: '..tostring(cnt))").exec()?;
        }

        Ok(())
    }
}
