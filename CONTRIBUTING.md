# Contributing to Megawren

## Report a bug or suggest a feature

Open an [issue](https://github.com/def324/megawren/issues). Include the output of `megawren --version`, your operating system, and the steps that lead to the problem. Report security problems privately instead, as described in the [security policy](SECURITY.md).

## Propose a change

1. Fork the repository and create a branch.
2. Make one focused change, with tests (see below).
3. Open a pull request against `main`. Say what changes for people using Megawren, and why.

CI must pass before a change is merged. It checks formatting, runs the tests and Clippy on macOS and Linux, builds the static Linux binaries and packages, and scans the result for known vulnerabilities.

## What a change needs

- **Tests.** New behavior and bug fixes come with automated tests in the same pull request. Tests must run without a MEGA account, like the existing suite.
- **Clean checks.** Run `cargo fmt`, and keep `./scripts/cargo.sh clippy --all-targets --locked -- -D warnings` free of warnings.
- **Fuzz targets.** Code in `src/text.rs` and `src/vault.rs` handles untrusted input. Keep its [fuzz targets](docs/development.md#fuzzing) passing, and add a target when you add a parser.
- **Few, permissive dependencies.** Add a crate only when it's needed. `deny.toml` lists the licenses CI accepts.
- **Docs.** Update the README when something people see or type changes.

[Working on Megawren](docs/development.md) explains how to build the SDK and run the tests.
