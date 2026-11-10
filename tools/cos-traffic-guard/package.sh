#!/usr/bin/env bash
set -Eeuo pipefail
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
output_dir=${1:-"$source_dir/../../dist/cos-traffic-guard"}
mkdir -p "$output_dir"
output_dir=$(cd "$output_dir" && pwd)
(cd "$source_dir" && go test ./... && go vet ./...)
for arch in amd64 arm64; do
 stage=$(mktemp -d)
 trap 'rm -rf "$stage"' EXIT
 package="edenvoice-cos-guard-linux-$arch"
 mkdir "$stage/$package"
 (cd "$source_dir" && CGO_ENABLED=0 GOOS=linux GOARCH="$arch" go build -trimpath -ldflags='-s -w' -o "$stage/$package/cos-traffic-guard" .)
 for file in config.json cam-policy.json edenvoice-cos-guard.service install.sh README.md; do
  cp "$source_dir/$file" "$stage/$package/"
 done
 cp "$source_dir/../../LICENSE" "$source_dir/../../THIRD_PARTY_NOTICES.md" "$source_dir/../../THIRD_PARTY_LICENSES.md" "$stage/$package/"
 # Include build/installation scripts as well as the Go module and dependencies.
 python3 "$source_dir/../../scripts/package-source.py" --scope cos --output "$stage/$package/corresponding-source.tar.gz"
 chmod 755 "$stage/$package/install.sh"
 COPYFILE_DISABLE=1 tar -C "$stage" -czf "$output_dir/$package.tar.gz" "$package"
 rm -rf "$stage"
 trap - EXIT
done
(cd "$output_dir" && shasum -a 256 edenvoice-cos-guard-linux-*.tar.gz > SHA256SUMS.txt)
printf 'Packages: %s\n' "$output_dir"
