#!/usr/bin/env bash
# Fetches the html5lib test corpus into crates/tenta-html/tests/html5lib-tests/
# (gitignored; CI runs this before `cargo nextest run -p tenta-html`).
#
# The tokenizer and encoding suites live in html5lib/html5lib-tests (MIT).
# The tree-construction .dat files moved to web-platform-tests, under
# html/syntax/parsing/resources/, so they are fetched from there and placed
# in tree-construction/ where the runner expects them.
set -euo pipefail

HTML5LIB_REPO="https://github.com/html5lib/html5lib-tests.git"
HTML5LIB_COMMIT="224991ec10db04f056a89eed8b0bd8695fd2950e"

WPT_REPO="https://github.com/web-platform-tests/wpt.git"
# Keep in sync with wpt/config.toml.
WPT_COMMIT="b89af32bc8f42d678f444eb0703bca015ddcf240"
WPT_DIR="html/syntax/parsing/resources"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="${script_dir}/html5lib-tests"
repo_root="$(cd "${script_dir}/../../.." && pwd)"

rm -rf "${dest}"
mkdir -p "${dest}"

echo "fetching html5lib-tests at ${HTML5LIB_COMMIT}"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT
git -C "${tmp}" init --quiet html5lib
git -C "${tmp}/html5lib" remote add origin "${HTML5LIB_REPO}"
git -C "${tmp}/html5lib" fetch --quiet --depth 1 origin "${HTML5LIB_COMMIT}"
git -C "${tmp}/html5lib" checkout --quiet FETCH_HEAD
cp -R "${tmp}/html5lib/tokenizer" "${dest}/tokenizer"
cp -R "${tmp}/html5lib/encoding" "${dest}/encoding"
cp "${tmp}/html5lib/LICENSE" "${dest}/LICENSE"

echo "fetching web-platform-tests ${WPT_DIR} at ${WPT_COMMIT}"
mkdir -p "${dest}/tree-construction"
if [ -d "${repo_root}/wpt/checkout/${WPT_DIR}" ] &&
   [ "$(git -C "${repo_root}/wpt/checkout" rev-parse HEAD 2>/dev/null || true)" = "${WPT_COMMIT}" ]; then
  echo "  using the existing wpt/checkout"
  cp "${repo_root}/wpt/checkout/${WPT_DIR}"/*.dat "${dest}/tree-construction/"
  cp "${repo_root}/wpt/checkout/${WPT_DIR}/README.md" "${dest}/tree-construction/"
else
  git -C "${tmp}" init --quiet wpt
  git -C "${tmp}/wpt" remote add origin "${WPT_REPO}"
  git -C "${tmp}/wpt" config core.sparseCheckout true
  git -C "${tmp}/wpt" sparse-checkout set --no-cone "${WPT_DIR}"
  git -C "${tmp}/wpt" fetch --quiet --depth 1 origin "${WPT_COMMIT}"
  git -C "${tmp}/wpt" checkout --quiet FETCH_HEAD
  cp "${tmp}/wpt/${WPT_DIR}"/*.dat "${dest}/tree-construction/"
  cp "${tmp}/wpt/${WPT_DIR}/README.md" "${dest}/tree-construction/"
fi

echo "done:"
echo "  $(ls "${dest}/tree-construction"/*.dat | wc -l | tr -d ' ') tree-construction files"
echo "  $(ls "${dest}/tokenizer" | wc -l | tr -d ' ') tokenizer files"
echo "  $(ls "${dest}/encoding" | wc -l | tr -d ' ') encoding entries"
