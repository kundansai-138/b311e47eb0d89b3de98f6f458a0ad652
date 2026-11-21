# syntax=docker/dockerfile:1
ARG RUST_VERSION=1.90.0
FROM rust:${RUST_VERSION}-bookworm AS build
WORKDIR /build
ENV CARGO_PROFILE_RELEASE_STRIP=false CARGO_PROFILE_RELEASE_DEBUG=1
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ ./src/
COPY native/ ./native/
COPY vendor/ ./vendor/
ARG CHECK_UNITS=0
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/build/target \
    touch build.rs src/*.rs && if [ "$CHECK_UNITS" = 1 ]; then ROUND4_REQUIRE_AVX512=1 RUSTFLAGS="-C target-cpu=x86-64-v3" cargo test --release --locked; fi && RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --locked && cp target/release/shopping-sessions /build/shop
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /build/shop /app/shopping-sessions
COPY run.sh /app/run.sh
RUN chmod +x /app/run.sh
USER 1000:1000
CMD ["/app/run.sh"]
