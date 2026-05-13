# sas-linter (Rust)

Single-binary port of the [Ruby `sas-linter` gem](../README.md).

Same rules, same YAML config, same exit codes — no Ruby or RubyGems required.

```sh
cargo build --release
./target/release/sas-lint --list-rules
./target/release/sas-lint path/to/file.sas
```

Designed to be spun out into a standalone repo. The `Cargo.toml` and
sources here are self-contained — copy this directory plus `spec/fixtures`
elsewhere and it builds without any reference to the parent tree.
