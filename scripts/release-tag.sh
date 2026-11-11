#!/usr/bin/env bash
# Resolves a release tag (or a manual build's app) to GitHub job outputs.
# Both apps must agree with their tag before any bundle is built.
set -euo pipefail
cd "$(dirname "$0")/.."

version=""
case "${GITHUB_REF:-}" in
  refs/tags/ond-v*) app=ond; version="${GITHUB_REF#refs/tags/ond-v}" ;;
  refs/tags/rede-v*) app=rede; version="${GITHUB_REF#refs/tags/rede-v}" ;;
  refs/tags/v*) app=ond; version="${GITHUB_REF#refs/tags/v}" ;;
  refs/tags/*) echo "Unknown release tag: $GITHUB_REF" >&2; exit 1 ;;
  *) app="${1:-ond}" ;;
esac
case "$app" in
  ond) package=ond-player; name=Ond ;;
  rede) package=rede; name=Rede ;;
  *) echo "Unknown app: $app" >&2; exit 1 ;;
esac
crate="$(cargo pkgid -p "$package")"
crate="${crate##*#}"
crate="${crate##*@}"
if [[ "${GITHUB_REF:-}" == refs/tags/* && "$version" != "$crate" ]]; then
  echo "::error::$GITHUB_REF names $version, but $package is $crate" >&2
  exit 1
fi
version="$crate"
prerelease=false
if [[ "$version" == *-* ]]; then prerelease=true; fi
printf 'app=%s\nname=%s\nversion=%s\nprerelease=%s\n' "$app" "$name" "$version" "$prerelease"
