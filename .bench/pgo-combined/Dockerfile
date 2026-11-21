FROM rust:1.90.0-bookworm AS source
WORKDIR /build/app
COPY app/ ./
COPY binary-name /build/binary-name
COPY build.sh /build/build.sh
RUN chmod +x /build/build.sh && cargo fetch --locked

FROM source AS normal-build
RUN /build/build.sh normal

FROM source AS pgo-build
RUN rustup component add llvm-tools-preview
COPY generator/ /build/generator/
COPY training-fixtures/ /build/training-fixtures/
ARG PGO_SEEDS="17 173 1729"
ARG PGO_EVENTS=500000
ARG PGO_CLICKS=125000
ARG PGO_NCPU=8
ARG PGO_MEM_GB=24
RUN PGO_SEEDS="$PGO_SEEDS" PGO_EVENTS="$PGO_EVENTS" PGO_CLICKS="$PGO_CLICKS" \
    PGO_NCPU="$PGO_NCPU" PGO_MEM_GB="$PGO_MEM_GB" /build/build.sh pgo

FROM debian:bookworm-slim AS runtime
RUN useradd --uid 1000 --no-create-home --home-dir /scratch app
COPY run.sh /app/run.sh
RUN chmod +x /app/run.sh
ENV DATA_DIR=/data OUTPUT_DIR=/output SCRATCH_DIR=/scratch TMPDIR=/scratch HOME=/scratch
WORKDIR /app
USER 1000:1000
CMD ["/app/run.sh"]

FROM runtime AS normal
COPY --from=normal-build /artifacts/shopping-sessions /app/shopping-sessions

FROM runtime AS pgo
COPY --from=pgo-build /artifacts/shopping-sessions /app/shopping-sessions

FROM scratch AS pgo-artifacts
COPY --from=pgo-build /artifacts/ /
