//! Keeps every place that names a release version in step with the source
//! of truth, so a version bump can't leave stale install instructions or a
//! VSCode extension that downloads the previous binary.
//!
//! Sources of truth:
//! * the linter version — `Cargo.toml` (`CARGO_PKG_VERSION`);
//! * the extension version — `editors/vscode/package.json`.

use std::path::{Path, PathBuf};

const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-pc-windows-msvc",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn json_version(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // JSON is a subset of YAML, so the YAML parser already in dev-deps does.
    let v: serde_yaml::Value = serde_yaml::from_str(&text).expect("valid JSON");
    v["version"]
        .as_str()
        .unwrap_or_else(|| panic!("{} has no top-level version", path.display()))
        .to_string()
}

fn linter_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn extension_version() -> String {
    json_version(&root().join("editors/vscode/package.json"))
}

#[test]
fn readme_install_commands_name_the_current_binary_release() {
    let readme = read("README.md");
    let tag = format!("v{}", linter_version());
    let mut missing = Vec::new();
    for target in TARGETS {
        let ext = if target.contains("windows") {
            ".exe"
        } else {
            ""
        };
        let url = format!(
            "https://github.com/mes-amis/sas-linter-rs/releases/download/{tag}/sas-lint-{tag}-{target}{ext}"
        );
        if !readme.contains(&url) {
            missing.push(url);
        }
    }
    assert!(
        missing.is_empty(),
        "README.md install commands must link the current release ({tag}); missing:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn readme_install_commands_have_no_placeholders() {
    let readme = read("README.md");
    for placeholder in ["<TAG>", "<TARGET>", "<VERSION>"] {
        assert!(
            !readme.contains(placeholder),
            "README.md still has a {placeholder} placeholder — spell out the current version instead"
        );
    }
}

#[test]
fn readme_names_the_current_vscode_extension_release() {
    let readme = read("README.md");
    let ver = extension_version();
    let url = format!(
        "https://github.com/mes-amis/sas-linter-rs/releases/download/vscode-v{ver}/sas-linter-vscode-{ver}.vsix"
    );
    assert!(
        readme.contains(&url),
        "README.md must link the current extension release: {url}"
    );
}

#[test]
fn vscode_extension_pins_the_current_binary_release() {
    // A fresh extension install downloads whatever this pin names; an older
    // binary rejects configs that mention a newer rule.
    let binary_ts = read("editors/vscode/src/binary.ts");
    let expected = format!("const DEFAULT_BINARY_VERSION = \"v{}\";", linter_version());
    assert!(
        binary_ts.contains(&expected),
        "editors/vscode/src/binary.ts must pin the current binary: {expected}"
    );
}

#[test]
fn vscode_lockfile_matches_package_version() {
    let ver = extension_version();
    let lock = read("editors/vscode/package-lock.json");
    let v: serde_yaml::Value = serde_yaml::from_str(&lock).expect("valid JSON");
    assert_eq!(
        v["version"].as_str(),
        Some(ver.as_str()),
        "package-lock.json top-level version"
    );
    assert_eq!(
        v["packages"][""]["version"].as_str(),
        Some(ver.as_str()),
        "package-lock.json root package version"
    );
}
