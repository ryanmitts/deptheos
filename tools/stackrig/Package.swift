// swift-tools-version:5.9
// stackrig: drives a Canon EOS body (raw PTP over USB through ImageCaptureCore) and a Revopoint
// dual-axis turntable (Bluetooth LE) through "focus to the near edge, shoot a focus bracket, turn
// the table" loops. See README.md.
import PackageDescription

let package = Package(
    name: "stackrig",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "stackrig",
            path: "Sources/stackrig",
            linkerSettings: [
                .linkedFramework("ImageCaptureCore"),
                .linkedFramework("CoreBluetooth"),
            ]
        ),
        .testTarget(name: "stackrigTests", dependencies: ["stackrig"], path: "Tests/stackrigTests"),
    ]
)
