#!/usr/bin/env bash
# Compares the size of small programs built against std and against litestd.
# The programs are the probe crate in scripts/size.
#
# Usage:
#   scripts/size.sh [TARGET...]
#       Builds every program for each target with std and with litestd, runs
#       each build once, and prints the stripped sizes. The default targets
#       are x86_64 Linux with glibc and with static musl, and x86_64 Windows
#       with the GNU toolchain, built with cargo-zigbuild, and with MSVC,
#       built with cargo-xwin; both run under wine. Apple targets are built
#       with cargo-zigbuild for macOS 14.4, or MACOSX_DEPLOYMENT_TARGET, and
#       not run.
#   scripts/size.sh symbols [-t TARGET] [-n COUNT] PROGRAM
#       Prints the COUNT largest symbols (default 30) of the unstripped
#       litestd build of PROGRAM, then its size per crate. On Windows and
#       macOS it measures only the Rust code, in the object that LTO
#       produces, where each function extends to the next.
#
# The probe crate's release profile sets `panic = "abort"`, fat LTO, one
# codegen unit and stripping. RUSTFLAGS is replaced so that litestd is built
# for the same CPU baseline as the prebuilt std; it only silences the
# linker's chatter. Builds go to target/size, or to size under
# `CARGO_TARGET_DIR`.

set -euo pipefail

PROGRAMS=(hello env threads fs tcp child io_error)
TARGETS=(
    x86_64-unknown-linux-gnu
    x86_64-unknown-linux-musl
    x86_64-pc-windows-gnu
    x86_64-pc-windows-msvc
)

root=$(cd "$(dirname "$0")/.." && pwd)
manifest=$root/scripts/size/Cargo.toml
out=${CARGO_TARGET_DIR:-$root/target}/size

usage() {
    sed -n '/^# Usage:/,/^#$/s/^# \{0,1\}//p' "$0" >&2
    exit 2
}

# Builds the programs for target $1 into $out/$2, passing the remaining
# arguments to cargo and `EXTRA_RUSTFLAGS` to rustc.
build() {
    local target=$1 dir=$2 cmd=(build)
    shift 2
    if [[ $target == *windows-msvc* ]]; then
        cmd=(xwin build)
    elif [[ $target == *windows* || $target == *apple* ]]; then
        cmd=(zigbuild)
    fi
    RUSTFLAGS="-Alinker-messages ${EXTRA_RUSTFLAGS-}" \
        MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-14.4} \
        CARGO_TARGET_DIR="$out/$dir" cargo "${cmd[@]}" --quiet --release \
        --manifest-path "$manifest" --target "$target" "$@"
}

# Prints the path of program $2 built for target $1 into $out/$3.
binary() {
    local suffix=
    if [[ $1 == *windows* ]]; then
        suffix=.exe
    fi
    printf '%s/%s/%s/release/%s%s\n' "$out" "$3" "$1" "$2" "$suffix"
}

# Runs the program $1 built for target $2 once, from a new scratch directory
# that it must leave empty, and fails with its output if the program does.
# Apple programs cannot run here.
run() {
    local bin=$1 target=$2 scratch output status=0
    local cmd=(timeout 60 "$bin")
    if [[ $target == *apple* ]]; then
        return
    elif [[ $target == *windows* ]]; then
        cmd=(env WINEDEBUG=-all timeout 120 wine "$bin")
    fi
    scratch=$(mktemp -d)
    output=$(cd "$scratch" && "${cmd[@]}" 2>&1) || status=$?
    rmdir "$scratch"
    if ((status != 0)); then
        printf 'error: %s exited with status %d:\n%s\n' "$bin" "$status" \
            "$output" >&2
        return 1
    fi
}

# Prints the sizes of every program for the targets given, or the default
# ones, and how litestd's compare with std's.
compare() {
    local targets=("$@") target variant program std lite
    if ((${#targets[@]} == 0)); then
        targets=("${TARGETS[@]}")
    fi
    for target in "${targets[@]}"; do
        for variant in std lite; do
            if [[ $variant == lite ]]; then
                build "$target" "$variant" --features lite
            else
                build "$target" "$variant"
            fi
            for program in "${PROGRAMS[@]}"; do
                run "$(binary "$target" "$program" "$variant")" "$target"
            done
        done
    done
    printf '%-9s %-26s %11s %11s\n' program target std litestd
    for target in "${targets[@]}"; do
        for program in "${PROGRAMS[@]}"; do
            std=$(stat -c %s "$(binary "$target" "$program" std)")
            lite=$(stat -c %s "$(binary "$target" "$program" lite)")
            awk -v p="$program" -v t="$target" -v s="$std" -v l="$lite" '
                BEGIN {
                    if (l <= s) v = sprintf("%.1fx smaller", s / l)
                    else v = sprintf("%.0f%% larger", (l / s - 1) * 100)
                    printf "%-9s %-26s %7.1f KiB %7.1f KiB  %s\n",
                        p, t, s / 1024, l / 1024, v
                }'
        done
    done
}

# Lists the largest symbols of a litestd build and its size per crate.
symbols() {
    local target=x86_64-unknown-linux-gnu count=30 opt bin= f end
    while getopts t:n: opt; do
        case $opt in
            t) target=$OPTARG ;;
            n) count=$OPTARG ;;
            *) usage ;;
        esac
    done
    shift $((OPTIND - 1))
    if (($# != 1)); then
        usage
    fi
    if [[ $target == *windows* || $target == *apple* ]]; then
        # zig keeps unused code in unstripped Windows programs, and neither
        # their symbols nor those of Mach-O programs have sizes.
        EXTRA_RUSTFLAGS=-Csave-temps build "$target" symbols \
            --features lite --bin "$1"
        # MSVC programs lack the hash suffix that cargo gives the others.
        for f in "$out/symbols/$target/release/deps/$1"[-.]*.rcgu.o; do
            if [[ -z ${bin-} || $f -nt $bin ]]; then
                bin=$f
            fi
        done
        if [[ $target == *windows-msvc* ]]; then
            # Each function has a section of its own there: its size is the
            # section's. llvm-readobj cannot demangle Rust's v0 names.
            llvm-readobj --sections --section-symbols "$bin" |
                awk '
                    /^  Section \{/ { size = 0; code = 0; named = 0 }
                    /^    RawDataSize:/ { size = $2 }
                    /IMAGE_SCN_CNT_CODE/ { code = 1 }
                    /^        Name: / && code && !named {
                        name = substr($0, index($0, "Name: ") + 6)
                        if (name !~ /^\./) { print size, name; named = 1 }
                    }' | llvm-cxxfilt
        else
            end=$(llvm-size -A "$bin" |
                awk '$1 == ".text" || $1 == "__text" { print $2 }')
            llvm-nm --numeric-sort --defined-only --demangle --radix=d \
                "$bin" | awk -v end="$end" '
                    $2 ~ /^[tT]$/ {
                        if (name != "") print $1 - addr, name
                        addr = $1; $1 = ""; $2 = ""; name = substr($0, 3)
                    }
                    END { if (name != "") print end - addr, name }'
        fi
    else
        CARGO_PROFILE_RELEASE_STRIP=false build "$target" symbols \
            --features lite --bin "$1"
        nm --size-sort --print-size --demangle --radix=d \
            "$(binary "$target" "$1" symbols)" |
            awk '$3 ~ /^[tTrRdDbB]$/ {
                     size = $2; $1 = ""; $2 = ""; $3 = ""
                     print size + 0, substr($0, 4)
                 }'
    fi | sort -rn | awk -v n="$count" '
        {
            size = $1; name = substr($0, length($1) + 2)
            if (NR <= n) printf "%8d  %s\n", size, name
            # The crate is the first path segment; C symbols have none.
            crate = "[C]"
            if (match(name, /[A-Za-z_][A-Za-z0-9_]*::/))
                crate = substr(name, RSTART, RLENGTH - 2)
            total[crate] += size
            all += size
        }
        END {
            printf "\n%8d  total\n", all
            sort = "sort -rn"
            for (crate in total) printf "%8d  %s\n", total[crate], crate | sort
        }'
}

if [[ ${1-} == symbols ]]; then
    shift
    symbols "$@"
elif [[ ${1-} == -* ]]; then
    usage
else
    compare "$@"
fi
