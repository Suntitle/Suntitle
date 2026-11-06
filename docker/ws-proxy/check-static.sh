#!/bin/sh
# Release executables MUST have neither shared dependencies nor a dynamic loader.
set -eu
for binary in "$@"; do
    dynamic="$(readelf -d "$binary")"
    headers="$(readelf -l "$binary")"
    case "$dynamic" in
        *'(NEEDED)'*) echo "Shared library dependency in $binary" >&2; exit 1 ;;
    esac
    case "$headers" in
        *INTERP*) echo "Dynamic loader in $binary" >&2; exit 1 ;;
    esac
    "$binary" --help >/dev/null
done
