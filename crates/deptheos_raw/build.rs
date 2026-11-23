//! Builds LibRaw (the submodule `native/vendor/LibRaw` at the repo root) and
//! this crate's C API (`native/raw_c_api.cpp`) into static libraries. LibRaw
//! is reused from the per-user native cache when one of the same build
//! exists (`native_build::native_cache`); the C API always compiles.

use std::path::{Path, PathBuf};

use native_build::build_sources::collect_sources;
use native_build::flags::Flags;
use native_build::native_cache;

/// LibRaw's placeholder stubs for builds without postprocessing. They define
/// the same symbols as the real implementations, so linking both clashes.
const LIBRAW_EXCLUDED: &[&str] = &[
    "postprocessing/postprocessing_ph.cpp",
    "preprocessing/preprocessing_ph.cpp",
    "write/write_ph.cpp",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let native = manifest.join("../../native");
    let libraw = native.join("vendor/LibRaw");

    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=build.rs");
    // `raw_c_api.h` for dependents' native code (deptheos_dng reads it as
    // DEP_DEPTHEOS_RAW_INCLUDE): DtRawMetadata is handed through unchanged.
    println!("cargo:include={}", manifest.join("native").display());

    if !libraw.join("src").is_dir() {
        panic!(
            "LibRaw sources not found at {}. It is a git submodule — run:\n    \
             git submodule update --init native/vendor/LibRaw",
            libraw.display()
        );
    }
    // A directory is watched recursively, so this covers LibRaw's headers
    // (libraw/, internal/) as well as its sources.
    println!("cargo:rerun-if-changed={}", libraw.display());

    // Emission order is link order, most-dependent first: the C API, then
    // LibRaw.
    libraw_build(&manifest, &libraw, Code::Ours)
        .file(manifest.join("native/raw_c_api.cpp"))
        .compile("deptheos_raw_native");

    let inputs = [libraw.clone(), manifest.join("build.rs"), manifest.join("../native_build/src")];
    native_cache::build_or_reuse("libraw", &["deptheos_libraw"], &inputs, || {
        let sources = collect_sources(&libraw.join("src"), "cpp", true, LIBRAW_EXCLUDED);
        libraw_build(&manifest, &libraw, Code::LibRaw).files(&sources).compile("deptheos_libraw");
    });
    // After LibRaw, which uses it, and on a cached build too.
    if is_linux_gcc(cc::Build::new().cpp(true)) {
        println!("cargo:rustc-link-lib=gomp");
    }
}

/// Whose code a build compiles, which decides its warnings
/// (`native_build::flags`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Code {
    /// LibRaw's.
    LibRaw,
    /// The C API in `native/`, which includes LibRaw's headers.
    Ours,
}

/// Settings LibRaw and the C API over it share.
fn libraw_build(manifest: &Path, libraw: &Path, code: Code) -> cc::Build {
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").include(manifest.join("native"));
    match code {
        Code::Ours => build.ours(&[libraw.to_path_buf()]),
        Code::LibRaw => build.include(libraw).vendored(),
    };

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Static link: suppress LibRaw's dllimport/dllexport decoration.
        build.define("LIBRAW_NODLL", None);
        build.define("NOMINMAX", None);
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // LibRaw parallelizes decoding and demosaicing with OpenMP (GCD on
        // Apple, no flag); without it a 45 MP CR3 takes about 1.1 s on one
        // core. The binaries then load vcomp140.dll, which the Windows
        // installer must ship with the VC++ runtime.
        build.flag("/openmp");
    }
    if is_linux_gcc(&build) {
        // The same OpenMP on Linux, through GCC's libgomp, which ships with
        // GCC (`main` links it). Clang would need LLVM's libomp, so a
        // clang build stays on one core.
        build.flag("-fopenmp");
    }
    build
}

/// Whether this is a Linux build compiled by GCC, the one that gets OpenMP
/// through libgomp.
fn is_linux_gcc(build: &cc::Build) -> bool {
    std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
        && build.get_compiler().is_like_gnu()
}
