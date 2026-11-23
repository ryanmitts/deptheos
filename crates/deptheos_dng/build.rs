//! Builds the Adobe DNG SDK, the XMP toolkit, libjpeg and libjxl from the
//! repo root's `native/vendor/`, and the DngBridge and its C API from this
//! crate's `native/`. Nothing is fetched. An unconfigured target fails the
//! build: every front end needs the SDK.

use std::path::{Path, PathBuf};

use native_build::build_sources::collect_sources;
use native_build::flags::Flags;
use native_build::native_cache;

/// The platforms the native build is configured for, read once from
/// `CARGO_CFG_TARGET_OS`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Mac,
    Ios,
    Windows,
    Linux,
}

impl Target {
    fn current() -> Result<Self, String> {
        match std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default().as_str() {
            "macos" => Ok(Self::Mac),
            "ios" => Ok(Self::Ios),
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            other => Err(other.to_string()),
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(dng_test_hooks)");

    let target = Target::current().unwrap_or_else(|target_os| {
        panic!(
            "deptheos_dng: the DNG SDK's build is configured for macOS, iOS, Windows and \
             Linux, not {target_os:?}"
        )
    });
    if test_hooks() {
        println!("cargo::rustc-cfg=dng_test_hooks");
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest.join("../../native/vendor");
    // deptheos_raw's `raw_c_api.h`, for DtRawMetadata.
    let raw_include = PathBuf::from(
        std::env::var("DEP_DEPTHEOS_RAW_INCLUDE").expect("deptheos_raw exports its include dir"),
    );

    // Printed here rather than by the builders, which do not run when the
    // vendored libraries come from the native cache.
    for dir in ["dng_sdk/source", "xmp/toolkit", "libjxl/libjxl", "libjpeg"] {
        println!("cargo:rerun-if-changed={}", vendor.join(dir).display());
    }

    // Emission order is link order: cc prints `rustc-link-lib` as each library
    // finishes and a cache hit prints VENDORED's order, so both list each
    // library before the ones it calls, since this crate's tests link them one
    // by one and GNU ld resolves each only against later ones. The bridge
    // always compiles; the vendored libraries come from the native cache when
    // it holds the same build (`native_build::native_cache`).
    build_dng_bridge(target, &manifest, &vendor, &raw_include);

    const VENDORED: &[&str] = &[
        "deptheos_dng_sdk",
        "deptheos_xmpcore",
        "deptheos_expat",
        "deptheos_zlib",
        "deptheos_jxl",
        "deptheos_brotli",
        "deptheos_highway",
        "deptheos_skcms",
        "deptheos_jpeg",
    ];
    let inputs = [
        vendor.join("dng_sdk"),
        vendor.join("xmp"),
        vendor.join("libjxl"),
        vendor.join("libjpeg"),
        manifest.join("build.rs"),
        manifest.join("../native_build/src"),
    ];
    native_cache::build_or_reuse("dng-vendor", VENDORED, &inputs, || {
        build_dng_sdk(target, &vendor);
        build_xmp(target, &vendor);
        build_libjxl(&vendor);
        build_libjpeg(&vendor);
    });
    link_system_libraries(target);
}

/// The platform libraries the DNG SDK and XMP call into.
fn link_system_libraries(target: Target) {
    match target {
        Target::Mac => {
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=CoreServices");
            println!("cargo:rustc-link-lib=framework=ApplicationServices");
        }
        Target::Ios => println!("cargo:rustc-link-lib=framework=CoreFoundation"),
        Target::Windows => {
            // MessageBoxA in dng_show_message; CoCreateGuid for XMP UUIDs.
            println!("cargo:rustc-link-lib=user32");
            println!("cargo:rustc-link-lib=ole32");
        }
        Target::Linux => {}
    }
}

/// libjxl and its bundled third-party deps. The DNG SDK's `dng_jxl.cpp`
/// includes the jxl C++ headers unconditionally, so this is not optional.
fn build_libjxl(vendor: &Path) {
    let jxl_root = vendor.join("libjxl/libjxl");
    let third = jxl_root.join("third_party");
    let brotli = third.join("brotli");
    let highway = third.join("highway");
    let skcms = third.join("skcms");
    let lib = jxl_root.join("lib");

    // --- libjxl proper ---
    // Test-only translation units that ship inside lib/jxl.
    const JXL_EXCLUDED: &[&str] =
        &["dec_transforms_testonly.cc", "test_memory_manager.cc", "test_utils.cc", "test_image.cc"];
    let mut jxl_sources = collect_sources(&lib.join("jxl"), "cc", true, JXL_EXCLUDED);
    jxl_sources.extend(collect_sources(&lib.join("threads"), "cc", true, JXL_EXCLUDED));

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&lib)
        .include(lib.join("include"))
        // Hand-written replacements for the CMake-generated *_export.h headers.
        .include(vendor.join("libjxl/client_projects/include"))
        .include(&jxl_root)
        .include(&highway)
        .include(brotli.join("c/include"))
        .include(&skcms)
        .define("JPEGXL_ENABLE_SKCMS", "1")
        .vendored()
        .files(&jxl_sources)
        .compile("deptheos_jxl");

    // --- brotli (C) ---
    let mut brotli_sources = Vec::new();
    for part in ["c/common", "c/dec", "c/enc"] {
        brotli_sources.extend(collect_sources(&brotli.join(part), "c", true, &[]));
    }
    cc::Build::new()
        .include(brotli.join("c/include"))
        .vendored()
        .files(&brotli_sources)
        .compile("deptheos_brotli");

    // --- highway (C++): only the runtime pieces jxl links against ---
    const HIGHWAY_SOURCES: &[&str] = &[
        "hwy/abort.cc",
        "hwy/aligned_allocator.cc",
        "hwy/nanobenchmark.cc",
        "hwy/per_target.cc",
        "hwy/print.cc",
        "hwy/stats.cc",
        "hwy/targets.cc",
        "hwy/timer.cc",
        "hwy/contrib/thread_pool/topology.cc",
    ];
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&highway)
        .vendored()
        .files(HIGHWAY_SOURCES.iter().map(|f| highway.join(f)))
        .compile("deptheos_highway");

    // --- skcms (C++) ---
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&skcms)
        // Only the baseline transforms are vendored (no
        // skcms_TransformHsw/Skx.cc), so switch off the dispatch to the
        // AVX2/AVX-512 variants.
        .define("SKCMS_DISABLE_HSW", None)
        .define("SKCMS_DISABLE_SKX", None)
        .vendored()
        .file(skcms.join("skcms.cc"))
        .file(skcms.join("src/skcms_TransformBaseline.cc"))
        .compile("deptheos_skcms");
}

/// IJG libjpeg 9c. The DNG SDK's `dng_jpeglib.h` expects this ABI
/// (JPEG_LIB_VERSION 90) — libjpeg-turbo is NOT a drop-in here.
fn build_libjpeg(vendor: &Path) {
    // Command-line tools and alternative memory managers: not part of the
    // library.
    const EXCLUDED: &[&str] = &[
        "cjpeg.c",
        "djpeg.c",
        "jpegtran.c",
        "rdjpgcom.c",
        "wrjpgcom.c",
        "ckconfig.c",
        "example.c",
        "cdjpeg.c",
        "rdcolmap.c",
        "rdswitch.c",
        "rdbmp.c",
        "rdgif.c",
        "rdppm.c",
        "rdrle.c",
        "rdtarga.c",
        "wrbmp.c",
        "wrgif.c",
        "wrppm.c",
        "wrrle.c",
        "wrtarga.c",
        "transupp.c",
        "jmemansi.c",
        "jmemdos.c",
        "jmemmac.c",
        "jmemname.c",
    ];

    let root = vendor.join("libjpeg");
    let sources = collect_sources(&root, "c", false, EXCLUDED);
    assert!(!sources.is_empty(), "no libjpeg sources found in {}", root.display());

    let mut build = cc::Build::new();
    build.include(&root).define("NO_GETENV", None).vendored();
    // The DNG SDK's `dng_error_exit` throws a C++ exception through
    // libjpeg's C frames, which need unwind tables (MSVC makes them on every
    // 64-bit target).
    if !build.get_compiler().is_like_msvc() {
        build.flag("-fexceptions");
    }
    build.files(&sources).compile("deptheos_jpeg");
}

/// Header search paths shared by every XMP translation unit, relative to
/// `native/vendor/xmp/toolkit`.
fn xmp_includes(target: Target, toolkit: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = [
        ".",
        "build",
        "public/include",
        "public/include/client-glue",
        "XMPCommon",
        "XMPCommon/Interfaces",
        "XMPCommon/ImplHeaders",
        "XMPCommon/Utilities",
        "XMPCore",
        "XMPCore/Interfaces",
        "XMPCore/source",
        "XMPCore/third-party/expat/public/lib",
        "third-party/zuid/interfaces",
    ]
    .iter()
    .map(|p| toolkit.join(p))
    .collect();

    // Platform resource dirs carry expat_config.h and the build config.
    let resource = match target {
        Target::Windows => "win",
        Target::Linux => "linux",
        Target::Mac | Target::Ios => "mac", // iOS uses the mac resources too
    };
    paths.push(toolkit.join(format!("XMPCore/resource/{resource}")));
    // On Unix, XMPDocOps-Utils.cpp makes UUIDs with Boost's; the toolkit
    // ships the headers it needs, as Adobe's own Unix build includes them.
    if resource == "linux" {
        paths.push(toolkit.join("XMPCore/third-party/boost"));
    }
    paths
}

/// The platform switch XMP keys off (`MAC_ENV` / `IOS_ENV` / `WIN_ENV` /
/// `UNIX_ENV`), which the DNG SDK's headers read too.
fn os_defines(build: &mut cc::Build, target: Target) {
    match target {
        Target::Mac => build.define("MAC_ENV", "1"),
        Target::Ios => build.define("IOS_ENV", "1"),
        // Host_IO-Win and OS_Utils_WIN pass wide strings to the unsuffixed
        // Win32 names, hence UNICODE.
        Target::Windows => build
            .define("WIN_ENV", "1")
            .define("NOMINMAX", None)
            .define("UNICODE", None)
            .define("_UNICODE", None),
        Target::Linux => build.define("UNIX_ENV", "1"),
    };
}

/// Defines shared by every XMP translation unit.
fn xmp_defines(build: &mut cc::Build, target: Target) {
    os_defines(build, target);
    build
        .define("XMP_StaticBuild", "1")
        .define("AdobePrivate", "1")
        .define("XML_STATIC", None)
        // The toolkit's adapter refuses any packet with a DOCTYPE: an
        // entity can only be declared there, and the five predefined ones
        // still work.
        .define("BanAllEntityUsage", "1")
        .define("HAVE_EXPAT_CONFIG_H", "1")
        .define("XMP_64", "1")
        .define("ENABLE_CPP_DOM_MODEL", "1");
}

/// Where expat's hash salt comes from on `target`: the source file to
/// compile and the define that selects it (Windows needs none, `_WIN32`
/// selects `rand_s`). The toolkit's `expat_config.h` names no source.
fn expat_entropy(target: Target) -> (&'static str, Option<&'static str>) {
    match target {
        Target::Mac | Target::Ios => ("random_arc4random_buf.c", Some("HAVE_ARC4RANDOM_BUF")),
        Target::Linux => ("random_dev_urandom.c", Some("XML_DEV_URANDOM")),
        Target::Windows => ("random_rand_s.c", None),
    }
}

/// Adobe XMP toolkit: XMPCore (with expat compiled from source), through
/// which the DNG SDK reads and writes XMP packets (`qDNGUseXMP` stays at
/// its default of 1); and zlib, which the SDK compresses with.
fn build_xmp(target: Target, vendor: &Path) {
    let toolkit = vendor.join("xmp/toolkit");
    let includes = xmp_includes(target, &toolkit);

    // ---------------- XMPCore ----------------
    // These are #included by xmltok.c rather than compiled on their own.
    // The `random_*.c` files are one per source of the hash salt's
    // entropy; `expat_entropy` names the one this target compiles.
    const EXPAT_EXCLUDED: &[&str] = &[
        "xmltok_impl.c",
        "xmltok_ns.c",
        "random_arc4random.c",
        "random_arc4random_buf.c",
        "random_dev_urandom.c",
        "random_getentropy.c",
        "random_getrandom.c",
        "random_rand_s.c",
    ];
    // Android-only helper that ships in the shared source dir.
    const XMPCORE_EXCLUDED: &[&str] = &["Android_Utils.cpp"];

    let mut core_cpp =
        collect_sources(&toolkit.join("XMPCommon/source"), "cpp", true, XMPCORE_EXCLUDED);
    core_cpp.extend(collect_sources(
        &toolkit.join("XMPCore/source"),
        "cpp",
        true,
        XMPCORE_EXCLUDED,
    ));
    core_cpp.extend(collect_sources(
        &toolkit.join("public/include/XMPCommon/source"),
        "cpp",
        true,
        &[],
    ));
    // Shared toolkit sources XMPCore calls into. MD5 comes from
    // third-party/zuid/sources, not interfaces/: both exist, and compiling
    // both duplicates symbols.
    core_cpp.extend(
        [
            "source/UnicodeConversions.cpp",
            "source/XML_Node.cpp",
            "source/XMP_LibUtils.cpp",
            "third-party/zuid/sources/MD5.cpp",
        ]
        .iter()
        .map(|f| toolkit.join(f)),
    );

    let expat_lib = toolkit.join("XMPCore/third-party/expat/public/lib");
    let mut expat_c = collect_sources(&expat_lib, "c", true, EXPAT_EXCLUDED);
    let (entropy_source, entropy_define) = expat_entropy(target);
    expat_c.push(expat_lib.join(entropy_source));

    let core_defines: &[(&str, Option<&str>)] = &[
        ("BUILDING_XMPCORE_LIB", Some("1")),
        ("BUILDING_XMPCORE_AS_STATIC", Some("1")),
        ("BUILDING_XMPCOMMON_LIB", Some("1")),
        ("BUILDING_XMPCOMMON_AS_STATIC", Some("1")),
        ("XMP_COMPONENT_INT_NAMESPACE", Some("AdobeXMPCore_Int")),
    ];

    let mut core = cc::Build::new();
    core.cpp(true).std("c++17");
    xmp_defines(&mut core, target);
    for (k, v) in core_defines {
        core.define(k, *v);
    }
    for inc in &includes {
        core.include(inc);
    }
    core.vendored().files(&core_cpp).compile("deptheos_xmpcore");

    // expat is C; it needs the same defines but not the C++ standard.
    let mut expat = cc::Build::new();
    xmp_defines(&mut expat, target);
    for (k, v) in core_defines {
        expat.define(k, *v);
    }
    for inc in &includes {
        expat.include(inc);
    }
    // General entities as expat builds by default; the toolkit's
    // `expat_config.h` predates the setting.
    expat.define("XML_GE", "1");
    if let Some(define) = entropy_define {
        expat.define(define, "1");
    }
    expat.vendored().files(&expat_c).compile("deptheos_expat");

    // zlib, minus the gzip file I/O layer (it calls read/write/close without
    // <unistd.h> and XMP does not use it).
    const ZLIB_EXCLUDED: &[&str] =
        &["example.c", "minigzip.c", "gzclose.c", "gzlib.c", "gzread.c", "gzwrite.c"];
    let zlib_c = collect_sources(&toolkit.join("third-party/zlib"), "c", true, ZLIB_EXCLUDED);

    let mut zlib = cc::Build::new();
    xmp_defines(&mut zlib, target);
    // On Apple platforms zutil.h defines `fdopen(fd,mode)` as NULL unless
    // defined, which mangles <stdio.h>'s declaration; defining the name skips
    // that. Only the excluded gz* layer calls fdopen.
    zlib.define("fdopen", "fdopen");
    zlib.include(toolkit.join("third-party/zlib"));
    zlib.vendored()
        .flag_if_supported("-Wno-error=implicit-function-declaration")
        .files(&zlib_c)
        .compile("deptheos_zlib");
}

/// Adobe DNG SDK 1.7.1.
fn build_dng_sdk(target: Target, vendor: &Path) {
    let source = vendor.join("dng_sdk/source");

    // Shared helper text that is #included by the two lossless-JPEG variants
    // rather than compiled on its own.
    const EXCLUDED: &[&str] = &["dng_lossless_jpeg_shared.cpp"];

    let sources = collect_sources(&source, "cpp", false, EXCLUDED);
    assert!(!sources.is_empty(), "no DNG SDK sources in {}", source.display());

    let mut build = cc::Build::new();
    dng_common(&mut build, target, vendor);
    build.vendored().flag_if_supported("-UDEBUG").files(&sources).compile("deptheos_dng_sdk");
}

/// Settings every translation unit that includes DNG SDK headers must
/// share: C++17, the platform switches, the SDK configuration and the
/// header search paths. `qDNGThreadSafe` in particular changes `dng_host`'s
/// layout, so the SDK and the bridge must agree on it.
fn dng_common(build: &mut cc::Build, target: Target, vendor: &Path) {
    build.includes(dng_includes(vendor));
    dng_settings(build, target);
}

/// [`dng_common`]'s header search paths, in order.
fn dng_includes(vendor: &Path) -> Vec<PathBuf> {
    let toolkit = vendor.join("xmp/toolkit");
    vec![
        vendor.join("dng_sdk/source"),
        toolkit.join("public/include"),
        toolkit.join("public/include/client-glue"),
        vendor.join("libjpeg"),
        vendor.join("libjxl/libjxl/lib/include"),
        vendor.join("libjxl/client_projects/include"),
        // dng_read_image.cpp, dng_big_table.cpp and dng_image_writer.cpp do
        // `#include "zlib.h"`: resolve it to the zlib XMP is built against,
        // not whatever system copy the default search path holds.
        toolkit.join("third-party/zlib"),
    ]
}

/// [`dng_common`] without its header search paths.
fn dng_settings(build: &mut cc::Build, target: Target) {
    build.cpp(true).std("c++17");
    dng_platform_defines(build, target);
    build
        .define("qDNGUseStdInt", "1")
        .define("BIB_MULTI_THREAD", "1")
        .define("qDNGThreadSafe", "1")
        .define("qDNGUseLibJPEG", "1")
        .define("qDNGXMPDocOps", "0")
        // The SDK only initializes XMPFiles; it never reads a file through it.
        .define("qDNGXMPFiles", "0")
        .define("XMP_StaticBuild", "1");
}

/// Platform switches the DNG SDK keys off (`qMacOS` / `qWinOS` / `qiPhone` /
/// `qLinux`), with XMP's.
fn dng_platform_defines(build: &mut cc::Build, target: Target) {
    os_defines(build, target);
    match target {
        Target::Mac => build
            .define("qMacOS", "1")
            .define("qWinOS", "0")
            .define("Macintosh", "1")
            .define("NO_EXCEPT_FALSE", "noexcept"),
        Target::Ios => build
            .define("qiPhone", "1")
            .define("qMacOS", "0")
            .define("qWinOS", "0")
            .define("NO_EXCEPT_FALSE", "noexcept"),
        Target::Windows => build.define("qWinOS", "1").define("qMacOS", "0"),
        Target::Linux => build
            .define("qLinux", "1")
            .define("qMacOS", "0")
            .define("qWinOS", "0")
            .define("NO_EXCEPT_FALSE", "noexcept"),
    };
}

/// Whether the bridge's test hooks (`dng_bridge_testing.h`) are built. A
/// build script can't tell a test build apart, so every build but a release
/// one has them, and a release one with `test-support`.
fn test_hooks() -> bool {
    std::env::var_os("CARGO_FEATURE_TEST_SUPPORT").is_some()
        || std::env::var("PROFILE").as_deref() != Ok("release")
}

/// The DngBridge plus the plain-C API over it (and over libjxl).
fn build_dng_bridge(target: Target, manifest: &Path, vendor: &Path, raw_include: &Path) {
    let bridge = manifest.join("native");

    let hooks = test_hooks();
    let excluded: &[&str] = if hooks { &[] } else { &["dng_test_hooks.cpp"] };
    let sources = collect_sources(&bridge, "cpp", false, excluded);
    assert!(!sources.is_empty(), "no DngBridge sources in {}", bridge.display());

    // The version written into DNG previews (dng_write.cpp), as a C string.
    let version = format!("\"{}\"", std::env::var("CARGO_PKG_VERSION").unwrap());

    let mut build = cc::Build::new();
    // The bridge's own headers first, then the SDK's (no header name is in
    // both).
    build.include(&bridge).include(raw_include);
    dng_settings(&mut build, target);
    build
        .define("DEPTHEOS_VERSION", version.as_str())
        .define("DEPTHEOS_TEST_HOOKS", if hooks { "1" } else { "0" })
        .ours(&dng_includes(vendor))
        .files(&sources)
        .compile("deptheos_dng_bridge");

    // The directory, for its headers and for a source added or removed.
    println!("cargo:rerun-if-changed={}", bridge.display());
    println!("cargo:rerun-if-changed={}", raw_include.join("raw_c_api.h").display());
}
