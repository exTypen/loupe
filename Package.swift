// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "Loupe",
    platforms: [.macOS(.v15)],
    targets: [
        .executableTarget(
            name: "Loupe",
            path: "Sources/Loupe",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(name: "LoupeTests", dependencies: ["Loupe"], path: "Tests/LoupeTests"),
    ]
)
