//! VS Code–style file-type icons: Material Icon Theme SVGs, embedded. They're multi-coloured, so draw them with
//! gpui's `img` (an `Image` of format Svg from `svg(name)`), not `svg()`, which paints a single-colour mask.

#[path = "icon_data.rs"]
mod data;

fn by_name(file: &str) -> Option<&'static str> {
    Some(match file {
        "cargo.toml" | "cargo.lock" => "rust",
        "flake.nix" | "flake.lock" | "default.nix" | "shell.nix" => "nix",
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" | "compose.yaml" => "docker",
        "package.json" => "nodejs",
        "package-lock.json" | ".npmrc" => "npm",
        "pnpm-lock.yaml" | "pnpm-workspace.yaml" => "pnpm",
        "yarn.lock" => "yarn",
        "bun.lockb" | "bun.lock" => "bun",
        "tsconfig.json" => "tsconfig",
        "makefile" | "justfile" => "makefile",
        ".gitignore" | ".gitattributes" | ".gitmodules" => "git",
        "readme.md" | "readme" => "readme",
        "license" | "license.md" | "license.txt" => "license",
        ".editorconfig" => "editorconfig",
        ".prettierrc" => "prettier",
        ".eslintrc" | "eslint.config.js" | "eslint.config.mjs" => "eslint",
        "babel.config.js" | ".babelrc" => "babel",
        "tailwind.config.js" | "tailwind.config.ts" => "tailwindcss",
        "webpack.config.js" => "webpack",
        "vite.config.ts" | "vite.config.js" => "vite",
        "nginx.conf" => "nginx",
        ".envrc" => "tune",
        "chart.yaml" => "helm",
        "go.mod" | "go.sum" => "go",
        _ => return None,
    })
}

fn by_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rust",
        "nix" => "nix",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "react_ts",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "react",
        "json" | "jsonc" | "json5" => "json",
        "md" | "mdx" => "markdown",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "sql" => "database",
        "go" => "go",
        "py" | "pyi" => "python",
        "swift" => "swift",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "sass",
        "sh" | "bash" | "zsh" | "fish" => "console",
        "lock" => "lock",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "heic" => "image",
        "svg" => "svg",
        "feature" => "cucumber",
        "proto" => "proto",
        "graphql" | "gql" => "graphql",
        "vue" => "vue",
        "svelte" => "svelte",
        "kt" | "kts" => "kotlin",
        "java" => "java",
        "c" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "h" => "h",
        "sol" => "solidity",
        "tf" | "tfvars" => "terraform",
        "xml" | "plist" => "xml",
        "csv" | "tsv" => "table",
        "pdf" => "pdf",
        "zip" | "gz" | "tar" => "zip",
        "env" => "tune",
        "rb" => "ruby",
        "php" => "php",
        "lua" => "lua",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "zig" => "zig",
        "dart" => "dart",
        "cs" => "csharp",
        "pem" | "crt" => "certificate",
        "key" => "key",
        "log" => "log",
        "ttf" | "otf" | "woff" | "woff2" => "font",
        "mp4" | "mov" | "webm" => "video",
        "mp3" | "wav" => "audio",
        "ipynb" => "jupyter",
        "conf" | "ini" | "cfg" => "settings",
        "txt" => "file",
        _ => return None,
    })
}

/// The icon for a path: by file name, then a few name patterns, then by extension; "file" when nothing matches.
pub fn icon_name(path: &str) -> &'static str {
    let file = crate::file_name(path).to_lowercase();
    if let Some(n) = by_name(&file) {
        return n;
    }
    if file.starts_with(".env") {
        return "tune";
    }
    if file.starts_with("dockerfile") {
        return "docker";
    }
    let ext = crate::extension(&file);
    // foo.test.ts / foo.spec.tsx get the test variant.
    if file.contains(".test.") || file.contains(".spec.") {
        match ext {
            "ts" => return "test-ts",
            "tsx" | "jsx" => return "test-jsx",
            "js" => return "test-js",
            _ => {}
        }
    }
    if (path.contains("/k8s/") || path.contains("/kubernetes/")) && (ext == "yaml" || ext == "yml") {
        return "kubernetes";
    }
    by_ext(ext).unwrap_or("file")
}

/// An icon's SVG source.
pub fn svg(name: &str) -> Option<&'static str> {
    data::SVG.binary_search_by(|(n, _)| n.cmp(&name)).ok().map(|i| data::SVG[i].1)
}

/// The SVG for a path's icon (the plain file icon when its own is missing).
pub fn svg_for(path: &str) -> &'static str {
    svg(icon_name(path)).or_else(|| svg("file")).unwrap()
}

/// Every embedded icon, by name.
pub fn all() -> &'static [(&'static str, &'static str)] {
    data::SVG
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_file_icons() {
        assert_eq!(icon_name("central_api/src/lib.rs"), "rust");
        assert!(icon_name("flake.nix") == "nix" && icon_name("flake.lock") == "nix");
        assert_eq!(icon_name("user_console/src/Panel.test.tsx"), "test-jsx");
        assert_eq!(icon_name("web/Panel.tsx"), "react_ts");
        assert!(icon_name("Cargo.toml") == "rust" && icon_name(".env.local") == "tune");
        assert_eq!(icon_name("data/migrations/x/up.sql"), "database");
        assert_eq!(icon_name("deploy/k8s/app.yaml"), "kubernetes");
        assert_eq!(icon_name("weird.unknownext"), "file");
        // Every mapped name must exist in the embedded icon data.
        for p in ["a.rs", "a.nix", "a.ts", "a.tsx", "a.md", "a.yaml", "a.sql", "a.feature", "Dockerfile", "a.png", "a.sh", "a.json"] {
            assert!(svg(icon_name(p)).is_some(), "{p}");
        }
        let every = (by_name_all().into_iter()).chain(["tune", "docker", "test-ts", "test-jsx", "test-js", "kubernetes", "file"]);
        for n in every {
            assert!(svg(n).is_some_and(|s| s.starts_with("<svg")), "{n}");
        }
    }

    fn by_name_all() -> Vec<&'static str> {
        let exts = "rs nix ts tsx js jsx json md yml toml sql go py swift html css scss sh lock png svg feature proto graphql vue svelte kt java c cc h sol tf xml csv pdf zip env rb php lua ex hs zig dart cs pem key log ttf mp4 mp3 ipynb conf txt";
        let names = "cargo.toml flake.nix dockerfile package.json package-lock.json pnpm-lock.yaml yarn.lock bun.lock tsconfig.json makefile .gitignore readme license .editorconfig .prettierrc .eslintrc .babelrc tailwind.config.js webpack.config.js vite.config.ts nginx.conf .envrc chart.yaml go.mod";
        exts.split(' ').filter_map(by_ext).chain(names.split(' ').filter_map(by_name)).collect()
    }
}
