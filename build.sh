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
PRODUCTS_ALL="date daycount calendar schedule period imm time solver black interp sabr core bond spread future delivery deposit sofr-future curve jacobian index swap swaption volcube capfloor golden rates fx-barrier"

product_target() {
    case "$1" in
        date)       echo "fq-core time::date::" ;;
        daycount)   echo "fq-core time::daycount::" ;;
        calendar)   echo "fq-core time::calendar::" ;;
        schedule)   echo "fq-core time::schedule::" ;;
        period)     echo "fq-core time::period::" ;;
        imm)        echo "fq-core time::imm::" ;;
        time)       echo "fq-core time::" ;;
        solver)     echo "fq-core math::solver::" ;;
        black)      echo "fq-core math::black::" ;;
        interp)     echo "fq-core math::interpolation::" ;;
        sabr)       echo "fq-core math::sabr::" ;;
        core)       echo "fq-core -" ;;
        bond)       echo "fq-rates bond::" ;;
        spread)     echo "fq-rates bond::spread::" ;;
        future)     echo "fq-rates future::" ;;
        delivery)   echo "fq-rates future::delivery_option::" ;;
        deposit)    echo "fq-rates instrument::deposit::" ;;
        sofr-future) echo "fq-rates future::sofr_future::" ;;
        swap)       echo "fq-rates swap::" ;;
        swaption)   echo "fq-rates swaption::" ;;
        volcube)    echo "fq-rates swaption::vol_cube::" ;;
        capfloor)   echo "fq-rates capfloor::" ;;
        golden)     echo "fq-rates golden_" ;;
        curve)      echo "fq-rates curve::" ;;
        jacobian)   echo "fq-rates curve::jacobian::" ;;
        index)      echo "fq-rates index::" ;;
        rates)      echo "fq-rates -" ;;
        fx-barrier) echo "fq-fx barrier::" ;;
        *)          return 1 ;;
    esac
}

# Python-side products: name -> pytest file under python/tests.
# `-p py-NAME` with `--py test` runs just that file; `-t` adds a -k filter.
PY_PRODUCTS_ALL="py-bond py-curve py-swap py-swaption"

py_product_target() {
    case "$1" in
        py-bond)     echo "test_bonds.py" ;;
        py-curve)    echo "test_curves.py" ;;
        py-swap)     echo "test_swaps.py" ;;
        py-swaption) echo "test_swaptions.py" ;;
        *)           return 1 ;;
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

Python (crates/fq-py bindings + python/fqview tool):
      --py ACTION      One of:
                         setup  create the venv and install maturin/pytest
                         build  compile the ferrumquant module into the venv
                         test   run python/tests (honours -p py-* and -t)
                         view   print the sample blotter (args after --)
                         shell  open a shell with the venv activated
                         all    setup + build + test + view
                       build/test/view set up the venv first if it is missing.
      --venv DIR       Virtual env location (default: .venv).
      --python EXE     Interpreter used to create the venv (default: python3).
      --with-quantlib  Also install QuantLib-Python for the cross-check tests.
      --py-list        List python products.

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
  ./build.sh --py all --with-quantlib  # venv, module, pytest, blotter
  ./build.sh --py build -r             # optimized module into the venv
  ./build.sh --py test -p py-bond -t sweep
  ./build.sh --py view -- 2026-10-16
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
    echo
    list_py_products
}

list_py_products() {
    printf "%-12s %-10s %s\n" "PY PRODUCT" "RUNNER" "FILE"
    for p in $PY_PRODUCTS_ALL; do
        f=$(py_product_target "$p")
        local status=""
        [ -f "python/tests/$f" ] || status="  (not written yet)"
        printf "%-12s %-10s %s%s\n" "$p" "pytest" "python/tests/$f$status"
    done
}

# --- defaults --------------------------------------------------------------
PROFILE="dev"
BUILD_ONLY=0 LIST_TESTS=0 CLIPPY=0 FMT=0 CLEAN=0 NOCAPTURE=0 VERBOSE=0
JOBS="" RAW_FILTER=""
PRODUCTS=() CRATES=() TEST_ARGS=()
PY_ACTION="" VENV=".venv" PYTHON="python3" WITH_QUANTLIB=0

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
        --py)          need_arg "$@"; PY_ACTION="$2"; shift ;;
        --venv)        need_arg "$@"; VENV="$2"; shift ;;
        --python)      need_arg "$@"; PYTHON="$2"; shift ;;
        --with-quantlib) WITH_QUANTLIB=1 ;;
        --py-list)     list_py_products; exit 0 ;;
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

# --- python ----------------------------------------------------------------
# The venv lives at $VENV (default .venv, git-ignored). maturin compiles
# crates/fq-py straight into it, so `import ferrumquant` works from any
# script run with $VENV/bin/python; python/fqview is used in place via
# PYTHONPATH, no install step needed.
case "$VENV" in /*) VENV_ABS="$VENV" ;; *) VENV_ABS="$PWD/$VENV" ;; esac
VENV_PY="$VENV_ABS/bin/python"
[ -d "$VENV_ABS/Scripts" ] && VENV_PY="$VENV_ABS/Scripts/python.exe"   # Windows/Git Bash

py_setup() {
    if [ ! -x "$VENV_PY" ]; then
        command -v "$PYTHON" >/dev/null 2>&1 || die "$PYTHON not found on PATH"
        echo "==> creating venv at $VENV with $("$PYTHON" --version 2>&1)"
        run "$PYTHON" -m venv "$VENV"
    fi
    echo "==> installing python build/test deps"
    run "$VENV_PY" -m pip install -q --upgrade pip
    run "$VENV_PY" -m pip install -q "maturin>=1.5,<2" pytest
    if [ "$WITH_QUANTLIB" -eq 1 ]; then
        echo "==> installing QuantLib-Python (cross-check tests)"
        run "$VENV_PY" -m pip install -q QuantLib
    fi
}

py_ensure() { [ -x "$VENV_PY" ] || py_setup; }

py_build() {
    py_ensure
    local flags=()
    [ "$PROFILE" = "release" ] && flags+=(--release)
    [ "$PROFILE" != "dev" ] && [ "$PROFILE" != "release" ] && flags+=(--profile "$PROFILE")
    echo "==> maturin develop (crates/fq-py -> $VENV)"
    # maturin locates the target venv through VIRTUAL_ENV.
    ( cd crates/fq-py && VIRTUAL_ENV="$VENV_ABS" run "$VENV_PY" -m maturin develop ${flags[@]+"${flags[@]}"} )
}

py_have_module() { "$VENV_PY" -c "import ferrumquant" >/dev/null 2>&1; }

py_test() {
    py_ensure
    py_have_module || py_build
    local files=()
    for p in ${PRODUCTS[@]+"${PRODUCTS[@]}"}; do
        case "$p" in
            py-*) f=$(py_product_target "$p") || die "unknown python product '$p' (try --py-list)"
                  [ -f "python/tests/$f" ] || { echo "build.sh: skipping '$p': python/tests/$f not written yet" >&2; continue; }
                  files+=("tests/$f") ;;
            *)    die "'$p' is a Rust product; with --py test use py-* names (try --py-list)" ;;
        esac
    done
    local args=(-q)
    [ "$NOCAPTURE" -eq 1 ] && args+=(-s)
    [ "$VERBOSE" -eq 1 ] && args=(-v)
    [ -n "$RAW_FILTER" ] && args+=(-k "$RAW_FILTER")
    echo "==> pytest ${files[*]:-python/tests}"
    ( cd python && run "$VENV_PY" -m pytest "${args[@]}" ${files[@]+"${files[@]}"} "$@" )
}

py_view() {
    py_ensure
    py_have_module || py_build
    ( cd python && run "$VENV_PY" -m fqview "$@" )
}

py_shell() {
    py_ensure
    echo "==> venv shell ($VENV); PYTHONPATH includes python/. Type 'exit' to leave."
    PYTHONPATH="$PWD/python${PYTHONPATH:+:$PYTHONPATH}" VIRTUAL_ENV="$VENV_ABS" \
        PATH="$VENV_ABS/bin:$PATH" "${SHELL:-bash}" -i
}

if [ -n "$PY_ACTION" ]; then
    START=$(date +%s)
    case "$PY_ACTION" in
        setup) py_setup ;;
        build) py_build ;;
        test)  py_test ${TEST_ARGS[@]+"${TEST_ARGS[@]}"} ;;
        view)  py_view ${TEST_ARGS[@]+"${TEST_ARGS[@]}"} ;;
        shell) py_shell ;;
        all)   py_setup; py_build; py_test; py_view ${TEST_ARGS[@]+"${TEST_ARGS[@]}"} ;;  # extra args go to the blotter
        *)     die "unknown --py action '$PY_ACTION' (setup|build|test|view|shell|all)" ;;
    esac
    echo "==> python done in $(( $(date +%s) - START ))s"
    exit 0
fi

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
