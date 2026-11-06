#!/bin/bash
# Selected checksums MUST pass before any executable is installed.
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

components=(ws-proxy control-api)
if (( $# > 3 )); then
    components=("${@:4}")
fi
binaries=()
for component in "${components[@]}"; do
    case "$component" in
        control-api) binaries+=(control_api) ;;
        ws-proxy|diamant) binaries+=("$component") ;;
        *) echo "Unknown release component: $component" >&2; exit 1 ;;
    esac
done

release_dir="$(mktemp -d)"
trap 'rm -rf "$release_dir"' EXIT
cd "$release_dir"
release_url="https://github.com/${release_repository}/releases/download/${release_version}"
for component in "${components[@]}"; do
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
# Every selected download is verified before extraction or installation.
for index in "${!components[@]}"; do
    tar -xJf "${components[$index]}.x86_64-unknown-linux-musl.tar.xz" "${binaries[$index]}"
done
install -d "$install_dir"
install -m 755 "${binaries[@]}" "$install_dir/"
