#!/usr/bin/env python3
"""Scan all Git refs and the publishable working tree; never read ignored data."""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--gitleaks', default='gitleaks')
args = parser.parse_args()
reports = ROOT / '.build/open-source-audit'
reports.mkdir(parents=True, exist_ok=True)
common = ['--redact', '--report-format', 'json', '--ignore-gitleaks-allow']
subprocess.run([args.gitleaks, 'git', *common, '--report-path', str(reports / 'gitleaks-history.json'), '--log-opts=--all'], cwd=ROOT, check=True)
with tempfile.TemporaryDirectory(prefix='edenvoice-publishable-tree-') as temp:
    names = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], cwd=ROOT).decode().split('\0')
    for name in names:
        if not name:
            continue
        path = ROOT / name
        if not path.exists():
            continue
        if path.is_symlink():
            raise SystemExit(f'Review symbolic link before publishing: {name}')
        target = Path(temp) / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, target)
    subprocess.run([args.gitleaks, 'dir', *common, '--report-path', str(reports / 'gitleaks-worktree.json'), temp], cwd=ROOT, check=True)
