// Builds the vendored RandomX C++ library via cmake so that `cargo build`
// produces a self-contained binary from a fresh clone. The tree lives under
// `vendor/randomx/` and is compiled Release.

use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let randomx = cmake::Config::new("vendor/randomx")
        .define("CMAKE_BUILD_TYPE", "Release")
        .out_dir(out_dir.join("randomx"))
        .build_target("randomx")
        .build();
    println!(
        "cargo:rustc-link-search=native={}/build",
        randomx.display()
    );
    println!("cargo:rustc-link-lib=static=randomx");

    println!("cargo:rustc-link-lib=c++");

    println!("cargo:rerun-if-changed=vendor/randomx/src");
    println!("cargo:rerun-if-changed=build.rs");
}
