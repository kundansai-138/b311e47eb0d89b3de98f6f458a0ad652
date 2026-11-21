FROM rust:1.100.0-bookworm AS source
WORKDIR /build
RUN command -v g++ && command -v ar
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY src/ ./src/
COPY native/ ./native/
COPY vendor/ ./vendor/
COPY tests/generator/Cargo.toml tests/generator/Cargo.lock ./tests/generator/
COPY tests/generator/src/ ./tests/generator/src/
COPY fixtures/ ./fixtures/
COPY build/build.sh ./build/build.sh
COPY build/pgo-fixtures/ ./build/pgo-fixtures/

FROM source AS normal-build
RUN timeout 570 sh -c 'cargo fetch --locked && sh /build/build/build.sh normal'

FROM source AS pgo-build
RUN timeout 570 sh -c 'rustup component add llvm-tools-preview && cargo fetch --locked && sh /build/build/build.sh pgo'

# Optional export target; it must not be the default final image.
FROM scratch AS pgo-artifacts
COPY --from=pgo-build /artifacts/ /

FROM debian:bookworm-slim AS runtime-base
RUN useradd --uid 1000 --no-create-home --home-dir /scratch app
COPY run.sh /app/run.sh
RUN chmod +x /app/run.sh
ENV DATA_DIR=/data OUTPUT_DIR=/output SCRATCH_DIR=/scratch TMPDIR=/scratch HOME=/scratch
WORKDIR /app
USER 1000:1000
CMD ["/app/run.sh"]

FROM runtime-base AS normal
COPY --from=normal-build /artifacts/shopping-sessions /app/shopping-sessions
RUN set -eu; dependencies="$(ldd /app/shopping-sessions)"; printf '%s\n' "$dependencies"; case "$dependencies" in *"not found"*) exit 1 ;; esac

FROM runtime-base AS pgo
COPY --from=pgo-build /artifacts/shopping-sessions /app/shopping-sessions
RUN set -eu; dependencies="$(ldd /app/shopping-sessions)"; printf '%s\n' "$dependencies"; case "$dependencies" in *"not found"*) exit 1 ;; esac

# Plain docker build must produce the optimized, runnable submission.
FROM pgo AS runtime
