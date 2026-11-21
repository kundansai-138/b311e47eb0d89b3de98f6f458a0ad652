FROM rust:1.90-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
ARG TARGETARCH
RUN set -eu; \
    if [ "$TARGETARCH" = "amd64" ]; then export RUSTFLAGS="-C target-cpu=x86-64-v3 -C target-feature=+aes"; fi; \
    cargo test --release --locked; \
    cargo build --release --locked --offline

FROM debian:bookworm-slim
RUN useradd --uid 1000 --no-create-home --home-dir /scratch app
COPY --from=builder /build/target/release/shopping-sessions /app/shopping-sessions
COPY run.sh /app/run.sh
RUN chmod +x /app/run.sh
ENV DATA_DIR=/data OUTPUT_DIR=/output SCRATCH_DIR=/scratch TMPDIR=/scratch HOME=/scratch
WORKDIR /app
USER 1000:1000
CMD ["/app/run.sh"]
