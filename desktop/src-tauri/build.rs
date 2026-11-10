fn main() {
    println!("cargo:rerun-if-changed=native/system_audio.m");
    println!("cargo:rerun-if-changed=native/system_audio.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
            "arm64"
        } else {
            "x86_64"
        };
        assert!(
            std::process::Command::new("xcrun")
                .args([
                    "clang",
                    "-arch",
                    arch,
                    "-fobjc-arc",
                    "-mmacosx-version-min=12.0",
                    "-c",
                    "native/system_audio.m",
                    "-o"
                ])
                .arg(out.join("system_audio.o"))
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("ar")
                .arg("crs")
                .arg(out.join("libeden_system_audio.a"))
                .arg(out.join("system_audio.o"))
                .status()
                .unwrap()
                .success()
        );
        println!("cargo:rustc-link-search=native={}", out.display());
        println!("cargo:rustc-link-lib=static=eden_system_audio");
        println!("cargo:rustc-link-lib=framework=CoreAudio");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
    tauri_build::build()
}
