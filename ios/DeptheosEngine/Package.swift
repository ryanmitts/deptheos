// swift-tools-version: 6.2
// The Rust engine for the iOS app: the static library built by
// ios/build-xcframework.sh (DeptheosFFI.xcframework) and its generated Swift
// bindings (Sources/DeptheosEngine). Both are build outputs, not sources:
// rerun the script after changing crates/.
import PackageDescription

let package = Package(
    name: "DeptheosEngine",
    platforms: [.iOS(.v26)],
    products: [
        .library(name: "DeptheosEngine", targets: ["DeptheosEngine"]),
    ],
    targets: [
        .binaryTarget(name: "DeptheosFFI", path: "DeptheosFFI.xcframework"),
        .target(
            name: "DeptheosEngine",
            dependencies: ["DeptheosFFI"],
            // UniFFI's generated code predates Swift 6's strict checking.
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                // The DNG SDK, XMP and libjxl are C++.
                .linkedLibrary("c++"),
                // wgpu's Metal backend.
                .linkedFramework("Metal"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("IOSurface"),
                .linkedFramework("Foundation"),
                .linkedFramework("CoreFoundation"),
            ]
        ),
    ]
)
