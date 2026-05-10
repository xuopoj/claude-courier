# syntax=docker/dockerfile:1.7
FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --bin claude-courier && \
    cp target/release/claude-courier /claude-courier

FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/* && \
    useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin courier
COPY --from=builder /claude-courier /usr/local/bin/claude-courier
USER courier
EXPOSE 3007
ENTRYPOINT ["/usr/local/bin/claude-courier"]
CMD ["broker"]
