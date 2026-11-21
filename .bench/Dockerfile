# syntax=docker/dockerfile:1
FROM rust:1.90.0-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ ./src/
COPY native/ ./native/
COPY vendor/ ./vendor/
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/build/target \
    touch build.rs src/*.rs && RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --locked && cp target/release/shopping-sessions /build/shop
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /build/shop /app/shopping-sessions
COPY run.sh /app/run.sh
RUN chmod +x /app/run.sh
USER 1000:1000
CMD ["/app/run.sh"]
