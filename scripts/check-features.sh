#!/usr/bin/env bash
# Lints litestd's library with clippy, warnings denied, for combinations of
# its module features on Linux, macOS, FreeBSD, NetBSD, Windows, WASI 0.1
# and 0.2, and wasm32-unknown-unknown: no features, each feature alone, each
# pair, all but one, and all of them. Then, with the nightly toolchain, lints
# the `nightly` feature: all targets with every module, and the library with
# `thread` alone and with no module; OpenBSD and DragonFly, which have no
# prebuilt std, as listed below; and WebAssembly with threads, which needs
# `nightly`: every combination on `wasm32-wasip1-threads`, and a few on
# wasm32-unknown-unknown built with the `atomics` target feature. Prints
# every failing combination and exits non-zero if there is one. Honors
# `CARGO_TARGET_DIR`, and ignores the host's `RUSTFLAGS`, which would break
# the cross targets.
#
# Usage: scripts/check-features.sh [stable|nightly]
#
# With an argument, runs only the stable combinations or only the nightly
# checks.

set -euo pipefail

part=${1:-all}
case $part in
    all | stable | nightly) ;;
    *)
        echo "usage: $0 [stable|nightly]" >&2
        exit 2
        ;;
esac

# The module features. Every other list in this script derives from it.
FEATURES=(alloc command env fs io net path process stdio sync thread time)
TARGETS=(
    x86_64-unknown-linux-gnu
    aarch64-apple-darwin
    x86_64-apple-darwin
    x86_64-unknown-freebsd
    x86_64-unknown-netbsd
    x86_64-pc-windows-gnu
    wasm32-wasip1
    wasm32-unknown-unknown
)
# Targets whose tests do not build, as `std::os::wasi` is unstable on WASI
# 0.2: only the library's combinations.
LIB_TARGETS=(wasm32-wasip2)
# WebAssembly with threads, which the nightly half lints.
THREADS_TARGET=wasm32-wasip1-threads
ATOMICS_RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals"
# Targets without a prebuilt std, whose library the nightly half checks by
# building `core` and `alloc` from source: without features, with each
# feature alone, and with all of them.
SOURCE_TARGETS=(x86_64-unknown-openbsd x86_64-unknown-dragonfly)

cd "$(dirname "$0")/.."
unset RUSTFLAGS

count=${#FEATURES[@]}
combos=()
declare -A seen=()

# Adds the combination of the given features, unless it is already listed.
add() {
    local combo
    combo=$(IFS=,; echo "$*")
    if [[ -z ${seen["[$combo]"]+set} ]]; then
        seen["[$combo]"]=1
        combos+=("$combo")
    fi
}

add
for ((i = 0; i < count; i++)); do
    add "${FEATURES[i]}"
done
for ((i = 0; i < count; i++)); do
    for ((j = i + 1; j < count; j++)); do
        add "${FEATURES[i]}" "${FEATURES[j]}"
    done
done
for ((i = 0; i < count; i++)); do
    rest=()
    for ((j = 0; j < count; j++)); do
        if ((j != i)); then
            rest+=("${FEATURES[j]}")
        fi
    done
    add "${rest[@]}"
done
add "${FEATURES[@]}"

failures=()
total=0

# Runs cargo with the given arguments and warnings denied, recording the
# check as $1. `CHECK_RUSTFLAGS`, when set, becomes cargo's `RUSTFLAGS`.
check() {
    local label=$1 output
    shift
    total=$((total + 1))
    if output=$(env ${CHECK_RUSTFLAGS:+"RUSTFLAGS=$CHECK_RUSTFLAGS"} \
        cargo "$@" -- -D warnings 2>&1); then
        printf 'ok   %s\n' "$label"
    else
        failures+=("$label")
        printf 'FAIL %s\n%s\n' "$label" "$output"
    fi
}

if [[ $part != nightly ]]; then
    for target in "${TARGETS[@]}" "${LIB_TARGETS[@]}"; do
        for combo in "${combos[@]}"; do
            args=(clippy --quiet --lib --no-default-features --target "$target")
            if [[ -n $combo ]]; then
                args+=(--features "$combo")
            fi
            check "$target [${combo:-no features}]" "${args[@]}"
        done
    done
fi

if [[ $part != stable ]]; then
    all=$(IFS=,; echo "${FEATURES[*]}")
    for target in "${TARGETS[@]}"; do
        check "$target nightly [all targets, all modules]" +nightly clippy \
            --quiet --all-targets --no-default-features --target "$target" \
            --features "$all,nightly,test-with-std"
        for combo in thread,nightly nightly; do
            check "$target nightly [$combo]" +nightly clippy --quiet --lib \
                --no-default-features --target "$target" --features "$combo"
        done
    done
    for target in "${SOURCE_TARGETS[@]}"; do
        for combo in "" "${FEATURES[@]}" "$all" "$all,nightly"; do
            args=(+nightly clippy --quiet -Zbuild-std=core,alloc --lib
                --no-default-features --target "$target")
            if [[ -n $combo ]]; then
                args+=(--features "$combo")
            fi
            check "$target nightly [${combo:-no features}]" "${args[@]}"
        done
    done
    for combo in "${combos[@]}"; do
        features=nightly${combo:+,$combo}
        check "$THREADS_TARGET nightly [$features]" +nightly clippy --quiet \
            --lib --no-default-features --target "$THREADS_TARGET" \
            --features "$features"
    done
    check "$THREADS_TARGET nightly [all targets, all modules]" +nightly \
        clippy --quiet --all-targets --no-default-features \
        --target "$THREADS_TARGET" --features "$all,nightly,test-with-std"
    for combo in "" "${FEATURES[@]}" "$all"; do
        features=nightly${combo:+,$combo}
        CHECK_RUSTFLAGS=$ATOMICS_RUSTFLAGS check \
            "wasm32-unknown-unknown atomics [$features]" +nightly clippy \
            --quiet -Zbuild-std=core,alloc --lib --no-default-features \
            --target wasm32-unknown-unknown --features "$features"
    done
fi

if ((${#failures[@]} > 0)); then
    printf '\n%d of %d checks failed:\n' "${#failures[@]}" "$total"
    printf '  %s\n' "${failures[@]}"
    exit 1
fi
printf '\nall %d checks passed\n' "$total"
