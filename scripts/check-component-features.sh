#!/usr/bin/env bash
set -euo pipefail

# check-component-features.sh
#
# SUMMARY
#
#   Ensures that all components have corresponding features in `Cargo.toml` and
#   that each of these features declares declares all dependencies
#   necessary to build it without having other features enabled.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Here are all the features that will be added along the tested features.
# Used mainly for passing mandatory, mutually exclusive features -
# which are designed to force an error in not properly set.
FORCED_FEATURES=(
  alloc-jemalloc # check with jemalloc
)

toml-extract() {
  WHAT="$1"
  remarshal --if toml --of json | jq -r "$WHAT"
}

check-listed-features() {
  xargs -I{} sh -cx "(cargo check --tests --no-default-features --features '${FORCED_FEATURES[*]}' --features {}) || exit 255"
}

cargo-clean-when-in-ci() {
  if [[ "${CI:-"false"}" == "true" ]]; then
    echo "Cleaning to save some disk space"
    cargo clean
  fi
}

echo "Checking that Vector and tests can be built without default features..."
cargo check --tests --no-default-features --features "${FORCED_FEATURES[*]}"

echo "Checking that all components have corresponding features in Cargo.toml..."
COMPONENTS="$(cargo run --no-default-features --features "${FORCED_FEATURES[*]}" -- list)"
if (echo "$COMPONENTS" | grep -E -v "(Log level|^(Sources:|Transforms:|Sinks:|)$)" >/dev/null); then
  echo "Some of the components do not have a corresponding feature flag in Cargo.toml:"
  # shellcheck disable=SC2001
  echo "$COMPONENTS" | sed "s/^/    /"
  exit 1
fi

echo "Checking that each source feature can be built without other features..."
toml-extract ".features.sources|.[]" < Cargo.toml | check-listed-features

cargo-clean-when-in-ci

echo "Checking that each transform feature can be built without other features..."
toml-extract ".features.transforms|.[]" < Cargo.toml | check-listed-features

cargo-clean-when-in-ci

echo "Checking that each sink feature can be built without other features..."
toml-extract ".features.sinks|.[]" < Cargo.toml | check-listed-features
