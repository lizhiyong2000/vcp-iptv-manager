# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --bin vcp-iptv-manager

FROM debian:bookworm-slim AS runtime

# 改用阿里云镜像源加速 APT 下载
RUN sed -i 's@//deb.debian.org/@//mirrors.aliyun.com/@g' /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        wget \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -u 10001 -g nogroup vcp

WORKDIR /app

COPY --from=builder /app/target/release/vcp-iptv-manager /usr/local/bin/vcp-iptv-manager
COPY config.toml /etc/vcp-iptv-manager/config.toml

RUN mkdir -p /app/data \
    && chown -R vcp:nogroup /app/data /etc/vcp-iptv-manager

USER vcp

ENV VCP_CONFIG_PATH=/etc/vcp-iptv-manager/config.toml

EXPOSE 5001

VOLUME ["/app/data"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD wget -qO- http://127.0.0.1:5001/api/stats >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/vcp-iptv-manager"]
