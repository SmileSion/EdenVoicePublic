#!/usr/bin/env python3
"""Generate the Meson cross environment used by cargo-xwin on macOS."""

from __future__ import annotations

import argparse
from pathlib import Path


def quote(value: Path | str) -> str:
    return "'" + str(value).replace("\\", "\\\\").replace("'", "\\'") + "'"


def array(values: list[Path | str]) -> str:
    return "[" + ", ".join(quote(value) for value in values) + "]"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--xwin-root", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--clang-cl", type=Path, required=True)
    parser.add_argument("--llvm-lib", type=Path, required=True)
    parser.add_argument("--llvm-strip", type=Path, required=True)
    parser.add_argument("--llvm-rc", type=Path, required=True)
    args = parser.parse_args()

    output_dir = args.output_dir.resolve()
    wrapper_dir = output_dir / "bin"
    wrapper_dir.mkdir(parents=True, exist_ok=True)
    wrapper = wrapper_dir / "clang-cl"
    wrapper.write_text(
        "#!/usr/bin/env python3\nimport os\nimport sys\n\n"
        "arguments = [os.path.relpath(arg) if arg.startswith('/') and os.path.exists(arg) else arg "
        "for arg in sys.argv[1:]]\n"
        f"os.execv({str(args.clang_cl.absolute())!r}, ['clang-cl', *arguments])\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o755)

    xwin = args.xwin_root.resolve()
    compiler_args: list[Path | str] = [
        "--target=x86_64-pc-windows-msvc",
        "-Wno-unused-command-line-argument",
        "-fuse-ld=lld-link",
    ]
    for include in (
        xwin / "crt/include",
        xwin / "sdk/include/ucrt",
        xwin / "sdk/include/um",
        xwin / "sdk/include/shared",
        xwin / "sdk/include/winrt",
    ):
        compiler_args.extend(("/imsvc", include))

    link_args: list[Path | str] = [
        "/link",
        "/manifest:no",
        "/subsystem:console",
        f"/libpath:{xwin / 'crt/lib/x86_64'}",
        f"/libpath:{xwin / 'sdk/lib/um/x86_64'}",
        f"/libpath:{xwin / 'sdk/lib/ucrt/x86_64'}",
    ]
    cross_file = output_dir / "webrtc-meson-cross.ini"
    cross_file.write_text(
        "[binaries]\n"
        f"c = {array([wrapper, *compiler_args])}\n"
        f"cpp = {array([wrapper, *compiler_args, '/EHsc'])}\n"
        f"ar = {quote(args.llvm_lib.absolute())}\n"
        f"strip = {quote(args.llvm_strip.absolute())}\n"
        f"windres = {quote(args.llvm_rc.absolute())}\n\n"
        "[built-in options]\n"
        f"c_link_args = {array(link_args)}\n"
        f"cpp_link_args = {array(link_args)}\n"
        "cpp_std = 'c++17'\n\n"
        "[properties]\nneeds_exe_wrapper = true\n\n"
        "[host_machine]\nsystem = 'windows'\ncpu_family = 'x86_64'\n"
        "cpu = 'x86_64'\nendian = 'little'\n",
        encoding="utf-8",
    )
    print(cross_file)


if __name__ == "__main__":
    main()
