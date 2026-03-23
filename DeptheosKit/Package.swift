// swift-tools-version: 6.2
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

// ---------------------------------------------------------------
// MARK: - XMP Settings
// ---------------------------------------------------------------

let xmpHeaderSearchPaths: [CXXSetting] = [
    .headerSearchPath("."),
    .headerSearchPath("build"),
    .headerSearchPath("public/include"),
    .headerSearchPath("public/include/client-glue"),
    .headerSearchPath("XMPCommon"),
    .headerSearchPath("XMPCommon/Interfaces"),
    .headerSearchPath("XMPCommon/ImplHeaders"),
    .headerSearchPath("XMPCommon/Utilities"),
    .headerSearchPath("XMPCore"),
    .headerSearchPath("XMPCore/Interfaces"),
    .headerSearchPath("XMPCore/source"),
    .headerSearchPath("XMPCore/third-party/expat/public/lib"),
    .headerSearchPath("XMPCore/resource/mac"),
    .headerSearchPath("third-party/zuid/interfaces"),
]

let xmpCHeaderSearchPaths: [CSetting] = [
    .headerSearchPath("."),
    .headerSearchPath("build"),
    .headerSearchPath("public/include"),
    .headerSearchPath("public/include/client-glue"),
    .headerSearchPath("XMPCommon"),
    .headerSearchPath("XMPCommon/Interfaces"),
    .headerSearchPath("XMPCommon/ImplHeaders"),
    .headerSearchPath("XMPCommon/Utilities"),
    .headerSearchPath("XMPCore"),
    .headerSearchPath("XMPCore/Interfaces"),
    .headerSearchPath("XMPCore/source"),
    .headerSearchPath("XMPCore/third-party/expat/public/lib"),
    .headerSearchPath("XMPCore/resource/mac"),
    .headerSearchPath("third-party/zuid/interfaces"),
]

let xmpCxxDefines: [CXXSetting] = [
    .define("MAC_ENV", to: "1", .when(platforms: [.macOS])),
    .define("IOS_ENV", to: "1", .when(platforms: [.iOS, .visionOS])),
    .define("XMP_StaticBuild", to: "1"),
    .define("XMP_INCLUDE_XMPFILES", to: "1"),
    .define("AdobePrivate", to: "1"),
    .define("XML_STATIC"),
    .define("XML_GE", to: "0"),
    .define("HAVE_EXPAT_CONFIG_H", to: "1"),
    .define("XMP_64", to: "1"),
    .define("ENABLE_CPP_DOM_MODEL", to: "1"),
]

let xmpCDefines: [CSetting] = [
    .define("MAC_ENV", to: "1", .when(platforms: [.macOS])),
    .define("IOS_ENV", to: "1", .when(platforms: [.iOS, .visionOS])),
    .define("XMP_StaticBuild", to: "1"),
    .define("XMP_INCLUDE_XMPFILES", to: "1"),
    .define("AdobePrivate", to: "1"),
    .define("XML_STATIC"),
    .define("XML_GE", to: "0"),
    .define("HAVE_EXPAT_CONFIG_H", to: "1"),
    .define("XMP_64", to: "1"),
    .define("ENABLE_CPP_DOM_MODEL", to: "1"),
]
// ---------------------------------------------------------------
// MARK: - Vendor targets
// ---------------------------------------------------------------

let cLibJPEG: Target = .target(
    name: "CLibJPEG",
    path: "Vendor/libjpeg",
    exclude: [
        "cjpeg.c", "djpeg.c", "jpegtran.c",
        "rdjpgcom.c", "wrjpgcom.c",
        "ckconfig.c", "example.c",
        "cdjpeg.c", "rdcolmap.c", "rdswitch.c",
        "rdbmp.c", "rdgif.c", "rdppm.c", "rdrle.c", "rdtarga.c",
        "wrbmp.c", "wrgif.c", "wrppm.c", "wrrle.c", "wrtarga.c",
        "transupp.c",
        "jmemansi.c", "jmemdos.c", "jmemmac.c", "jmemname.c",
    ],
    publicHeadersPath: ".",
    cSettings: [
        .define("NO_GETENV"),
    ]
)

let cBrotli: Target = .target(
    name: "CBrotli",
    path: "Vendor/libjxl/libjxl/third_party/brotli",
    exclude: [
        "c/common/dictionary.bin",
        "c/common/dictionary.bin.br",
    ],
    sources: ["c/common", "c/dec", "c/enc"],
    publicHeadersPath: "c/include",
    cSettings: [.headerSearchPath("c/include")]
)

let cHighway: Target = .target(
    name: "CHighway",
    path: "Vendor/libjxl/libjxl/third_party/highway",
    sources: [
        "hwy/abort.cc",
        "hwy/aligned_allocator.cc",
        "hwy/nanobenchmark.cc",
        "hwy/per_target.cc",
        "hwy/print.cc",
        "hwy/stats.cc",
        "hwy/targets.cc",
        "hwy/timer.cc",
        "hwy/contrib/thread_pool/topology.cc",
    ],
    publicHeadersPath: "."
)

let cSkcms: Target = .target(
    name: "CSkcms",
    path: "Vendor/libjxl/libjxl/third_party/skcms",
    sources: ["skcms.cc", "src/skcms_TransformBaseline.cc"],
    publicHeadersPath: ".",
    cxxSettings: [
        .define("SKCMS_DISABLE_HSW"),
        .define("SKCMS_DISABLE_SKX"),
    ]
)

let cLibJXL: Target = .target(
    name: "CLibJXL",
    dependencies: ["CBrotli", "CHighway", "CSkcms"],
    path: "Vendor/libjxl/libjxl/lib",
    exclude: [
        "extras",
        "jpegli",
        "jxl/dec_transforms_testonly.cc",
        "jxl/dec_transforms_testonly.h",
        "jxl/test_memory_manager.cc",
        "jxl/test_memory_manager.h",
        "jxl/test_utils.h",
        "jxl/image_test_utils.h",
        "jxl/dct_for_test.h",
        "jxl/fake_parallel_runner_testonly.h",
        "jxl/render_pipeline/test_render_pipeline_stages.h",
        "jxl/testing.h",
        "jxl/test_image.h",
        "jxl/test_image.cc",
        "jxl/test_utils.cc",
        "threads/libjxl_threads.pc.in",
        "jxl/version.h.in",
        "jxl/libjxl.pc.in",
        "jxl/libjxl_cms.pc.in",
        "jxl/jxl.syms",
        "jxl/jxl_osx.syms",
        "jxl/jxl.version",
        "CMakeLists.txt",
        "jxl_benchmark.cmake",
        "jxl_cms.cmake",
        "jxl_extras.cmake",
        "jxl_lists.cmake",
        "jxl_tests.cmake",
        "jxl_threads.cmake",
        "jxl.cmake",
    ],
    sources: ["jxl", "threads"],
    publicHeadersPath: "include",
    cSettings: [
        .headerSearchPath("."),
        .headerSearchPath("include"),
        .headerSearchPath("../../client_projects/include"),
        .headerSearchPath(".."),
    ],
    cxxSettings: [
        .headerSearchPath("."),
        .headerSearchPath("include"),
        .headerSearchPath("../../client_projects/include"),
        .headerSearchPath(".."),
        .define("JPEGXL_ENABLE_SKCMS", to: "1"),
        .unsafeFlags(["-w"]),
    ]
)

var xmpCoreCSettings: [CSetting] = []
xmpCoreCSettings.append(contentsOf: xmpCDefines)
xmpCoreCSettings.append(contentsOf: xmpCHeaderSearchPaths)
xmpCoreCSettings.append(.define("BUILDING_XMPCORE_LIB", to: "1"))
xmpCoreCSettings.append(.define("BUILDING_XMPCORE_AS_STATIC", to: "1"))
xmpCoreCSettings.append(.define("BUILDING_XMPCOMMON_LIB", to: "1"))
xmpCoreCSettings.append(.define("BUILDING_XMPCOMMON_AS_STATIC", to: "1"))
xmpCoreCSettings.append(.define("XMP_COMPONENT_INT_NAMESPACE", to: "AdobeXMPCore_Int"))
xmpCoreCSettings.append(.unsafeFlags(["-w", "-O2"]))

var xmpCoreCxxSettings: [CXXSetting] = []
xmpCoreCxxSettings.append(contentsOf: xmpCxxDefines)
xmpCoreCxxSettings.append(contentsOf: xmpHeaderSearchPaths)
xmpCoreCxxSettings.append(.define("BUILDING_XMPCORE_LIB", to: "1"))
xmpCoreCxxSettings.append(.define("BUILDING_XMPCORE_AS_STATIC", to: "1"))
xmpCoreCxxSettings.append(.define("BUILDING_XMPCOMMON_LIB", to: "1"))
xmpCoreCxxSettings.append(.define("BUILDING_XMPCOMMON_AS_STATIC", to: "1"))
xmpCoreCxxSettings.append(.define("XMP_COMPONENT_INT_NAMESPACE", to: "AdobeXMPCore_Int"))
xmpCoreCxxSettings.append(.unsafeFlags(["-w", "-O2"]))

let cXMPCore: Target = .target(
    name: "CXMPCore",
    path: "Vendor/xmp/toolkit",
    exclude: [
        "XMPCore/third-party/expat/public/lib/xmltok_impl.c",
        "XMPCore/third-party/expat/public/lib/xmltok_ns.c",
        "XMPCore/source/Android_Utils.cpp",
        "XMPCore/source/Android_Utils.hpp",
        "XMPCore/third-party/expat/public/lib/Makefile.am",
        "XMPCore/third-party/expat/public/lib/libexpat.def",
        "XMPCore/third-party/expat/public/lib/libexpatw.def",
    ],
    sources: [
        "XMPCommon/source",
        "XMPCore/source",
        "XMPCore/third-party/expat/public/lib",
        "public/include/XMPCommon/source",
    ],
    publicHeadersPath: "public/include",
    cSettings: xmpCoreCSettings,
    cxxSettings: xmpCoreCxxSettings
)

var xmpFilesCSettings: [CSetting] = []
xmpFilesCSettings.append(contentsOf: xmpCDefines)
xmpFilesCSettings.append(contentsOf: xmpCHeaderSearchPaths)
xmpFilesCSettings.append(.headerSearchPath("source"))
xmpFilesCSettings.append(.headerSearchPath("XMPFiles"))
xmpFilesCSettings.append(.headerSearchPath("XMPFiles/source"))
xmpFilesCSettings.append(.headerSearchPath("XMPFiles/source/FormatSupport"))
xmpFilesCSettings.append(.headerSearchPath("XMPFiles/source/PluginHandler"))
xmpFilesCSettings.append(.headerSearchPath("XMPFilesPlugins/api/source"))
xmpFilesCSettings.append(.headerSearchPath("third-party/zlib"))
xmpFilesCSettings.append(.define("BUILDING_XMPFILES_LIB", to: "1"))
xmpFilesCSettings.append(.define("BUILDING_XMPFILES_AS_STATIC", to: "1"))
xmpFilesCSettings.append(.define("XMP_COMPONENT_INT_NAMESPACE", to: "AdobeXMPFiles_Int"))
xmpFilesCSettings.append(.unsafeFlags(["-w", "-Wno-error=implicit-function-declaration", "-O2"]))

var xmpFilesCxxSettings: [CXXSetting] = []
xmpFilesCxxSettings.append(contentsOf: xmpCxxDefines)
xmpFilesCxxSettings.append(contentsOf: xmpHeaderSearchPaths)
xmpFilesCxxSettings.append(.headerSearchPath("source"))
xmpFilesCxxSettings.append(.headerSearchPath("XMPFiles"))
xmpFilesCxxSettings.append(.headerSearchPath("XMPFiles/source"))
xmpFilesCxxSettings.append(.headerSearchPath("XMPFiles/source/FormatSupport"))
xmpFilesCxxSettings.append(.headerSearchPath("XMPFiles/source/PluginHandler"))
xmpFilesCxxSettings.append(.headerSearchPath("XMPFilesPlugins/api/source"))
xmpFilesCxxSettings.append(.headerSearchPath("third-party/zlib"))
xmpFilesCxxSettings.append(.define("BUILDING_XMPFILES_LIB", to: "1"))
xmpFilesCxxSettings.append(.define("BUILDING_XMPFILES_AS_STATIC", to: "1"))
xmpFilesCxxSettings.append(.define("XMP_COMPONENT_INT_NAMESPACE", to: "AdobeXMPFiles_Int"))
xmpFilesCxxSettings.append(.define("_LIBCPP_ENABLE_CXX17_REMOVED_UNARY_BINARY_FUNCTION"))
xmpFilesCxxSettings.append(.unsafeFlags(["-w", "-O2"]))

let cXMPFiles: Target = .target(
    name: "CXMPFiles",
    dependencies: ["CXMPCore"],
    path: "Vendor/xmp/toolkit",
    exclude: [
        "source/Host_IO-Win.cpp",
        "XMPFiles/source/PluginHandler/OS_Utils_WIN.cpp",
        "XMPFiles/source/PluginHandler/OS_Utils_Linux.cpp",
        "XMPFiles/source/PluginHandler/OS_Utils_Android.cpp",
        "third-party/zlib/example.c",
        "third-party/zlib/minigzip.c",
        "third-party/zlib/zconf.h.in",
        "third-party/zlib/zconf.h.cmakein",
        "third-party/zlib/zlib.3",
        "third-party/zlib/zlib.pc.in",
        "third-party/zlib/zlib2ansi",
        "third-party/zlib/algorithm.txt",
        "third-party/zlib/ChangeLog",
        "third-party/zlib/configure",
        "third-party/zlib/FAQ",
        "third-party/zlib/INDEX",
        "third-party/zlib/README",
        "third-party/zlib/ReadMe.txt",
        // gzip I/O files call read/write/close without <unistd.h> — XMP doesn't need them
        "third-party/zlib/gzclose.c",
        "third-party/zlib/gzlib.c",
        "third-party/zlib/gzread.c",
        "third-party/zlib/gzwrite.c",
    ],
    sources: [
        "XMPFiles/source",
        "source/Host_IO-POSIX.cpp",
        "source/IOUtils.cpp",
        "source/PerfUtils.cpp",
        "source/SafeStringAPIs.cpp",
        "source/UnicodeConversions.cpp",
        "source/XIO.cpp",
        "source/XML_Node.cpp",
        "source/XMP_LibUtils.cpp",
        "source/XMP_ProgressTracker.cpp",
        "source/XMPFiles_IO.cpp",
        "third-party/zlib",
        "third-party/zuid/sources/MD5.cpp",
    ],
    publicHeadersPath: "XMPFiles/source",
    cSettings: xmpFilesCSettings,
    cxxSettings: xmpFilesCxxSettings
)

let cLibRaw: Target = .target(
    name: "CLibRaw",
    path: "Vendor/libraw",
    exclude: [
        "src/Makefile",
        // Placeholder stubs for building without postprocessing — conflict with real implementations
        "src/postprocessing/postprocessing_ph.cpp",
        "src/preprocessing/preprocessing_ph.cpp",
        "src/write/write_ph.cpp",
    ],
    sources: ["src"],
    publicHeadersPath: "libraw",
    cxxSettings: [
        .headerSearchPath("."),
        .unsafeFlags(["-w"]),
    ],
    linkerSettings: [
        .linkedLibrary("c++"),
    ]
)

let cDNGSDK: Target = .target(
    name: "CDNGSDK",
    dependencies: ["CLibJPEG", "CLibJXL", "CXMPCore", "CXMPFiles"],
    path: "Vendor/dng_sdk/source",
    exclude: [
        "dng_lossless_jpeg_shared.cpp",
    ],
    publicHeadersPath: ".",
    cxxSettings: [
        .define("qMacOS", to: "1", .when(platforms: [.macOS])),
        .define("qiPhone", to: "1", .when(platforms: [.iOS, .visionOS])),
        .define("qWinOS", to: "0"),
        .define("qDNGUseStdInt", to: "1"),
        .define("Macintosh", to: "1", .when(platforms: [.macOS])),
        .define("MAC_ENV", to: "1", .when(platforms: [.macOS])),
        .define("IOS_ENV", to: "1", .when(platforms: [.iOS, .visionOS])),
        .define("BIB_MULTI_THREAD", to: "1"),
        .define("qDNGThreadSafe", to: "1"),
        .define("qDNGUseLibJPEG", to: "1"),
        .define("qDNGXMPDocOps", to: "0"),
        .define("XMP_StaticBuild", to: "1"),
        .define("NDEBUG", to: "1"),
        .headerSearchPath("../../xmp/toolkit/public/include"),
        .headerSearchPath("../../xmp/toolkit/public/include/client-glue"),
        .unsafeFlags(["-w", "-O2", "-UDEBUG"]),
    ],
    linkerSettings: [
        .linkedFramework("CoreFoundation"),
        .linkedFramework("CoreServices", .when(platforms: [.macOS])),
        .linkedFramework("ApplicationServices", .when(platforms: [.macOS])),
        .linkedLibrary("c++"),
    ]
)

let cDNGBridge: Target = .target(
    name: "CDNGBridge",
    dependencies: ["CDNGSDK"],
    path: "Sources/DngBridge",
    publicHeadersPath: "include",
    cxxSettings: [
        .define("qMacOS", to: "1", .when(platforms: [.macOS])),
        .define("qiPhone", to: "1", .when(platforms: [.iOS, .visionOS])),
        .define("qWinOS", to: "0"),
        .define("qDNGUseStdInt", to: "1"),
        .define("Macintosh", to: "1", .when(platforms: [.macOS])),
        .define("MAC_ENV", to: "1", .when(platforms: [.macOS])),
        .define("IOS_ENV", to: "1", .when(platforms: [.iOS, .visionOS])),
        .define("BIB_MULTI_THREAD", to: "1"),
        .define("qDNGUseLibJPEG", to: "1"),
        .define("qDNGXMPDocOps", to: "0"),
        .define("NDEBUG", to: "1"),
        .define("XMP_StaticBuild", to: "1"),
        .headerSearchPath("../../Vendor/dng_sdk/source"),
        .headerSearchPath("../../Vendor/libjpeg"),
        .headerSearchPath("../../Vendor/libjxl/libjxl/lib/include"),
        .headerSearchPath("../../Vendor/xmp/toolkit/public/include"),
        .headerSearchPath("../../Vendor/xmp/toolkit/public/include/client-glue"),
        .unsafeFlags(["-std=c++17"]),
    ],
    linkerSettings: [
        .linkedLibrary("c++"),
    ]
)

let package = Package(
    name: "DeptheosKit",
    platforms: [.macOS(.v26), .iOS(.v26)],
    products: [
            .library(name: "DeptheosKit", targets: ["DeptheosKit"]),
            .executable(name: "deptheos", targets: ["DeptheosCLI"]),
//            .executable(name: "deptheos-cli", targets: ["DepthosCLI"]),
//            .executable(name: "deptheos-dng-cli", targets: ["DeptheosDngCLI"]),
//            .executable(name: "deptheos-image-cli", targets: ["DeptheosImageCLI"]),
        ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser", from: "1.7.1"),
        .package(url: "https://github.com/schwa/MetalCompilerPlugin", branch: "main"),
        .package(url: "https://github.com/gohanlon/swift-memberwise-init-macro", from: "0.5.2"),
        .package(path: "../SlugFont"),
    ],
    targets: [
                cLibJPEG,
                cBrotli,
                cHighway,
                cSkcms,
                cLibJXL,
                cXMPCore,
                cXMPFiles,
                cLibRaw,
                cDNGSDK,
                cDNGBridge,
        .target(
            name: "DeptheosShaders",
            exclude: [
                "Depth.metal",
                "DngRender.metal", "ECC.metal", "FocusMapDisplay.metal",
                "Pyramid.metal", "StackingCommon.metal", "Warp.metal",
                "Wavelet.metal",
            ],
            publicHeadersPath: "include",
            plugins: [
                .plugin(name: "MetalCompilerPlugin", package: "MetalCompilerPlugin"),
            ]
        ),
        .target(
            name: "CJXLShim",
            dependencies: ["CLibJXL"],
            path: "Sources/CJXLShim",
            publicHeadersPath: "include",
            cSettings: [
                .headerSearchPath("../../Vendor/libjxl/libjxl/lib/include"),
            ]
        ),
        .target(
            name: "CAccelerateNewLAPACK",
            path: "Sources/CAccelerateNewLAPACK",
            publicHeadersPath: "include",
            cSettings: [
                .define("ACCELERATE_NEW_LAPACK"),
                .define("ACCELERATE_LAPACK_ILP64"),
            ],
            linkerSettings: [
                .linkedFramework("Accelerate"),
            ]
        ),
        .target(
            name: "CLibRawBridge",
            dependencies: ["CLibRaw"],
            path: "Sources/LibRawBridge",
            publicHeadersPath: "include",
            cxxSettings: [
                .headerSearchPath("../../Vendor/libraw"),
            ],
            linkerSettings: [
                .linkedLibrary("c++"),
            ]
        ),

        // Targets are the basic building blocks of a package, defining a module or a test suite.
        // Targets can depend on other targets in this package and products from dependencies.
        .target(
            name: "DeptheosKit",
            dependencies: [
                "CDNGBridge", "CLibRawBridge", "DeptheosShaders", "CJXLShim",
                "CAccelerateNewLAPACK",
                .product(name: "MemberwiseInit", package: "swift-memberwise-init-macro"),
                .product(name: "SlugFont", package: "SlugFont"),
            ],
            swiftSettings: [
                .interoperabilityMode(.Cxx),
                // Accelerate's module map declares ACCELERATE_NEW_LAPACK / ILP64 as
                // config_macros — each consumer TU selects its own variant. The
                // CAccelerateNewLAPACK C shim sets these for SourceKit's benefit
                // (reliable index → editor sees __LAPACK_int), but Swift's own
                // clang importer needs them here at compile time too.
                .unsafeFlags([
                    "-Xcc", "-DACCELERATE_NEW_LAPACK",
                    "-Xcc", "-DACCELERATE_LAPACK_ILP64",
                ]),
            ]
        ),
        .executableTarget(
            name: "DeptheosCLI",
            dependencies: [
                "DeptheosKit",
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
            ],
            swiftSettings: [.interoperabilityMode(.Cxx)]
        ),
        .testTarget(
            name: "DeptheosKitTests",
            dependencies: ["DeptheosKit"],
            resources: [.copy("Resources")],
            swiftSettings: [.interoperabilityMode(.Cxx)]
        ),
    ],
    cxxLanguageStandard: .cxx17
)
