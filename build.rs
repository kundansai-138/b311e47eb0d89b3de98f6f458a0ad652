use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command) {
    let status = command
        .status()
        .expect("launch native compiler/archive tool");
    assert!(status.success(), "native compilation failed: {command:?}");
}

fn compile(out: &Path, avx512: bool) -> PathBuf {
    let suffix = if avx512 { "avx512" } else { "avx2" };
    let object = out.join(format!("sort_{suffix}.o"));
    let mut compiler = Command::new(env::var_os("CXX").unwrap_or_else(|| "g++".into()));
    compiler.args([
        "-std=c++20",
        "-O3",
        "-DNDEBUG",
        "-fPIC",
        "-fno-exceptions",
        "-fno-rtti",
        // Exact feature set checked by simd_sort.rs; no host-specific ISA flags.
        "-march=x86-64",
        "-mavx2",
        "-mbmi",
        "-mbmi2",
        "-mlzcnt",
        "-mpopcnt",
        "-mf16c",
        "-mfma",
        "-msse4.2",
        "-maes",
        "-mpclmul",
        "-Ivendor/highway",
    ]);
    if avx512 {
        // Highway AVX3 / Skylake server subset; does not enable Ice Lake ISAs.
        compiler.args([
            "-mavx512f",
            "-mavx512cd",
            "-mavx512dq",
            "-mavx512bw",
            "-mavx512vl",
        ]);
    }
    compiler.arg(format!("-DSHOPPING_SORT_NAME=shopping_sort128_{suffix}"));
    // Isolate even target-independent weak inline helpers between ISA objects.
    // This prevents compiler auto-vectorization from bypassing runtime gates.
    compiler.arg(format!("-Dhwy=shopping_highway_{suffix}"));
    compiler
        .args(["-c", "native/sort_bridge.cc", "-o"])
        .arg(&object);
    run(&mut compiler);
    object
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(vqsort_native)");
    println!("cargo:rerun-if-changed=native/sort_bridge.cc");
    println!("cargo:rerun-if-changed=vendor/highway");
    println!("cargo:rerun-if-env-changed=CXX");
    println!("cargo:rerun-if-env-changed=AR");
    if env::var_os("CARGO_FEATURE_VQSORT").is_none()
        || env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux")
        || env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64")
    {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut objects = vec![compile(&out, false)];
    if env::var_os("CARGO_FEATURE_VQSORT_AVX512").is_some() {
        objects.push(compile(&out, true));
    }
    let mut archive = Command::new(env::var_os("AR").unwrap_or_else(|| "ar".into()));
    archive
        .arg("crs")
        .arg(out.join("libshopping_vqsort.a"))
        .args(objects);
    run(&mut archive);
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=shopping_vqsort");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-cfg=vqsort_native");
}
