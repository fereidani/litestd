#!/usr/bin/env bash
# Runs litestd's tests under Miri, several test targets at a time: Miri
# interprets a program on one thread, so a single `cargo miri test` leaves
# the other processors idle. Runs the unit tests, the doc tests and each
# file in `tests/` as a target of its own, prints each target's result as
# it ends and then the output of those that failed, and exits non-zero if
# one did. Honors `MIRIFLAGS`, `CARGO_TARGET_DIR`, and `RUSTUP_TOOLCHAIN`,
# which defaults to `nightly`.
#
# A target that stops at a foreign function Miri does not emulate, with no
# undefined behavior found before, is unsupported rather than failed; it
# fails the run only without `--allow-unsupported`. Miri lacks many Windows
# functions, which leaves part of the tests out there.
#
# Usage: scripts/miri.sh [--allow-unsupported] [TARGET]
#
# Interprets the programs for TARGET, a target triple, instead of the host.

set -euo pipefail

cd "$(dirname "$0")/.."
export RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-nightly}
allow_unsupported=false
if [[ ${1-} == --allow-unsupported ]]; then
    allow_unsupported=true
    shift
fi
target=()
if [[ $# -gt 0 ]]; then
    target=(--target "$1")
fi
logs=$(mktemp -d)
trap 'rm -f -- "$logs"/*; rmdir -- "$logs"' EXIT
export logs
# `run` below splits this back into its words.
export target_args="${target[*]}"

cargo miri test --no-run --features test-with-std "${target[@]}"

# Runs the target that the `cargo test` arguments in $1 select.
run() {
    local log="$logs/${1// /-}.log"
    # $1 and the target split into their arguments, such as `--test io`.
    # shellcheck disable=SC2086
    if cargo miri test --features test-with-std $target_args $1 \
        >"$log" 2>&1; then
        echo "ok           $1"
    elif grep -q "unsupported operation:" "$log" &&
        ! grep -q "Undefined Behavior" "$log"; then
        local missing
        missing=$(grep -o "foreign function \`[^\`]*\`" "$log" | head -n 1)
        echo "unsupported  $1: ${missing:-see its output}"
        mv -- "$log" "$log.unsupported"
    else
        echo "FAILED       $1"
        mv -- "$log" "$log.failed"
    fi
}
export -f run

{
    echo --lib
    echo --doc
    for file in tests/*.rs; do
        echo "--test $(basename "$file" .rs)"
    done
} | xargs -P "$(getconf _NPROCESSORS_ONLN)" -I {} bash -c 'run "$1"' _ {}

failed=0
for log in "$logs"/*.failed; do
    [[ -e $log ]] || continue
    cat -- "$log"
    failed=1
done
if ! $allow_unsupported; then
    for log in "$logs"/*.unsupported; do
        [[ -e $log ]] || continue
        cat -- "$log"
        failed=1
    done
fi
exit "$failed"
