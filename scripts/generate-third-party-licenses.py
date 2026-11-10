#!/usr/bin/env python3
"""Collect verbatim notices from the locked, downloaded dependencies.

Run after npm ci and cargo fetch. No license text is synthesized. The retained
native-library/model notices in THIRD_PARTY_NOTICES.md are also required.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parent.parent


def run(*args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True)


def json_stream(text):
    decoder = json.JSONDecoder()
    while text.strip():
        text = text.lstrip()
        value, offset = decoder.raw_decode(text)
        yield value
        text = text[offset:]


def license_files(directory, explicit=None):
    paths = set()
    if explicit:
        candidate = directory / explicit
        if candidate.resolve().is_relative_to(directory.resolve()):
            paths.add(candidate)
    for base in [directory, directory / 'licenses', directory / 'LICENSES']:
        if base.is_dir():
            for path in base.iterdir():
                if path.is_file() and path.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'UNLICENSE', 'COPYRIGHT')):
                    paths.add(path)
    return sorted(path for path in paths if path.is_file())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--fetch-missing', action='store_true', help='Fetch omitted notices from the exact upstream Git commit via gh')
    args = parser.parse_args()
    groups = {}
    missing = []
    fallbacks = json.loads((ROOT / 'third-party/license-fallbacks.json').read_text())

    def upstream_notices(directory, repository):
        vcs = directory / '.cargo_vcs_info.json'
        if not vcs.is_file() or not repository or urlparse(repository).hostname != 'github.com':
            return []
        revision = json.loads(vcs.read_text())
        commit = revision['git']['sha1']
        repo = urlparse(repository).path.strip('/').removesuffix('.git')
        repo = '/'.join(repo.split('/')[:2])
        cache = ROOT / 'third-party/licenses' / hashlib.sha256((repo + commit).encode()).hexdigest()[:20]
        if cache.is_dir():
            return license_files(cache)
        if not args.fetch_missing:
            return []
        texts = {}
        for folder in ['', revision.get('path_in_vcs', '')]:
            response = subprocess.run(['gh', 'api', f'repos/{repo}/contents/{folder}?ref={commit}'], capture_output=True, text=True)
            if response.returncode:
                continue
            entries = json.loads(response.stdout)
            if not isinstance(entries, list):
                continue
            for entry in entries:
                if entry['type'] != 'file' or not entry['name'].upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'UNLICENSE', 'COPYRIGHT')):
                    continue
                value = json.loads(run('gh', 'api', f'repos/{repo}/contents/{entry["path"]}?ref={commit}'))
                texts[entry['name']] = base64.b64decode(value['content']).decode('utf-8')
            if texts:
                break
        if texts:
            cache.mkdir(parents=True, exist_ok=True)
            for name, text in texts.items():
                (cache / name).write_text(text)
            (cache / 'SOURCE.json').write_text(json.dumps({'repository': 'https://github.com/' + repo, 'commit': commit, 'files_sha256': {name: hashlib.sha256(text.encode()).hexdigest() for name, text in texts.items()}}, indent=2) + '\n')
        return license_files(cache)

    def add(label, directory, explicit=None, repository=None):
        files = license_files(directory, explicit)
        # DeepFilterNet keeps its license texts in the repository root.
        if not files and label.startswith('Rust deep_filter '):
            files = license_files(directory.parent)
        if not files:
            files = upstream_notices(directory, repository)
        if not files:
            missing.append(label)
        for path in files:
            content = path.read_text(encoding='utf-8').strip()
            digest = hashlib.sha256(content.encode()).hexdigest()
            groups.setdefault(digest, {'text': content, 'packages': set()})['packages'].add(label)

    crates = {}
    for manifest in ['client/Cargo.toml', 'desktop/src-tauri/Cargo.toml', 'tools/glib-security-check/Cargo.toml']:
        data = json.loads(run('cargo', 'metadata', '--locked', '--format-version', '1', '--manifest-path', manifest))
        for package in data['packages']:
            if not package['name'].startswith('edenvoice-'):
                crates[(package['name'], package['version'])] = package
    for (name, version), package in sorted(crates.items()):
        label = f'Rust {name} {version} ({package.get("license") or "see upstream license"})'
        directory = Path(package['manifest_path']).parent
        if f'{name}@{version}' in fallbacks:
            chosen = fallbacks[f'{name}@{version}']
            # Reviewed packages publish an SPDX declaration but no separate
            # license file at their recorded upstream commit. Preserve their
            # author metadata and existing source notices, and supply the
            # standard terms explicitly, without inventing copyright holders.
            terms = (ROOT / 'third-party/spdx' / (chosen + '.txt')).read_text()
            notices = set()
            for path in directory.rglob('*'):
                if path.suffix in ['.rs', '.md', '.toml', '.c', '.h'] and path.is_file():
                    for line in path.read_text(errors='replace').splitlines():
                        if 'copyright' in line.lower() or 'SPDX-License-Identifier:' in line:
                            notices.add(line.strip())
            provenance = directory / '.cargo_vcs_info.json'
            content = ('Published package: ' + name + ' ' + version + '\nDeclared license: ' + package['license']
                       + '\nAuthors from Cargo.toml: ' + '; '.join(package.get('authors', []))
                       + '\nRecorded upstream revision: ' + (provenance.read_text().strip() if provenance.is_file() else 'not supplied by upstream')
                       + '\nRetained source notices:\n' + '\n'.join(sorted(notices))
                       + '\n\nNo separate license text is supplied by this upstream package. Standard SPDX ' + chosen + ' terms follow:\n\n' + terms)
            groups.setdefault(hashlib.sha256(content.encode()).hexdigest(), {'text': content, 'packages': set()})['packages'].add(label)
        else:
            add(label, directory, package.get('license_file'), package.get('repository'))
    for module in ['server', 'tools/cos-traffic-guard']:
        run('go', 'mod', 'download', 'all', cwd=ROOT / module)
        for package in json_stream(run('go', 'list', '-m', '-json', 'all', cwd=ROOT / module)):
            if not package.get('Main'):
                add(f'Go {package["Path"]} {package["Version"]}', Path(package['Dir']))
    lock = json.loads((ROOT / 'desktop/package-lock.json').read_text())
    for location, package in sorted(lock['packages'].items()):
        if not location:
            continue
        directory = ROOT / 'desktop' / location
        if not directory.exists() and package.get('optional'):
            continue  # Platform-specific build tools absent from this distribution.
        # Native npm tool packages inherit the notices of their same-version
        # JavaScript wrapper; the native package archives omit those files.
        for prefix, wrapper in [('@esbuild/', 'esbuild'), ('@rolldown/binding-', 'rolldown'), ('@tauri-apps/cli-', '@tauri-apps/cli')]:
            if location.removeprefix('node_modules/').startswith(prefix) and not license_files(directory):
                directory = ROOT / 'desktop/node_modules' / wrapper
        add(f'npm {location.removeprefix("node_modules/")} {package["version"]} ({package.get("license", "see upstream license")})', directory)
    if missing:
        raise SystemExit('Missing bundled license text; review upstream before publishing:\n' + '\n'.join(missing))
    parts = ['# Locked dependency license texts\n\n'
             'Generated by `python3 scripts/generate-third-party-licenses.py`. '
             'Includes the Cargo lockfiles across platforms, both Go modules, '
             'and installed npm packages. Alternative licenses are retained as '
             'published; Slint is used under GPL-3.0-only. Also distribute '
             '`LICENSE` and `THIRD_PARTY_NOTICES.md` for native libraries and models.\n']
    for group in sorted(groups.values(), key=lambda item: sorted(item['packages'])):
        parts.append('\n## ' + '; '.join(sorted(group['packages'])) + '\n\n```text\n' + group['text'] + '\n```\n')
    content = ''.join(parts)
    target = ROOT / 'THIRD_PARTY_LICENSES.md'
    if args.check:
        if not target.is_file() or target.read_text() != content:
            raise SystemExit('THIRD_PARTY_LICENSES.md is stale; regenerate it.')
    else:
        target.write_text(content)
    print(f'Checked {len(crates)} Rust packages; retained {len(groups)} distinct notice texts.')


if __name__ == '__main__':
    main()
