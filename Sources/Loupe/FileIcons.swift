import SwiftUI
import AppKit

/// VS Code–style file-type icons (Material Icon Theme SVGs, rendered natively by NSImage).
enum FileIcons {
    private static let byName: [String: String] = [
        "cargo.toml": "rust", "cargo.lock": "rust", "flake.nix": "nix", "flake.lock": "nix", "default.nix": "nix", "shell.nix": "nix",
        "dockerfile": "docker", "docker-compose.yml": "docker", "docker-compose.yaml": "docker", "compose.yaml": "docker",
        "package.json": "nodejs", "package-lock.json": "npm", ".npmrc": "npm", "pnpm-lock.yaml": "pnpm", "pnpm-workspace.yaml": "pnpm",
        "yarn.lock": "yarn", "bun.lockb": "bun", "bun.lock": "bun", "tsconfig.json": "tsconfig", "makefile": "makefile", "justfile": "makefile",
        ".gitignore": "git", ".gitattributes": "git", ".gitmodules": "git", "readme.md": "readme", "readme": "readme",
        "license": "license", "license.md": "license", "license.txt": "license", ".editorconfig": "editorconfig",
        ".prettierrc": "prettier", ".eslintrc": "eslint", "eslint.config.js": "eslint", "eslint.config.mjs": "eslint",
        "babel.config.js": "babel", ".babelrc": "babel", "tailwind.config.js": "tailwindcss", "tailwind.config.ts": "tailwindcss",
        "webpack.config.js": "webpack", "vite.config.ts": "vite", "vite.config.js": "vite", "nginx.conf": "nginx", ".envrc": "tune",
        "chart.yaml": "helm", "go.mod": "go", "go.sum": "go",
    ]
    private static let byExt: [String: String] = [
        "rs": "rust", "nix": "nix", "ts": "typescript", "mts": "typescript", "cts": "typescript", "tsx": "react_ts",
        "js": "javascript", "mjs": "javascript", "cjs": "javascript", "jsx": "react", "json": "json", "jsonc": "json", "json5": "json",
        "md": "markdown", "mdx": "markdown", "yml": "yaml", "yaml": "yaml", "toml": "toml", "sql": "database", "go": "go",
        "py": "python", "pyi": "python", "swift": "swift", "html": "html", "htm": "html", "css": "css", "scss": "sass", "sass": "sass",
        "sh": "console", "bash": "console", "zsh": "console", "fish": "console", "lock": "lock", "png": "image", "jpg": "image",
        "jpeg": "image", "gif": "image", "webp": "image", "ico": "image", "heic": "image", "svg": "svg", "feature": "cucumber",
        "proto": "proto", "graphql": "graphql", "gql": "graphql", "vue": "vue", "svelte": "svelte", "kt": "kotlin", "kts": "kotlin",
        "java": "java", "c": "c", "cc": "cpp", "cpp": "cpp", "cxx": "cpp", "hpp": "cpp", "h": "h", "sol": "solidity", "tf": "terraform",
        "tfvars": "terraform", "xml": "xml", "plist": "xml", "csv": "table", "tsv": "table", "pdf": "pdf", "zip": "zip", "gz": "zip",
        "tar": "zip", "env": "tune", "rb": "ruby", "php": "php", "lua": "lua", "ex": "elixir", "exs": "elixir", "hs": "haskell",
        "zig": "zig", "dart": "dart", "cs": "csharp", "pem": "certificate", "crt": "certificate", "key": "key", "log": "log",
        "ttf": "font", "otf": "font", "woff": "font", "woff2": "font", "mp4": "video", "mov": "video", "webm": "video",
        "mp3": "audio", "wav": "audio", "ipynb": "jupyter", "conf": "settings", "ini": "settings", "cfg": "settings", "txt": "file",
    ]

    static func iconName(for path: String) -> String {
        let file = (path as NSString).lastPathComponent.lowercased()
        if let n = byName[file] { return n }
        if file.hasPrefix(".env") { return "tune" }
        if file.hasPrefix("dockerfile") { return "docker" }
        let ext = (file as NSString).pathExtension
        // foo.test.ts / foo.spec.tsx get the test variant.
        if [".test.", ".spec."].contains(where: file.contains) {
            if ext == "ts" { return "test-ts" }
            if ext == "tsx" || ext == "jsx" { return "test-jsx" }
            if ext == "js" { return "test-js" }
        }
        if path.contains("/k8s/") || path.contains("/kubernetes/"), ext == "yaml" || ext == "yml" { return "kubernetes" }
        return byExt[ext] ?? "file"
    }

    @MainActor private static var cache: [String: NSImage] = [:]

    @MainActor static func image(for path: String) -> NSImage {
        let name = iconName(for: path)
        if let img = cache[name] { return img }
        let img = FileIconData.svg[name].flatMap { NSImage(data: Data($0.utf8)) } ?? NSImage(systemSymbolName: "doc", accessibilityDescription: nil)!
        cache[name] = img
        return img
    }
}

struct FileIcon: View {
    var path: String
    var size: CGFloat = 16
    var body: some View {
        Image(nsImage: FileIcons.image(for: path)).resizable().interpolation(.high).frame(width: size, height: size)
    }
}

/// GitHub Desktop–style change marker: a small outlined square on the trailing edge.
struct StatusIcon: View {
    var kind: FileChange.Kind
    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(color)
            .help(label)
    }
    private var symbol: String {
        switch kind {
        case .added, .untracked: "plus.square"
        case .deleted: "minus.square"
        case .renamed, .copied: "arrow.right.square"
        case .conflicted: "exclamationmark.square"
        default: "dot.square"
        }
    }
    private var color: Color {
        switch kind {
        case .added, .untracked: .green
        case .deleted: .red
        case .renamed, .copied: .blue
        case .conflicted: .orange
        default: .yellow
        }
    }
    private var label: String {
        switch kind {
        case .added: "Added"
        case .untracked: "Untracked"
        case .deleted: "Deleted"
        case .renamed: "Renamed"
        case .copied: "Copied"
        case .conflicted: "Conflicted"
        case .typeChanged: "Type changed"
        default: "Modified"
        }
    }
}
