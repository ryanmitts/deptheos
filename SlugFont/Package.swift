// swift-tools-version: 6.2

import PackageDescription

let package = Package(
    name: "SlugFont",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "SlugFont", targets: ["SlugFont"]),
        .executable(name: "slug-font-cli", targets: ["SlugFontCLI"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser", from: "1.5.0"),
    ],
    targets: [
        .target(
            name: "SlugFont",
            resources: [
                .process("Shaders"),
                .process("Resources"),
            ]
        ),
        .executableTarget(
            name: "SlugFontCLI",
            dependencies: [
                "SlugFont",
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
            ]
        ),
    ]
)
