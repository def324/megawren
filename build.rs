fn main() {
    // A shared Cargo target can otherwise reuse another checkout's native bridge.
    println!("cargo:rerun-if-env-changed=MEGAWREN_BUILD_ROOT");
    for file in [
        "src/bridge.rs",
        "native/client.hpp",
        "native/client.cpp",
        "native/cache.hpp",
        "native/cache.cpp",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    let sdk = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("sdklib")
        .expect("Build SDK and set PKG_CONFIG_PATH as described in README.md");
    let sodium = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("libsodium")
        .expect("libsodium is required");
    let mut build = cxx_build::bridge("src/bridge.rs");
    build
        .file("native/client.cpp")
        .file("native/cache.cpp")
        .include(".")
        .std("c++20");
    for path in sdk.include_paths.iter().chain(&sodium.include_paths) {
        build.include(path);
    }
    build.compile("megawren-bridge");
    // GNU ld discards as-needed libraries that precede the static C++ bridge.
    for library in [&sdk, &sodium] {
        for path in &library.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
        for name in &library.libs {
            println!("cargo:rustc-link-lib={name}");
        }
    }
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        for path in sdk.link_paths {
            let target = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
                .join("target");
            let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
            if path == target.join("sdk/lib") && out.ancestors().nth(4) == Some(target.as_path()) {
                let loader = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
                    "@loader_path"
                } else {
                    "$ORIGIN"
                };
                // Cargo places test harnesses one directory below the CLI.
                for relative in ["../sdk/lib", "../../sdk/lib"] {
                    println!("cargo:rustc-link-arg=-Wl,-rpath,{loader}/{relative}");
                }
            } else {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", path.display());
            }
        }
    }
}
