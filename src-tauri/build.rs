use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=native/apple_speech_helper.swift");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
        let helper = out_dir.join("apple-speech-helper");
        let module_cache = out_dir.join("swift-module-cache");
        std::fs::create_dir_all(&module_cache).expect("create Swift module cache");
        let target = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("aarch64") => "arm64-apple-macosx12.0",
            Ok("x86_64") => "x86_64-apple-macosx12.0",
            _ => panic!("unsupported macOS target architecture"),
        };
        let status = Command::new("xcrun")
            .args([
                "swiftc",
                "-O",
                "-parse-as-library",
                "-target",
                target,
                "-module-cache-path",
            ])
            .arg(&module_cache)
            .args(["native/apple_speech_helper.swift", "-o"])
            .arg(&helper)
            .status()
            .expect("failed to run xcrun swiftc; install/select Xcode first");
        assert!(status.success(), "failed to compile Apple Speech helper");
        println!(
            "cargo:rustc-env=APPLE_SPEECH_HELPER_PATH={}",
            helper.display()
        );
    }

    tauri_build::build()
}
