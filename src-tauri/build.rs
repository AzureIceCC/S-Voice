use std::env;
use std::path::PathBuf;
use std::process::Command;

fn sdk_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

fn main() {
    println!("cargo:rerun-if-changed=native/apple_speech_helper.swift");
    println!("cargo:rerun-if-changed=native/apple_speech_helper_stub.swift");

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
        let swift_args = |source: &str| {
            let mut command = Command::new("xcrun");
            command
                .args([
                    "swiftc",
                    "-O",
                    "-parse-as-library",
                    "-target",
                    target,
                    "-module-cache-path",
                ])
                .arg(&module_cache)
                .args([source, "-o"])
                .arg(&helper);
            command
        };
        let sdk_version = Command::new("xcrun")
            .args(["--sdk", "macosx", "--show-sdk-version"])
            .output()
            .expect("failed to query macOS SDK version; install/select Xcode first");
        assert!(
            sdk_version.status.success(),
            "xcrun could not report the macOS SDK version"
        );
        let sdk_version = String::from_utf8_lossy(&sdk_version.stdout);
        let sdk_major = sdk_major(sdk_version.trim()).expect("invalid macOS SDK version");

        if sdk_major < 26 {
            println!(
                "cargo:warning=macOS SDK {sdk_version} predates SpeechAnalyzer support; compiling an explicit unsupported stub"
            );
            let stub_status = swift_args("native/apple_speech_helper_stub.swift")
                .status()
                .expect("failed to run xcrun swiftc for Apple Speech stub");
            assert!(
                stub_status.success(),
                "failed to compile Apple Speech unsupported stub"
            );
        } else {
            let real_status = swift_args("native/apple_speech_helper.swift")
                .status()
                .expect("failed to run xcrun swiftc; install/select Xcode first");
            assert!(
                real_status.success(),
                "failed to compile Apple Speech helper with macOS SDK {sdk_version}"
            );
        }
        println!(
            "cargo:rustc-env=APPLE_SPEECH_HELPER_PATH={}",
            helper.display()
        );
    }

    tauri_build::build()
}

#[cfg(test)]
mod tests {
    use super::sdk_major;

    #[test]
    fn parses_macos_sdk_major() {
        assert_eq!(sdk_major("26.1"), Some(26));
        assert_eq!(sdk_major("14.5"), Some(14));
        assert_eq!(sdk_major(""), None);
        assert_eq!(sdk_major("not-a-version"), None);
    }
}
