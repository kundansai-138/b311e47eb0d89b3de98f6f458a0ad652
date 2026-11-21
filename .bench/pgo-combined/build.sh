#!/bin/sh
# Executed only in the pinned Rust builder. Runtime has no training machinery.
set -eu
mode=${1:?normal or pgo required}
case "$mode" in normal|pgo) ;; *) echo "Invalid build mode: $mode" >&2; exit 2 ;; esac
case "$(rustc --version)" in 'rustc 1.90.0 '*) ;; *) echo 'Expected Rust 1.90.0' >&2; exit 2 ;; esac
host=$(rustc -vV | sed -n 's/^host: //p')
bin=$(cat /build/binary-name)
case "$bin" in ''|*[!a-zA-Z0-9_-]*) echo 'Invalid binary name' >&2; exit 2 ;; esac
common_flags=''
case "$host" in x86_64-*) common_flags='-C target-cpu=x86-64-v3' ;; esac
started=$(date +%s)
mkdir -p /artifacts
rustc -vV > /artifacts/compiler.txt
sha256sum Cargo.toml Cargo.lock > /artifacts/manifest-sha256.txt
find src -type f -print0 | sort -z | xargs -0 sha256sum > /artifacts/source-sha256.txt
printf 'mode=%s\nhost=%s\ncommon_flags=%s\n' "$mode" "$host" "$common_flags" > /artifacts/build-info.txt

build_app() {
    # --target keeps instrumentation flags away from host build scripts.
    # Reuse the target path between phases as well: generated OUT_DIR source
    # paths stay stable, while changed RUSTFLAGS force the needed recompiles.
    printf 'compile_phase=%s\n' "$1"
    CARGO_TARGET_DIR=/build/target-app RUSTFLAGS="$2" \
        cargo build --release --locked --offline --target "$host" --bin "$bin"
}

if [ "$mode" = normal ]; then
    build_app normal "$common_flags"
    cp "/build/target-app/$host/release/$bin" /artifacts/shopping-sessions
else
    # A new stage has no profiles from any earlier source or training run.
    raw=/pgo/raw
    merged=/pgo/merged.profdata
    test ! -e /pgo
    mkdir -p "$raw" /pgo/training /pgo/logs
    sysroot=$(rustc --print sysroot)
    profdata="$sysroot/lib/rustlib/$host/bin/llvm-profdata"
    test -x "$profdata"
    "$profdata" --version > /artifacts/llvm-profdata.txt
    build_app instrumented "$common_flags -Cprofile-generate=$raw"
    instrumented="/build/target-app/$host/release/$bin"
    # The official, dependency-free generator is never instrumented.
    CARGO_TARGET_DIR=/build/target-generator RUSTFLAGS="$common_flags" \
        cargo build --release --locked --offline --target "$host" \
        --manifest-path /build/generator/Cargo.toml --bin generate
    generator="/build/target-generator/$host/release/generate"
    events=${PGO_EVENTS:-500000}
    clicks=${PGO_CLICKS:-125000}
    cpus=${PGO_NCPU:-8}
    memory=${PGO_MEM_GB:-24}
    for number in "$events" "$clicks" "$cpus" "$memory"; do
        case "$number" in ''|*[!0-9]*) echo 'Training counts must be integers' >&2; exit 2 ;; esac
    done
    printf 'training_events=%s\ntraining_clicks=%s\ntraining_cpus=%s\ntraining_seeds=%s\n' \
        "$events" "$clicks" "$cpus" "${PGO_SEEDS:-17 173 1729}" >> /artifacts/build-info.txt
    train() {
        label=$1
        input=$2
        mkdir -p "/pgo/training/$label/output" "/pgo/training/$label/scratch"
        LLVM_PROFILE_FILE="$raw/$label-%m-%p.profraw" DATA_DIR="$input" \
            OUTPUT_DIR="/pgo/training/$label/output" SCRATCH_DIR="/pgo/training/$label/scratch" \
            NCPU="$cpus" MEM_GB="$memory" "$instrumented" > "/pgo/logs/$label.stdout" 2> "/pgo/logs/$label.stderr"
        printf 'trained %s\n' "$label"
    }
    for seed in ${PGO_SEEDS:-17 173 1729}; do
        case "$seed" in ''|*[!0-9]*) echo 'Seeds must be integers' >&2; exit 2 ;; esac
        input="/pgo/training/seed-$seed/input"
        "$generator" --seed "$seed" --events "$events" --clicks "$clicks" --output "$input"
        train "seed-$seed" "$input"
    done
    for input in /build/training-fixtures/*; do
        test -f "$input/manifest.json" || continue
        train "edge-$(basename "$input")" "$input"
    done
    set -- "$raw"/*.profraw
    test -f "$1"
    "$profdata" merge -o "$merged" "$raw"
    "$profdata" show "$merged" > /artifacts/profile-summary.txt
    # All other flags, source paths, dependencies, target and Cargo profile
    # match the instrumentation build. No instrumented executable is shipped.
    build_app optimized "$common_flags -Cprofile-use=$merged -Cllvm-args=-pgo-warn-missing-function"
    cp "/build/target-app/$host/release/$bin" /artifacts/shopping-sessions
    cp "$merged" /artifacts/merged.profdata
    cp -r /pgo/logs /artifacts/training-logs
fi
sha256sum /artifacts/shopping-sessions > /artifacts/binary-sha256.txt
elapsed=$(($(date +%s)-started))
printf 'builder_seconds=%s\n' "$elapsed" | tee -a /artifacts/build-info.txt
