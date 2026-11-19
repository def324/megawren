# Megawren - MEGA terminal client (tui) for macOS and Linux

Megawren is a MEGA terminal client for MEGA on macOS 15 or later and Linux. It uses the official MEGA SDK and runs without a background service.

## Features

- Sign in with two-factor authentication. Save your login with an optional local passphrase.
- Upload and download files or folders while browsing. Track and cancel transfers in the TUI.
- Rename, move, or copy selected items. Restore files and folders from the Rubbish Bin.
- Preview text files and images on request.
- Use `ls`, `get`, and `put` in scripts. `ls --json` gives a machine-readable listing.

[![Signing in with a six-digit authenticator code, uploading Coast walk.mp4 with progress in the sidebar, and downloading the video to the local Downloads folder.](docs/media/megawren.gif)](docs/media/megawren.gif?raw=true)

## Install

```sh
brew install def324/tap/megawren
```

Prebuilt archives for Apple Silicon, Intel Macs and x86-64 Linux are available on the [releases page](https://github.com/def324/megawren/releases/latest). Extract the archive and keep `bin/` and `lib/` together. The Linux archive lists its required system libraries in `RUNTIME.txt`.

## Open your drive

Run `megawren` to open the browser, or `./bin/megawren` from an unpacked archive.

Sign in with your MEGA email, password, and authenticator code if you use one. Choose **Yes, save login** to stay signed in on this computer. Set a local passphrase, or leave it blank and confirm **Save anyway** to open the drive automatically. Without a passphrase, anyone who can read your saved login can access your MEGA account.

The first visit can take a while on a large drive. Remembering your login also saves the file list so it opens faster next time.

## Find and transfer files

Use ↑ and ↓ to move, Enter to open a folder, and ← to go back. Press **i** to see a folder’s details and actions without opening it. Enter on a file opens the same panel. To work on several items at once, mark each with Space, then press Enter.

| To | In the browser |
| --- | --- |
| Download | Focus a file or mark several. Press **d**, open the local folder where you want them, then press **d** again. |
| Upload | Open the destination on MEGA. Press **u**, mark local files or folders with Space, then press **u** again. |
| Preview | Press **p** on a file or run `megawren preview /path/to/file`. **Esc** closes the viewer. |
| Check a transfer | Press **t** for filenames and progress. Use ↑ and ↓ to see each destination. **x** cancels the selected transfer (the whole batch for a multi-file selection). **Shift+X** cancels all. Both ask first. |
| Recover an item | Press **2** for the Rubbish Bin. Mark the item, press Enter, and choose **Restore to original folder**. Press **1** to return to your drive. |

Previews load only when opened. For supported text files up to 1 MiB, the viewer reads the first 128 KiB or less. JPEG, PNG, WebP, and GIF images have a 100 MiB file limit.

Press **Delete** to move a selection to the Rubbish Bin. You’ll be asked to confirm. **?** opens or closes the shortcut list. If file icons show up as squares, press **F3** in the browser or a file picker to switch to plain icons. Press **q** to quit.

## From the shell

```sh
megawren ls /Photos
megawren get /Photos/trip.jpg
megawren get /Photos/a.jpg /Photos/b.jpg ./pictures
megawren put ./reports/*.csv /Reports
```

With one path, `get` downloads here and `put` uploads to `/`. With two or more paths, the last one is the destination directory. It must already exist. Files and folders can travel together in one command.

Downloads keep existing files unless you pass `--overwrite`. Uploads stop on a conflicting filename. If a batch fails, completed files stay in place and the command exits with an error. For a public MEGA link, run `megawren get`, paste the link at the hidden prompt, then choose a download directory. Press Enter to use the current directory.

**From a script**, run `megawren login` once in a terminal. If you saved a passphrase, have your secret manager supply it on stdin using `--unlock-stdin`. This example receives it on file descriptor 3. With a passphrase-free login, just run `megawren ls --json /Reports`.

```sh
megawren --unlock-stdin ls --json /Reports <&3 >reports.json
```

`ls --json` writes an array of objects with `name`, `type` (`file` or `dir`), and `size` in bytes. Folders and unknown sizes use `null`. Pipes, redirects, and JSON output turn off progress animations. Errors and storage notices go to stderr. Shell commands exit with 0 on success, 1 on failure (including partial transfers), 2 for invalid syntax, or 130 on Ctrl-C.

## What stays on disk?

Run `megawren status` to see the paths on your machine. The files below live in `~/.megawren/`.

| Path | Contents |
| --- | --- |
| `session.enc` | Your saved MEGA session. A local passphrase protects it if you set one. |
| `cache.enc` | Your file index, compressed and encrypted. Browsing, previews, and downloads reuse it for up to a day. Cloud edits, uploads, or a full reload can update it sooner. |
| `state.lock` and `run-*/` | The account lock and temporary SDK databases and transfer data. A normal exit removes the working directory. |

A download to another filesystem briefly creates a `.megawren-part-*` file in the destination. A forced exit can leave it behind.

Forgot the local passphrase? Press **F2** at the unlock screen to sign in again. End the old session in MEGA’s session settings. To sign out normally and remove the saved login and index, run `megawren logout`.

## Build from source

Use rustup and a C++20 compiler. The checkout selects its [Rust toolchain](rust-toolchain.toml). Install the native dependencies for your platform.

```sh
# macOS with Homebrew
brew install cmake pkgconf cryptopp libsodium sqlite curl icu4c

# Ubuntu or Debian
sudo apt install build-essential cmake pkg-config libcrypto++-dev libsodium-dev libsqlite3-dev libcurl4-openssl-dev libicu-dev libssl-dev
```

Build the SDK and launch Megawren from the checkout.

```sh
git submodule update --init --recursive
./scripts/build-sdk.sh
./scripts/cargo.sh build --release --locked
./target/release/megawren
```

Try an [upload and download with disposable files](docs/manual-testing.md). For tests, a guide to the source, and instructions for rendering the README tour, see [Working on Megawren](docs/development.md).

## License

Megawren is released under the [MIT License](LICENSE). The MEGA SDK and other dependencies have their own licenses. [Third-party notices](THIRD_PARTY_NOTICES.md)

Megawren is an independent project unaffiliated with Mega Privacy LLC, the operator of MEGA Cloud.
