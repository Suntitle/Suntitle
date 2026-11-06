#!/bin/bash
# Checksums MUST pass before either executable is installed.
set -euo pipefail

release_version="$1"
release_repository="$2"
install_dir="$3"
if [[ ! "$release_version" =~ ^v[0-9][A-Za-z0-9._-]*$ ]]; then
    echo 'A pinned release tag beginning with v and a digit is required.' >&2
    exit 1
fi
if [[ ! "$release_repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]; then
    echo 'The release repository MUST use owner/repository format.' >&2
    exit 1
fi

release_dir="$(mktemp -d)"
trap 'rm -rf "$release_dir"' EXIT
cd "$release_dir"
release_url="https://github.com/${release_repository}/releases/download/${release_version}"
for component in ws-proxy control-api; do
    archive="${component}.x86_64-unknown-linux-musl.tar.xz"
    curl -fsSL --retry 5 "${release_url}/${archive}" -o "$archive"
    curl -fsSL --retry 5 "${release_url}/${archive}.sha256" -o "${archive}.sha256"
    checksum="$(awk -v name="$archive" '{sub(/^\*/, "", $2)} $2 == name {print $1}' "${archive}.sha256")"
    if [[ ! "$checksum" =~ ^[[:xdigit:]]{64}$ ]]; then
        echo "Missing or invalid checksum for $archive." >&2
        exit 1
    fi
    printf '%s  %s\n' "$checksum" "$archive" | sha256sum -c -
done
# Both downloads are verified before extraction or installation.
tar -xJf ws-proxy.x86_64-unknown-linux-musl.tar.xz ws-proxy
tar -xJf control-api.x86_64-unknown-linux-musl.tar.xz control_api
install -d "$install_dir"
install -m 755 ws-proxy control_api "$install_dir/"
