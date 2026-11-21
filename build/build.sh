#!/bin/sh
# Build-time only. Runtime contains the optimized executable and run.sh.
set -eu
cd /build
mode=${1:?normal or pgo required}
case "$mode" in normal|pgo) ;; *) echo "Invalid build mode: $mode" >&2; exit 2 ;; esac
case "$(rustc --version)" in 'rustc 1.100.0 '*) ;; *) echo 'Expected Rust 1.100.0' >&2; exit 2 ;; esac
host=$(rustc -vV | sed -n 's/^host: //p')
case "$host" in x86_64-unknown-linux-gnu) ;; *) echo 'Build with --platform linux/amd64' >&2; exit 2 ;; esac
bin=shopping-sessions
common_flags='-C target-cpu=x86-64-v3'
started=$(date +%s)
mkdir -p /artifacts
unset LLVM_PROFILE_FILE
rustc -vV > /artifacts/compiler.txt
sha256sum Cargo.toml Cargo.lock > /artifacts/manifest-sha256.txt
find build.rs src native vendor -type f -print0 | sort -z | xargs -0 sha256sum > /artifacts/source-sha256.txt
find tests/generator fixtures build/pgo-fixtures -type f -print0 | sort -z | xargs -0 sha256sum > /artifacts/training-source-sha256.txt
printf 'mode=%s\nhost=%s\ncommon_flags=%s\n' "$mode" "$host" "$common_flags" > /artifacts/build-info.txt

# Tests use ordinary code generation and a separate target directory. They
# cannot create instrumentation profiles or change the training population.
check_app() {
    label=$1
    shift
    if ! CARGO_TARGET_DIR=/build/target-check RUSTFLAGS="$common_flags" \
        cargo test --release --locked --offline --target "$host" --bin "$bin" "$@" \
        > "/artifacts/unit-$label.log" 2>&1; then
        cat "/artifacts/unit-$label.log" >&2
        exit 1
    fi
    cat "/artifacts/unit-$label.log"
}
check_app default
check_app no-default --no-default-features

build_app() {
    # Explicit --target keeps application flags off host build scripts. Both
    # PGO passes share source and OUT_DIR paths; changed flags trigger rebuilds.
    printf 'compile_phase=%s\n' "$1"
    CARGO_TARGET_DIR=/build/target-app RUSTFLAGS="$2" \
        cargo build --release --locked --offline --target "$host" --bin "$bin"
}

if [ "$mode" = normal ]; then
    build_app normal "$common_flags"
    cp "/build/target-app/$host/release/$bin" /artifacts/shopping-sessions
else
    # No cached or externally supplied profile can be reused for new source.
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

    # The official dependency-free generator is never instrumented.
    CARGO_TARGET_DIR=/build/target-generator RUSTFLAGS="$common_flags" \
        cargo build --release --locked --offline --target "$host" \
        --manifest-path /build/tests/generator/Cargo.toml --bin generate
    generator="/build/target-generator/$host/release/generate"
    printf 'training_events=500000\ntraining_clicks=125000\ntraining_cpus=8\ntraining_seeds=17 173 1729\n' \
        >> /artifacts/build-info.txt

    train() {
        label=$1
        input=$2
        destination="/pgo/training/$label"
        mkdir -p "$destination/output" "$destination/scratch"
        LLVM_PROFILE_FILE="$raw/$label-%m-%p.profraw" DATA_DIR="$input" \
            OUTPUT_DIR="$destination/output" SCRATCH_DIR="$destination/scratch" \
            TMPDIR="$destination/scratch" HOME="$destination/scratch" \
            NCPU=8 MEM_GB=24 "$instrumented" \
            > "/pgo/logs/$label.stdout" 2> "/pgo/logs/$label.stderr"
        printf 'trained %s\n' "$label"
    }
    for seed in 17 173 1729; do
        input="/pgo/training/seed-$seed/input"
        "$generator" --seed "$seed" --events 500000 --clicks 125000 --output "$input"
        train "seed-$seed" "$input"
    done
    supplied=0
    for input in /build/fixtures/*/input; do
        test -f "$input/manifest.json" || continue
        train "supplied-$(basename "$(dirname "$input")")" "$input"
        supplied=$((supplied+1))
    done
    test "$supplied" -eq 36
    extra=0
    for input in /build/build/pgo-fixtures/*; do
        test -f "$input/manifest.json" || continue
        train "edge-$(basename "$input")" "$input"
        extra=$((extra+1))
    done
    test "$extra" -eq 10
    printf 'training_supplied_fixtures=%s\ntraining_extra_fixtures=%s\n' "$supplied" "$extra" >> /artifacts/build-info.txt

    set -- "$raw"/*.profraw
    test -f "$1"
    "$profdata" merge -o "$merged" "$raw"
    "$profdata" show "$merged" > /artifacts/profile-summary.txt
    build_app optimized "$common_flags -Cprofile-use=$merged -Cllvm-args=-pgo-warn-missing-function"
    cp "/build/target-app/$host/release/$bin" /artifacts/shopping-sessions
    cp "$merged" /artifacts/merged.profdata
    cp -r /pgo/logs /artifacts/training-logs
fi
sha256sum /artifacts/shopping-sessions > /artifacts/binary-sha256.txt
elapsed=$(($(date +%s)-started))
printf 'builder_seconds=%s\n' "$elapsed" | tee -a /artifacts/build-info.txt
