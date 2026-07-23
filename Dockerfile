FROM rust:1.95-bookworm AS builder

WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --locked --release --bin http-credential-broker

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin broker

COPY --from=builder /workspace/target/release/http-credential-broker /usr/local/bin/http-credential-broker

USER broker
EXPOSE 8787

ENTRYPOINT ["/usr/local/bin/http-credential-broker"]
CMD ["--config", "/etc/http-credential-broker/broker.toml"]
