FROM rust:1.79-alpine as builder
WORKDIR /usr/src/capybara
COPY . .

RUN apk add --no-cache musl-dev

RUN cargo build --release && \
    cp target/capybara /usr/local/cargo/bin/capybara && \
    cargo clean

FROM alpine:3

LABEL maintainer="jjeffcaii@outlook.com"

VOLUME /etc/capybara

COPY --from=builder /usr/local/cargo/bin/capybara /usr/local/bin/capybara

RUN setcap cap_net_admin=ep /usr/local/bin/capybara

ENTRYPOINT ["capybara"]
