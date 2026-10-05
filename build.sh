#!/usr/bin/env bash
# build.sh -- build and test ferrumquant, optionally by product.
# Works with the bash 3.2 that ships on macOS (no associative arrays).
set -euo pipefail
cd "$(dirname "$0")"

# ---------------------------------------------------------------------------
# Product registry: name -> "crate test-filter".
# The filter is a module-path substring passed to `cargo test`, so tests are
# "labelled" by where they live (e.g. fq-rates/src/swap/... -> "swap::").
# Add new products here as their modules land.
# ---------------------------------------------------------------------------
PRODUCTS_ALL="date daycount calendar schedule time solver black core bond future curve index swap swaption golden rates fx-barrier"

product_target() {
    case "$1" in
        date)       echo "fq-core time::date::" ;;
        daycount)   echo "fq-core time::daycount::" ;;
        calendar)   echo "fq-core time::calendar::" ;;
        schedule)   echo "fq-core time::schedule::" ;;
        time)       echo "fq-core time::" ;;
        solver)     echo "fq-core math::solver::" ;;
        black)      echo "fq-core math::black::" ;;
        core)       echo "fq-core -" ;;
        bond)       echo "fq-rates bond::" ;;
        future)     echo "fq-rates future::" ;;
        swap)       echo "fq-rates swap::" ;;
        swaption)   echo "fq-rates swaption::" ;;
        golden)     echo "fq-rates golden_" ;;
        curve)      echo "fq-rates curve::" ;;
        index)      echo "fq-rates index::" ;;
        rates)      echo "fq-rates -" ;;
        fx-barrier) echo "fq-fx barrier::" ;;
        *)          return 1 ;;
    esac
}

usage() {
    cat <<'EOF'
Usage: ./build.sh [options] [-- <test-binary args>]

Build and test the ferrumquant workspace, optionally restricted to products.

Selection:
  -p, --product NAME   Test/build one product (repeatable). See --list.
  -c, --crate NAME     Restrict to a crate, e.g. fq-core (repeatable).
  -t, --test FILTER    Raw cargo test name filter (substring match).
  -l, --list           List products and what they map to.

Mode:
  -b, --build-only     Compile only (libs + test binaries), run nothing.
      --list-tests     List matching test names without running them.
      --clippy         Run clippy on the selection with -D warnings.
      --fmt            Check formatting (cargo fmt --check).
      --clean          cargo clean before anything else.

Profile:
  -d, --dev            Debug build (default).
  -r, --release        Optimized build.
      --profile NAME   Any cargo profile, e.g. bench.

Output:
  -n, --nocapture      Show println! output from tests.
  -v, --verbose        Echo every cargo command.
  -j, --jobs N         Parallel build jobs.
  -h, --help           Show this help.

Examples:
  ./build.sh                          # build + test everything (dev)
  ./build.sh -b -r                    # release build only
  ./build.sh -p bond -p daycount      # just bond and day-count tests
  ./build.sh -p swaption -r -n        # swaption tests, release, with output
  ./build.sh -p bond -t yield         # bond tests whose names contain "yield"
  ./build.sh --clippy --fmt           # lint pass for CI
  ./build.sh -p bond -- --test-threads=1
EOF
}

list_products() {
    printf "%-12s %-10s %s\n" "PRODUCT" "CRATE" "FILTER"
    for p in $PRODUCTS_ALL; do
        set -- $(product_target "$p")
        local status=""
        [ -f "crates/$1/Cargo.toml" ] || status="  (crate not created yet)"
        printf "%-12s %-10s %s%s\n" "$p" "$1" "$([ "$2" = "-" ] && echo "(all)" || echo "$2")" "$status"
    done
}

# --- defaults --------------------------------------------------------------
PROFILE="dev"
BUILD_ONLY=0 LIST_TESTS=0 CLIPPY=0 FMT=0 CLEAN=0 NOCAPTURE=0 VERBOSE=0
JOBS="" RAW_FILTER=""
PRODUCTS=() CRATES=() TEST_ARGS=()

die() { echo "build.sh: $*" >&2; exit 1; }
need_arg() { [ $# -ge 2 ] && [ -n "$2" ] || die "option $1 needs a value"; }

# --- parse -----------------------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)     usage; exit 0 ;;
        -l|--list)     list_products; exit 0 ;;
        -p|--product)  need_arg "$@"; PRODUCTS+=("$2"); shift ;;
        -c|--crate)    need_arg "$@"; CRATES+=("$2"); shift ;;
        -t|--test)     need_arg "$@"; RAW_FILTER="$2"; shift ;;
        -b|--build-only) BUILD_ONLY=1 ;;
        --list-tests)  LIST_TESTS=1 ;;
        --clippy)      CLIPPY=1 ;;
        --fmt)         FMT=1 ;;
        --clean)       CLEAN=1 ;;
        -d|--dev)      PROFILE="dev" ;;
        -r|--release)  PROFILE="release" ;;
        --profile)     need_arg "$@"; PROFILE="$2"; shift ;;
        -n|--nocapture) NOCAPTURE=1 ;;
        -v|--verbose)  VERBOSE=1 ;;
        -j|--jobs)     need_arg "$@"; JOBS="$2"; shift ;;
        --)            shift; TEST_ARGS=("$@"); break ;;
        *)             die "unknown option '$1' (try --help)" ;;
    esac
    shift
done

# --- common cargo flags ----------------------------------------------------
CARGO_FLAGS=()
case "$PROFILE" in
    dev)     ;;
    release) CARGO_FLAGS+=(--release) ;;
    *)       CARGO_FLAGS+=(--profile "$PROFILE") ;;
esac
[ -n "$JOBS" ] && CARGO_FLAGS+=(-j "$JOBS")

run() {
    [ "$VERBOSE" -eq 1 ] && echo "+ $*" >&2
    "$@"
}

# --- resolve selection into "crate filter" pairs ---------------------------
TARGETS=()
for p in ${PRODUCTS[@]+"${PRODUCTS[@]}"}; do
    tgt=$(product_target "$p") || die "unknown product '$p' (try --list)"
    set -- $tgt
    if [ ! -f "crates/$1/Cargo.toml" ]; then
        echo "build.sh: skipping '$p': crate $1 does not exist yet" >&2
        continue
    fi
    TARGETS+=("$1 $2")
done
for c in ${CRATES[@]+"${CRATES[@]}"}; do
    [ -f "crates/$c/Cargo.toml" ] || die "no such crate '$c'"
    TARGETS+=("$c -")
done
if [ ${#PRODUCTS[@]} -gt 0 ] && [ ${#TARGETS[@]} -eq 0 ]; then
    die "nothing to do: every selected product's crate is missing"
fi

# Package flags for build/clippy: unique crates, or the whole workspace.
PKG_FLAGS=()
if [ ${#TARGETS[@]} -eq 0 ]; then
    PKG_FLAGS=(--workspace)
else
    seen=" "
    for t in "${TARGETS[@]}"; do
        c=${t%% *}
        case "$seen" in *" $c "*) ;; *) PKG_FLAGS+=(-p "$c"); seen="$seen$c " ;; esac
    done
fi

# --- actions ---------------------------------------------------------------
command -v cargo >/dev/null 2>&1 || die "cargo not found on PATH"

START=$(date +%s)
echo "==> profile: $PROFILE   selection: ${PRODUCTS[*]:-${CRATES[*]:-workspace}}"

[ "$CLEAN" -eq 1 ] && run cargo clean
[ "$FMT" -eq 1 ] && run cargo fmt --all -- --check

if [ "$CLIPPY" -eq 1 ]; then
    run cargo clippy "${PKG_FLAGS[@]}" --all-targets ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"} -- -D warnings
fi

if [ "$BUILD_ONLY" -eq 1 ]; then
    run cargo build "${PKG_FLAGS[@]}" --all-targets ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
    echo "==> build finished in $(( $(date +%s) - START ))s"
    exit 0
fi

# Test binary args: nocapture, list mode, then anything after `--`.
BIN_ARGS=()
[ "$NOCAPTURE" -eq 1 ] && BIN_ARGS+=(--nocapture)
[ "$LIST_TESTS" -eq 1 ] && BIN_ARGS+=(--list)
BIN_ARGS+=(${TEST_ARGS[@]+"${TEST_ARGS[@]}"})

run_tests() { # filter crate-flags...
    local filter="$1"; shift
    [ "$filter" = "-" ] && filter=""
    local base=(cargo test "$@" ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"})
    if [ -n "$filter" ] && [ -n "$RAW_FILTER" ]; then
        # libtest ORs multiple filters, so AND product + name by listing
        # the tests and re-running the matches with --exact.
        local names
        names=$("${base[@]}" -q -- --list 2>/dev/null | sed -n 's/: test$//p' \
            | grep -F -- "$filter" | grep -F -- "$RAW_FILTER" || true)
        if [ -z "$names" ]; then
            echo "    no tests match '$filter' and '$RAW_FILTER'"
            return 0
        fi
        # shellcheck disable=SC2086  # test names contain no spaces
        run "${base[@]}" -- --exact $names ${BIN_ARGS[@]+"${BIN_ARGS[@]}"}
        return
    fi
    local f="${filter:-$RAW_FILTER}"
    run "${base[@]}" ${f:+"$f"} -- ${BIN_ARGS[@]+"${BIN_ARGS[@]}"}
}

if [ ${#TARGETS[@]} -eq 0 ]; then
    run_tests "" --workspace
else
    for t in "${TARGETS[@]}"; do
        set -- $t
        echo "==> testing $1 ${2/#-/(all)}"
        run_tests "$2" -p "$1"
    done
fi

echo "==> done in $(( $(date +%s) - START ))s"
