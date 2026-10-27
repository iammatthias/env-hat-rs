#!/usr/bin/env bash
# The project's CI, run locally. It runs as the pre-push hook (see
# .githooks) and by hand:
#
#   bash ci/run.sh
#
# On the host it first lints the Markdown docs (ci/docs-lint.sh). Then,
# inside an arm64 Debian trixie container (the Pi's platform), it checks
# formatting, runs clippy with warnings as errors, runs the tests (including
# the fixtures that need Linux errno text), audits Cargo.lock against
# RustSec, and builds the device binaries in release mode. It also runs
# the tests on the host when the host isn't Linux.
set -euo pipefail
cd "$(dirname "$0")/.."

bash ci/docs-lint.sh

image=env-hat-ci
docker build -q -t "$image" ci >/dev/null

docker run --rm -v "$PWD":/work -v env-hat-cargo:/root/.cargo/registry \
  -e CARGO_TARGET_DIR=/work/target/ci "$image" sh -ec '
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    cargo audit --deny warnings
    cargo build --workspace --locked --release
  '

if [ "$(uname -s)" != Linux ]; then
  cargo test --workspace --locked
fi
echo "ci: all checks passed"
