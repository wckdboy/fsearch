// swift-tools-version: 5.9
import Foundation
import PackageDescription

// Replaced by the tag workflow when an `ios-v*` release is published.
let remoteURL = "https://github.com/wckdboy/fsearch/releases/download/ios-v0.1.0/FSearchFFI.xcframework.zip"
let remoteChecksum = "0000000000000000000000000000000000000000000000000000000000000000"

let packageRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let bundledFramework = packageRoot + "/FSearchFFI.xcframework"
let envFramework = ProcessInfo.processInfo.environment["FSEARCH_XCFRAMEWORK"]

func ffiTarget() -> Target {
    if let envFramework, FileManager.default.fileExists(atPath: envFramework) {
        return .binaryTarget(name: "FSearchFFI", path: envFramework)
    }
    if FileManager.default.fileExists(atPath: bundledFramework) {
        return .binaryTarget(name: "FSearchFFI", path: "FSearchFFI.xcframework")
    }
    return .binaryTarget(name: "FSearchFFI", url: remoteURL, checksum: remoteChecksum)
}

let package = Package(
    name: "FSearchKit",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        .library(name: "FSearchKit", targets: ["FSearchKit"]),
    ],
    targets: [
        ffiTarget(),
        .target(
            name: "FSearchFFIBindings",
            dependencies: ["FSearchFFI"],
            path: "Sources/FSearchFFIBindings",
            linkerSettings: [
                .linkedLibrary("c++"),
                .linkedLibrary("iconv"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
        .target(
            name: "FSearchKit",
            dependencies: ["FSearchFFIBindings"],
            path: "Sources/FSearchKit"
        ),
    ]
)
