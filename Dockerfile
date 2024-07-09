FROM rust:1.79-alpine as builder
WORKDIR /usr/src/capybara
COPY . .

RUN apk add --no-cache musl-dev

RUN cargo build --release && \
    cp target/release/capybara /usr/local/cargo/bin/capybara && \
    cargo clean

FROM alpine:3

LABEL maintainer="jjeffcaii@outlook.com"

RUN apk --no-cache add ca-certificates tzdata libcap

COPY --from=builder /usr/local/cargo/bin/capybara /usr/local/bin/capybara

RUN setcap 'cap_net_admin+ep,cap_net_bind_service+ep' /usr/local/bin/capybara

VOLUME /etc/capybara

ENTRYPOINT ["capybara"]
