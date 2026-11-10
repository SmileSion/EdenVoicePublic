#!/usr/bin/env python3
"""Bundle the exact source tree and vendored dependencies beside release binaries."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scope', required=True, choices=['client', 'server', 'cos'])
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        raise SystemExit(f'Refusing to overwrite source archive: {output}')
    if not (ROOT / 'THIRD_PARTY_LICENSES.md').is_file():
        raise SystemExit('Generate THIRD_PARTY_LICENSES.md before packaging.')
    paths = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], cwd=ROOT).decode().split('\0')
    with tempfile.TemporaryDirectory(prefix='edenvoice-source-') as temp:
        source = Path(temp) / 'EdenVoice-source'
        source.mkdir()
        hashes = {}
        for name in sorted(set(paths)):
            if not name:
                continue
            path = ROOT / name
            if not path.exists():
                continue
            # A release never includes repository metadata or local credentials.
            if (path.is_symlink() or name.split('/')[0].startswith('.') and name.split('/')[0] not in ['.gitignore', '.dockerignore']
                    or path.suffix in ['.key', '.pem', '.p12', '.pfx'] or path.name.startswith('.env')):
                raise SystemExit(f'Review unsafe source archive entry: {name}')
            target = source / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
            hashes[name] = hashlib.sha256(target.read_bytes()).hexdigest()
        if args.scope == 'client':
            metadata = json.loads(subprocess.check_output([
                'cargo', 'metadata', '--locked', '--format-version', '1',
                '--manifest-path', str(source / 'client/Cargo.toml'),
            ], text=True, cwd=source))
            config = subprocess.check_output([
                'cargo', 'vendor', '--locked', '--versioned-dirs',
                '--manifest-path', str(source / 'desktop/src-tauri/Cargo.toml'),
                '--sync', str(source / 'client/Cargo.toml'), str(source / 'vendor/cargo'),
                '--sync', str(source / 'tools/glib-security-check/Cargo.toml'),
            ], text=True, cwd=source)
            # Cargo accepts a portable path relative to this config's parent.
            config = config.replace(str(source / 'vendor/cargo'), 'vendor/cargo')
            (source / '.cargo').mkdir(exist_ok=True)
            (source / '.cargo/config.toml').write_text(config)
            # DeepFilterNet includes models outside its libDF crate directory.
            # Cargo vendor flattens git crates, so retain the relative ../models
            # layout expected by its include_bytes! calls as well.
            deep_filter = next(p for p in metadata['packages'] if p['name'] == 'deep_filter')
            model_root = Path(deep_filter['manifest_path']).parent.parent / 'models'
            target_models = source / 'vendor/cargo/models'
            target_models.mkdir()
            for filename in ['DeepFilterNet3_onnx.tar.gz', 'DeepFilterNet3_ll_onnx.tar.gz']:
                model_file = model_root / filename
                if not model_file.is_file():
                    raise SystemExit(f'Missing pinned DeepFilterNet model: {filename}')
                shutil.copy2(model_file, target_models / filename)
                hashes['vendor/cargo/models/' + filename] = hashlib.sha256(model_file.read_bytes()).hexdigest()
            model = os.environ.get('RNNOISE_MODEL_PATH')
            if not model:
                candidates = sorted(ROOT.glob('client/target/**/rnnoise_data-*.tar.gz'))
                model = str(candidates[0]) if candidates else None
            if not model or not Path(model).is_file():
                raise SystemExit('Set RNNOISE_MODEL_PATH to the exact model archive used for the build.')
            (source / 'models').mkdir()
            shutil.copy2(model, source / 'models' / Path(model).name)
            hashes['models/' + Path(model).name] = hashlib.sha256(Path(model).read_bytes()).hexdigest()
        else:
            module = 'server' if args.scope == 'server' else 'tools/cos-traffic-guard'
            subprocess.run(['go', 'mod', 'vendor'], cwd=source / module, check=True)
        manifest = {
            'version': json.loads((source / 'desktop/package.json').read_text())['version'],
            'git_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'scope': args.scope, 'files_sha256': hashes,
            'note': 'The file hashes identify the actual working tree, including uncommitted fixes. Dependency source and its upstream notices are vendored alongside it.',
        }
        (source / 'SOURCE_MANIFEST.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (source / 'BUILD_SOURCE.txt').write_text(
            'See README.md for platform prerequisites. Go requires 1.26.10 or newer.\n'
            'Go modules include vendor/; build with go build -mod=vendor.\n'
            'Client source archives include Cargo dependency sources in vendor/cargo;\n'
            'build with --locked --offline after setting CMAKE_POLICY_VERSION_MINIMUM=3.5\n'
            'and RNNOISE_MODEL_PATH to the absolute path of models/rnnoise_data-*.tar.gz.\n'
            'Run npm ci in desktop/ for the locked frontend build tools.\n'
            'Read LICENSE, THIRD_PARTY_NOTICES.md, THIRD_PARTY_LICENSES.md and vendor/README.md.\n')
        output.parent.mkdir(parents=True, exist_ok=True)
        with tarfile.open(output, 'w:gz', format=tarfile.PAX_FORMAT) as archive:
            def clean(info):
                info.uid = info.gid = 0
                info.uname = info.gname = ''
                info.pax_headers = {}
                return info
            archive.add(source, arcname=source.name, filter=clean)
    print(output)


if __name__ == '__main__':
    main()
