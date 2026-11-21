# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY build.rs ./
COPY native/ ./native/
COPY vendor/ ./vendor/
ARG TARGETARCH
RUN --mount=type=cache,id=shopping3-registry,target=/usr/local/cargo/registry --mount=type=cache,id=shopping3-target,target=/build/target set -eu; \
    if [ "$TARGETARCH" = "amd64" ]; then export RUSTFLAGS="-C target-cpu=x86-64-v3"; fi; \
    touch src/*.rs build.rs; \
    cargo test --release --locked --features vqsort-avx512; \
    cargo build --release --locked --offline --features vqsort-avx512; \
    cp target/release/shopping-sessions /build/shopping-sessions

FROM debian:bookworm-slim
RUN useradd --uid 1000 --no-create-home --home-dir /scratch app
COPY --from=builder /build/shopping-sessions /app/shopping-sessions
COPY run.sh /app/run.sh
RUN chmod +x /app/run.sh
ENV DATA_DIR=/data OUTPUT_DIR=/output SCRATCH_DIR=/scratch TMPDIR=/scratch HOME=/scratch
WORKDIR /app
USER 1000:1000
CMD ["/app/run.sh"]
