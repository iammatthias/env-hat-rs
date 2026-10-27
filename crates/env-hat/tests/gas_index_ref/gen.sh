#!/bin/sh
# Regenerates expected.csv and expected_states.csv from Sensirion's reference C.
# -ffp-contract=off: clang fuses a*b+c into FMA by default, which Rust never does.
set -eu
cd "$(dirname "$0")"
out=$(mktemp)
cc -std=c99 -O0 -ffp-contract=off -w -o "$out" harness.c sensirion_gas_index_algorithm.c -lm
"$out" > expected.csv
"$out" states > expected_states.csv
rm -f "$out"
