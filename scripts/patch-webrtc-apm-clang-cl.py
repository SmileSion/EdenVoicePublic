#!/usr/bin/env python3
"""Patch webrtc-audio-processing-sys 2.1.0 for target-aware clang-cl builds."""

from __future__ import annotations

import os
from pathlib import Path


CRATE_VERSION = "2.1.0"
RELATIVE_SOURCE = Path(
    "webrtc-audio-processing/webrtc/modules/audio_processing/aec3/"
    "matched_filter_avx2.cc"
)
PATCHES = (
    (
        RELATIVE_SOURCE,
        "#ifdef _MSC_VER\n// Visual Studio\n",
        "#if defined(_MSC_VER) && !defined(__clang__)\n// Visual Studio\n",
        "clang-cl SIMD lookup",
    ),
    (
        Path("build.rs"),
        (
            '        .includes(&include_dirs)\n'
            '        .flag("-std=c++17")\n'
            '        .flag("-Wno-unused-parameter")\n'
            '        .out_dir(out_dir());',
            '        .includes(&include_dirs)\n'
            '        .flag("/std:c++17")\n'
            '        .flag("-Wno-unused-parameter")\n'
            '        .out_dir(out_dir());',
        ),
        '        .includes(&include_dirs)\n'
        '        .out_dir(out_dir());\n\n'
        '    if matches!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("windows")) {\n'
        '        cc_build.flag("/std:c++17");\n'
        '    } else {\n'
        '        cc_build.flag("-std=c++17");\n'
        '    }\n'
        '    cc_build.flag("-Wno-unused-parameter");',
        "clang-cl C++17 wrapper flag",
    ),
    (
        Path("build.rs"),
        (
            '    let objcopy = sysroot.join("lib").join("rustlib").join(host).join("bin").join("rust-objcopy");',
            '    let objcopy_name = if cfg!(windows) { "rust-objcopy.exe" } else { "rust-objcopy" };\n'
            '    let objcopy = sysroot.join("lib").join("rustlib").join(host).join("bin").join(objcopy_name);',
        ),
        '    let objcopy_name = if matches!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("windows")) { "rust-objcopy.exe" } else { "rust-objcopy" };\n'
        '    let objcopy = sysroot.join("lib").join("rustlib").join(host).join("bin").join(objcopy_name);',
        "Windows rust-objcopy executable",
    ),
    (
        Path("build.rs"),
        (
            '        .clang_args(&["-x", "c++", "-std=c++17", "-fparse-all-comments"])\n'
            '        .generate_comments(true)\n'
            '        .enable_cxx_namespaces();',
            '        .clang_args(&["-x", "c++", "-std=c++17", "-fparse-all-comments"])\n'
            '        .clang_arg(format!("--target={}", env::var("TARGET").expect("TARGET not set")))\n'
            '        .generate_comments(true)\n'
            '        .enable_cxx_namespaces();',
        ),
        '        .clang_args(&["-x", "c++", "-std=c++17", "-fparse-all-comments"])\n'
        '        .generate_comments(true)\n'
        '        .enable_cxx_namespaces();\n\n'
        '    if env::var("HOST").ok() != env::var("TARGET").ok() {\n'
        '        builder = builder\n'
        '            .clang_arg(format!("--target={}", env::var("TARGET").expect("TARGET not set")));\n'
        '    }',
        "bindgen target triple",
    ),
    (
        Path("build.rs"),
        (
            '        meson.arg("--reconfigure");\n\n'
            '        if cfg!(target_os = "macos") {',
            '        meson.arg("--reconfigure");\n\n'
            '        if let Ok(cross_file) = env::var("MESON_CROSS_FILE") {\n'
            '            meson.arg("--cross-file").arg(cross_file);\n'
            '        }\n\n'
            '        if matches!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos")) {',
        ),
        '        if webrtc_build_dir.join("meson-private/coredata.dat").exists() {\n'
        '            meson.arg("--reconfigure");\n'
        '        }\n\n'
        '        if let Ok(cross_file) = env::var("MESON_CROSS_FILE") {\n'
        '            meson.arg("--cross-file").arg(cross_file);\n'
        '        }\n\n'
        '        if matches!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos")) {',
        "Meson clean build, cross file, and target platform",
    ),
    (
        Path("build.rs"),
        '    if cfg!(target_os = "macos") {\n'
        '        println!("cargo:rustc-link-lib=framework=CoreFoundation");',
        '    if matches!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos")) {\n'
        '        println!("cargo:rustc-link-lib=framework=CoreFoundation");',
        "target macOS framework",
    ),
    (
        Path("build.rs"),
        '    // Set macos minimum version\n'
        '    if cfg!(target_os = "macos") {',
        '    // Set macos minimum version\n'
        '    if matches!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos")) {',
        "target macOS deployment version",
    ),
    (
        Path("build.rs"),
        '    let renamed_symbols = webrtc::prefix_library_symbols(&lib_dirs, SYMBOL_PREFIX)?;',
        '    let renamed_symbols = if matches!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("windows")) {\n'
        '        Vec::new()\n'
        '    } else {\n'
        '        webrtc::prefix_library_symbols(&lib_dirs, SYMBOL_PREFIX)?\n'
        '    };',
        "Windows COFF symbol prefix bypass",
    ),
)


def apply_patch(
    crate_dir: Path,
    relative_path: Path,
    original: str | tuple[str, ...],
    patched: str,
    label: str,
) -> None:
    source = crate_dir / relative_path
    text = source.read_text(encoding="utf-8")
    if patched in text:
        print(f"{label} patch already applied: {source}")
        return
    originals = (original,) if isinstance(original, str) else original
    matches = [candidate for candidate in originals if text.count(candidate) == 1]
    if len(matches) != 1:
        raise SystemExit(f"refusing to patch unexpected {label} source content: {source}")
    source.write_text(text.replace(matches[0], patched, 1), encoding="utf-8")
    print(f"applied {label} patch: {source}")


def main() -> None:
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    registry_sources = cargo_home / "registry" / "src"
    crate_dirs = sorted(
        registry_sources.glob(f"*/webrtc-audio-processing-sys-{CRATE_VERSION}")
    )
    if not crate_dirs:
        raise SystemExit(
            f"webrtc-audio-processing-sys {CRATE_VERSION} was not found under "
            f"{registry_sources}; run cargo fetch --locked first"
        )

    for crate_dir in crate_dirs:
        for relative_path, original, patched, label in PATCHES:
            apply_patch(crate_dir, relative_path, original, patched, label)


if __name__ == "__main__":
    main()
