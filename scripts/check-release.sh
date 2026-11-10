#!/usr/bin/env bash
# Local release checks. Deliberately does not publish, sign, or change visibility.
set -Eeuo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_root"
audit_bin=${CARGO_AUDIT_BINARY:-cargo-audit}
gitleaks_bin=${GITLEAKS_BINARY:-gitleaks}
for tool in go cargo npm python3 "$audit_bin" "$gitleaks_bin"; do
    command -v "$tool" >/dev/null || { printf 'Required release tool missing: %s\n' "$tool" >&2; exit 1; }
done
for module in server tools/cos-traffic-guard; do
    (cd -- "$module" && go test ./... && go vet ./... && go run golang.org/x/vuln/cmd/govulncheck@v1.9.0 ./...)
done
npm --prefix desktop ci
npm --prefix desktop run build
npm --prefix desktop test
npm --prefix desktop run test:admin
npm --prefix desktop audit --audit-level=moderate
cargo test --manifest-path client/Cargo.toml --locked --lib
cargo test --manifest-path client/Cargo.toml --locked --test nnef_security
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --manifest-path desktop/src-tauri/Cargo.toml --locked --lib
for lockfile in client/Cargo.lock desktop/src-tauri/Cargo.lock tools/glib-security-check/Cargo.lock; do
    "$audit_bin" audit --file "$lockfile"
done
if [[ $(uname -s) == Linux ]]; then
    cargo test --manifest-path tools/glib-security-check/Cargo.toml --locked --release
fi
python3 scripts/generate-third-party-licenses.py --check
python3 scripts/check-secrets.py --gitleaks "$gitleaks_bin"
printf 'Local release checks passed. Review platform capture QA and remote-history cleanup before publishing.\n'
