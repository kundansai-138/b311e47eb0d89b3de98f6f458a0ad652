#!/bin/sh
set -eu
: "${DATA_DIR:=/data}" "${OUTPUT_DIR:=/output}" "${SCRATCH_DIR:=/scratch}" "${NCPU:=8}"
export DATA_DIR OUTPUT_DIR SCRATCH_DIR NCPU
export TMPDIR="$SCRATCH_DIR" HOME="$SCRATCH_DIR"
exec /app/shopping-sessions
