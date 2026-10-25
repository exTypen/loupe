// swift-tools-version:6.0
import PackageDescription

// Tree-sitter grammars for code navigation in pull requests (go to definition, outline). Pinned: some releases'
// manifests drop their external scanner (they test for src/scanner.c relative to the wrong directory), and
// tree-sitter-swift only ships generated sources on its "-with-generated-files" tags.
let grammars: [(url: String, version: String, product: String, package: String)] = [
    ("https://github.com/tree-sitter/tree-sitter-rust", "0.24.2", "TreeSitterRust", "tree-sitter-rust"),
    ("https://github.com/alex-pinkus/tree-sitter-swift", "0.7.4-with-generated-files", "TreeSitterSwift", "tree-sitter-swift"),
    ("https://github.com/tree-sitter/tree-sitter-go", "0.25.0", "TreeSitterGo", "tree-sitter-go"),
    ("https://github.com/tree-sitter/tree-sitter-typescript", "0.23.2", "TreeSitterTypeScript", "tree-sitter-typescript"),
    ("https://github.com/tree-sitter/tree-sitter-javascript", "0.23.1", "TreeSitterJavaScript", "tree-sitter-javascript"),
    ("https://github.com/tree-sitter/tree-sitter-python", "0.23.1", "TreeSitterPython", "tree-sitter-python"),
    ("https://github.com/tree-sitter/tree-sitter-c", "0.24.2", "TreeSitterC", "tree-sitter-c"),
    ("https://github.com/tree-sitter/tree-sitter-cpp", "0.23.4", "TreeSitterCPP", "tree-sitter-cpp"),
    ("https://github.com/tree-sitter/tree-sitter-java", "0.23.5", "TreeSitterJava", "tree-sitter-java"),
]

let package = Package(
    name: "Loupe",
    platforms: [.macOS(.v15)],
    dependencies: [
        // The grammars depend on this URL (it redirects to tree-sitter/swift-tree-sitter); one identity avoids a clash.
        .package(url: "https://github.com/ChimeHQ/SwiftTreeSitter", from: "0.25.0"),
    ] + grammars.map { g in
        g.version.contains("-") ? .package(url: g.url, revision: g.version) : .package(url: g.url, exact: Version(g.version)!)
    },
    targets: [
        .executableTarget(
            name: "Loupe",
            dependencies: [.product(name: "SwiftTreeSitter", package: "SwiftTreeSitter")]
                + grammars.map { .product(name: $0.product, package: $0.package) },
            path: "Sources/Loupe",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(name: "LoupeTests", dependencies: ["Loupe"], path: "Tests/LoupeTests"),
    ]
)
