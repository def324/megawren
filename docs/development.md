# Working on Megawren

Build the native SDK once per checkout using the [build instructions](../README.md#build-from-source). Run Cargo through `./scripts/cargo.sh` so it finds the SDK under `target/sdk/`.

```sh
cargo fmt --check
./scripts/cargo.sh test --locked
./scripts/cargo.sh clippy --all-targets --locked -- -D warnings
```

The default suite runs without a MEGA account. To exercise a real upload and download, use the [manual transfer test](manual-testing.md).

## Where to make a change

| Task | Start here |
| --- | --- |
| Change a command or its output | [Command definitions](../src/main.rs), [path handling](../src/cli.rs), [listing and JSON output](../src/display.rs) |
| Change a key, picker, or dialog | [Browser input](../src/tui.rs), [local picker](../src/tui/picker.rs), [cloud actions](../src/tui/actions.rs), [widgets and help text](../src/tui/view.rs) |
| Change login, unlock, or shutdown | [Terminal lifecycle](../src/tui/lifecycle.rs), [account and SDK lifecycle](../src/backend.rs) |
| Add a preview format | [File types, byte limits, and decoding](../src/preview.rs), [preview requests and cancellation](../src/tui/worker/previews.rs) |
| Add an SDK operation | [Rust/C++ bridge](../src/bridge.rs), [C++ SDK adapter](../native/client.cpp) |

## Transfers and cloud changes

Both the CLI and TUI use the same transfer worker. In the TUI, keypresses send commands to the worker and the browser draws its latest snapshot. A slow SDK request can run while the user browses. Worker tests use a fake implementation of the `Remote` trait to exercise collisions, cancellation, and partial failures. [Transfer worker](../src/tui/worker.rs)

An upload reads from a private snapshot of the selected file. A download lands in staging, where Megawren checks its length before publishing it to the destination. Across filesystems, the last step uses a temporary file in the destination directory so an interrupted copy cannot expose a half-written file under the final name. A batch can finish some files before another fails. [Local transfer I/O](../src/transfer.rs)

Before moving a folder to the Rubbish Bin, the worker checks its descendants for shared items and public links. It records the SDK’s tree generation and checks it again when the user confirms. A tree change invalidates the review. [Removal review and submission](../src/tui/worker/manage.rs)

## Saved state

A passphrase-protected login uses the original session vault format. A login saved without a passphrase uses a distinct authenticated header and opens automatically with an empty passphrase. Its protection depends on the private file permissions. The session supplies the key for the encrypted file index. The SDK opens its databases in a private per-run directory. On exit, Megawren closes the SDK before saving a replacement index. A read-only visit can reuse the existing snapshot. [Session vault](../src/vault.rs) · [Index format](../src/cache.rs) · [Save decision](../src/backend.rs)

## Screenshots and packages

- To inspect a layout, run `./scripts/cargo.sh test --locked tui::preview::export_visual_fixtures -- --ignored`. Open the SVGs in `target/tui-preview/`.
- To update the README animation, edit the [scripted tour](../src/tui/preview/demo.rs) and run `./scripts/demo.py` with uv and asciinema agg 1.9 or newer installed. The script renders the widgets with fictional files and typesets the captions below the terminal.
- To package a release build for the current machine, run `python3 scripts/package.py` after `./scripts/cargo.sh build --release --locked`. Packaging needs Python 3.11 or newer and, on Linux, `patchelf`. The archive and its SHA-256 checksum go in `target/dist/`. Build macOS archives on Sequoia (15). The packager checks the minimum OS of each bundled library. CI builds and runs each archive on its target platform.
- The Linux release archives are statically linked against musl. `scripts/build-static-linux.sh` builds one inside an Alpine Linux 3.24 container for the container's CPU: `docker run --rm -v "$PWD:/work/megawren" -w /work/megawren alpine:3.24 sh scripts/build-static-linux.sh`. It builds Crypto++ and a minimal curl from checksum-verified sources, embeds ICU's English data, and runs the tests before packaging. musl lacks `pthread_cond_clockwait`, so [native/musl_clockwait.h](../native/musl_clockwait.h) supplies it to keep the SDK's untimed waits from spinning.

## Publishing a release

Tag the tested commit with `v` and the version from `Cargo.toml`. Publish the two macOS archives and the two static Linux archives (`linux-musl`), with their adjacent `.sha256` files, from that commit’s successful CI run in GitHub Releases. The Ubuntu archive is a CI check and is not published.

Update the tagged source URL and SHA-256 in the [Homebrew formula](https://github.com/def324/homebrew-tap/blob/main/Formula/megawren.rb). When the SDK changes, update the formula’s SDK resource to the commit reported by `git rev-parse HEAD:vendor/mega` and verify its archive checksum. Open a tap pull request so Homebrew tests the installation before the update ships.
