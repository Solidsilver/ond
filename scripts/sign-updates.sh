#!/usr/bin/env bash
# Sign only the downloaded release artifacts; a missing key still permits a
# notify-only release, while a broken configured key must stop publication.
set -euo pipefail
if [[ -z "${UPDATE_SIGNING_KEY:-}" ]]; then
  echo '::warning::No UPDATE_SIGNING_KEY: publishing unsigned; automatic installation will be unavailable.'
  exit 0
fi
key="$(mktemp)"
trap 'rm -f "$key"' EXIT
chmod 600 "$key"
printf '%s\n' "$UPDATE_SIGNING_KEY" > "$key"
for file in "${1:-dist}"/*; do
  [[ -f "$file" && "$file" != *.minisig ]] || continue
  printf '%s\n' "${UPDATE_SIGNING_PASSWORD:-}" | minisign -S -s "$key" -m "$file"
done
