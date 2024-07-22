FROM rust:1.79 AS builder
WORKDIR /usr/src/capybara
COPY . .

RUN cargo build --release && \
    cp target/release/capybara /usr/local/cargo/bin/capybara && \
    cargo clean

FROM ubuntu:jammy

LABEL maintainer="jjeffcaii@outlook.com"

RUN apt-get update && \
    apt-get install -y libcap2-bin && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/capybara /usr/local/bin/capybara

RUN setcap 'cap_net_admin+ep cap_net_bind_service+ep' /usr/local/bin/capybara

VOLUME /etc/capybara

ENTRYPOINT ["capybara"]
