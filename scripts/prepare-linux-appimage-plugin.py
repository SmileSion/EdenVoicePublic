#!/usr/bin/env python3
"""Expose linuxdeploy's AppImage plugin without executing its static-PIE runtime."""

from __future__ import annotations

import argparse
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path


def squashfs_offset(image: Path) -> int:
    data = image.read_bytes()
    cursor = 0
    while True:
        offset = data.find(b"hsqs", cursor)
        if offset < 0:
            break
        cursor = offset + 1
        if len(data) - offset < 48:
            continue
        block_size = struct.unpack_from("<I", data, offset + 12)[0]
        major, minor = struct.unpack_from("<HH", data, offset + 28)
        bytes_used = struct.unpack_from("<Q", data, offset + 40)[0]
        if (
            major == 4
            and minor == 0
            and block_size >= 4096
            and block_size & (block_size - 1) == 0
            and 0 < bytes_used <= len(data) - offset
        ):
            return offset
    raise SystemExit(f"no valid SquashFS payload found in {image}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--launcher", type=Path, required=True)
    parser.add_argument("--wrapper", type=Path, required=True)
    args = parser.parse_args()

    source = args.source.resolve()
    destination = args.destination.resolve()
    launcher = args.launcher.resolve()
    wrapper = args.wrapper.resolve()
    inner_plugin = destination / "usr/bin/linuxdeploy-plugin-appimage"
    appimagetool = destination / "appimagetool-prefix/usr/bin/appimagetool"

    if not (inner_plugin.is_file() and appimagetool.is_file()):
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(
            prefix=f".{destination.name}.", dir=destination.parent
        ) as temporary:
            extracted = Path(temporary) / "root"
            subprocess.run(
                [
                    "unsquashfs",
                    "-no-progress",
                    "-offset",
                    str(squashfs_offset(source)),
                    "-dest",
                    str(extracted),
                    str(source),
                ],
                check=True,
            )
            if destination.exists():
                shutil.rmtree(destination)
            extracted.rename(destination)

    shutil.copyfile(wrapper, launcher)
    launcher.chmod(0o755)
    print(f"prepared emulation-safe AppImage plugin launcher: {launcher}")


if __name__ == "__main__":
    main()
