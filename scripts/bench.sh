#!/bin/sh
# Runs the benchmarks, which compare litestd with std in one binary each.
#
# Usage: scripts/bench.sh [BENCH [FILTER]]
#
# BENCH is `sync`, `fs`, `net` or `io`; FILTER keeps the benchmarks whose name
# contains it. RUSTFLAGS is cleared so that litestd is compiled for the same
# CPU baseline as the prebuilt std it is compared with. BENCH_TOOLCHAIN picks
# a rustup toolchain and BENCH_FEATURES adds features, so
# `BENCH_TOOLCHAIN=nightly BENCH_FEATURES=nightly scripts/bench.sh sync
# thread_local` measures native thread-locals.
set -eu
cd "$(dirname "$0")/.."
features=test-with-std${BENCH_FEATURES:+,$BENCH_FEATURES}
if [ $# -eq 0 ]; then
    exec env -u RUSTFLAGS cargo ${BENCH_TOOLCHAIN:+"+$BENCH_TOOLCHAIN"} \
        bench --features "$features"
fi
bench=$1
shift
exec env -u RUSTFLAGS cargo ${BENCH_TOOLCHAIN:+"+$BENCH_TOOLCHAIN"} \
    bench --features "$features" --bench "$bench" -- "$@"
