![logo](./docs/capybara.jpg)

# Capybara

![GitHub Actions Workflow Status](https://img.shields.io/github/actions/workflow/status/jjeffcaii/capybara/rust.yml)
[![Codecov](https://img.shields.io/codecov/c/github/jjeffcaii/capybara)](https://app.codecov.io/gh/jjeffcaii/capybara)
[![Crates.io Version](https://img.shields.io/crates/v/capybara-bin)](https://crates.io/crates/capybara-bin)
[![Crates.io Total Downloads](https://img.shields.io/crates/d/capybara-bin)](https://crates.io/crates/capybara-bin)
![GitHub Tag](https://img.shields.io/github/v/tag/jjeffcaii/capybara)
![GitHub License](https://img.shields.io/github/license/jjeffcaii/capybara)

A reverse proxy in Rust, which is inspired from Nginx/OpenResty/Envoy.

> WARNING: still in an active development!!!

## Quick Start

- Prepare Bootstrap YAML

```yaml
loggers:
  main:
    path: ~/capybara/logs/main.log

providers:
  - kind: static_file
    props:
      path: /your/path/config.yaml
```

- Prepare config YAML

```yaml
listeners:
  httpbin:
    listen: 0.0.0.0:80
    protocol:
      name: http
      props:
        client_header_timeout: 30s
    pipelines:
      - name: capybara.pipelines.http.lua
        props:
          # write lua script
          content: |
            local cnts = 0

            function handle_request_line(ctx,request_line)
              -- set the upstream here, which links to 'upstreams.httpbin':
              ctx:set_upstream('upstream://httpbin')
            end

            function handle_status_line(ctx,status_line)
              ctx:replace_header('X-Powered-By','capybara')

              -- set a custom response header which counts the requests:
              cnts = cnts + 1
              ctx:replace_header('X-Capybara-Requests', tostring(cnts))
            end

upstreams:
  httpbin:
    transport: tcp
    resolver: default
    balancer: weighted
    endpoints:
      - addr: httpbin.org:443
        weight: 70
      - addr: postman-echo.com:443
        weight: 30

```

- Run & Test

```shell
$ cargo run --bin capybara -- run -c bootstrap.yaml
$ curl -i http://localhost/get
```
