#!/usr/bin/env bash
# Downloads a release archive, refuses it unless its SHA-256 is the one given, and puts
# one program of it into a folder.
#
#   pinned-tool.sh <url> <sha256> <the program's path in the archive> <folder>
set -euo pipefail

url=$1
sha256=$2
member=$3
folder=$4

archive=$(mktemp)
trap 'rm -f "$archive"' EXIT

curl --fail --silent --show-error --location --retry 3 --output "$archive" "$url"
echo "$sha256  $archive" | sha256sum --check --quiet -

mkdir -p "$folder"
# The folders above the program in the archive are left out: it lands in the folder itself.
above=$(awk -F/ '{ print NF - 1 }' <<<"$member")
tar --extract --gzip --file "$archive" --directory "$folder" --strip-components "$above" "$member"
