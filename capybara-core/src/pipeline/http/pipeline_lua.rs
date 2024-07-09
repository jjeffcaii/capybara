use std::sync::Arc;

use async_trait::async_trait;
use mlua::prelude::*;
use mlua::{Function, Lua, UserData, UserDataFields, UserDataMethods};
use tokio::sync::Mutex;

use crate::pipeline::{HttpContext, HttpPipeline, HttpPipelineFactory, PipelineConf};
use crate::protocol::http::{Headers, RequestLine};

struct LuaHttpContext(*mut HttpContext);

impl UserData for LuaHttpContext {
    fn add_fields<'lua, F: UserDataFields<'lua, Self>>(fields: &mut F) {}

    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("client_addr", |_, this, ()| {
            let ctx = unsafe { this.0.as_mut() }.unwrap();
            Ok(ctx.client_addr().to_string())
        });
    }
}

struct LuaRequestLine(*mut RequestLine);

impl UserData for LuaRequestLine {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("path", |lua, this, ()| {
            let request_line = unsafe { this.0.as_mut() }.unwrap();
            lua.create_string(request_line.path_bytes())
        });
    }
}

struct LuaHeaders(*mut Headers);

impl UserData for LuaHeaders {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("get", |lua, this, name: LuaString| {
            let headers = unsafe { this.0.as_mut() }.unwrap();
            let key = unsafe { std::str::from_utf8_unchecked(name.as_ref()) };
            match headers.get_bytes(key) {
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
            let handler = globals.get::<_, Function>("handle_request_line");
            if let Ok(fun) = handler {
                vm.scope(|scope| {
                    let ctx = scope.create_userdata(LuaHttpContext(ctx))?;
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
            let handler = globals.get::<_, Function>("handle_request_headers");
            if let Ok(fun) = handler {
                vm.scope(|scope| {
                    let ctx = scope.create_userdata(LuaHttpContext(ctx))?;
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
}

pub(crate) struct LuaHttpPipelineFactory {}

impl HttpPipelineFactory for LuaHttpPipelineFactory {
    type Item = LuaHttpPipeline;

    fn generate(&self) -> anyhow::Result<Self::Item> {
        todo!()
    }
}

impl TryFrom<&PipelineConf> for LuaHttpPipelineFactory {
    type Error = anyhow::Error;

    fn try_from(value: &PipelineConf) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mlua::Lua;
    use tokio::sync::Mutex;

    use crate::pipeline::http::pipeline_lua::LuaHttpPipeline;
    use crate::pipeline::{HttpContext, HttpPipeline};
    use crate::protocol::http::{Headers, RequestLine};

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_lua_pipeline() -> anyhow::Result<()> {
        init();

        // language=lua
        let script = r#"
function handle_request_line(ctx,request_line)
  print('client_addr: '..ctx:client_addr())
  print('path: '..request_line:path())
end

function handle_request_headers(ctx,headers)
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

end

        "#;

        let lua = Lua::new();
        lua.load(script).exec()?;

        let p = LuaHttpPipeline {
            vm: Arc::new(Mutex::new(lua)),
        };

        let mut ctx = HttpContext::default();

        let mut request_line = RequestLine::builder().uri("/anything").build();
        p.handle_request_line(&mut ctx, &mut request_line).await?;

        ctx.reset();
        let mut headers = Headers::builder()
            .put("Host", "www.example.com")
            .put("Accept", "*")
            .put("X-Forwarded-For", "127.0.0.1")
            .put("X-Forwarded-For", "127.0.0.2")
            .put("X-Forwarded-For", "127.0.0.3")
            .build();
        p.handle_request_headers(&mut ctx, &mut headers).await?;

        Ok(())
    }
}
