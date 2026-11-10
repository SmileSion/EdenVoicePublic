#!/usr/bin/env python3
"""Build the macOS capture probe with the real client's media declarations.

Launch the .app (not its executable) for a representative permission identity.
--check --report <path> checks API availability without capturing any media.
"""
import argparse
import pathlib
import plistlib
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=pathlib.Path, default=pathlib.Path('/tmp') / 'EdenScreenCaptureProbe-R3.app')
args = parser.parse_args()
source = pathlib.Path(__file__).resolve().parent
contents = args.output / 'Contents'
binary = contents / 'MacOS' / 'probe'
binary.parent.mkdir(parents=True, exist_ok=True)
subprocess.run(['swiftc', '-module-cache-path', str(pathlib.Path('/tmp') / 'eden-swift-module-cache'), str(source / 'screen-share-webkit-probe.swift'), '-o', str(binary)], check=True)
# WKWebView can hide mediaDevices when a normally launched app lacks a media
# usage declaration. Do not inherit the launching terminal's identity as proof.
info = plistlib.loads((source.parent / 'src-tauri' / 'Info.plist').read_bytes())
assert info.get('NSMicrophoneUsageDescription'), 'Client media usage declaration is required'
info.update(CFBundleExecutable='probe', CFBundleIdentifier='dev.edenvoice.capture-probe.r3', CFBundleName='EdenScreenCaptureProbe-R3', CFBundlePackageType='APPL', CFBundleVersion='3', CFBundleShortVersionString='3.0')
(contents / 'Info.plist').write_bytes(plistlib.dumps(info))
print(args.output.resolve())
