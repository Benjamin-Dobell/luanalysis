#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C LANG=C
unset LUANALYSIS_PROFILE

harness=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=${LUANALYSIS_REPO:-$(cd -- "$harness/../.." && pwd -P)}
baseline=${BASELINE_BIN:-}
current=${CURRENT_BIN:-}
lua=${LUALS_ROOT:-}
pi=${PROTOTYPE_ROOT:-}
output=
rounds=5
warmups=1
timeout=180
selected=()
usage() {
    cat <<'USAGE'
Usage: bash scripts/benchmark-type-predicates/bench.sh [options]
  --baseline PATH       Frozen baseline release binary (or BASELINE_BIN)
  --current PATH        Frozen current release binary (or CURRENT_BIN)
  --luals-root PATH     LuaLS 3.19.2 checkout (or LUALS_ROOT)
  --prototype-root PATH Project containing lib/ge_tts and lib/tts-types (or PROTOTYPE_ROOT)
  --repo PATH           Checker checkout; defaults to this script's repository
  --output PATH         New evidence directory; defaults to a fresh directory under TMPDIR
  --workload NAME       Select a workload; repeat to select several (default: all)
  --rounds N            Measured rounds per variant (default: 5)
  --warmups N           Warmup rounds per variant (default: 1)
  --timeout SECONDS     Timeout per process (default: 180)
  --quick               One measured round and a 60-second timeout
  --help                Show this usage text
Workloads: luals-j1 luals-j10 ge-tts-j1 ge-tts-j10 boolean128 boolean256 predicate128 predicate256 environment128 environment256 environment-bare128 environment-bare256
Corpus roots are required only for their selected workloads. No builds are run.
USAGE
}
while (($#)); do
    case "$1" in
        --help) usage; exit 0 ;;
        --quick) rounds=1; timeout=60; shift ;;
        --baseline|--current|--luals-root|--prototype-root|--repo|--output|--rounds|--warmups|--timeout|--workload)
            (($# >= 2)) || { printf 'Missing value for %s\n' "$1" >&2; exit 2; }
            case "$1" in
                --baseline) baseline=$2 ;;
                --current) current=$2 ;;
                --luals-root) lua=$2 ;;
                --prototype-root) pi=$2 ;;
                --repo) repo=$2 ;;
                --output) output=$2 ;;
                --rounds) rounds=$2 ;;
                --warmups) warmups=$2 ;;
                --timeout) timeout=$2 ;;
                --workload) selected+=("$2") ;;
            esac
            shift 2 ;;
        *) printf 'Unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[[ $rounds =~ ^[1-9][0-9]*$ && $warmups =~ ^[0-9]+$ && $timeout =~ ^[1-9][0-9]*([.][0-9]+)?$ ]] || { printf 'Invalid rounds/warmups/timeout\n' >&2; exit 2; }
[[ -x $baseline && -x $current ]] || { printf 'Both frozen binaries must exist before running\n' >&2; exit 2; }
if cmp -s "$baseline" "$current"; then
    printf 'Baseline and current binaries are byte-identical; freeze the intended distinct builds before measuring (Cargo target: target/release/luanalysis-cli).\n' >&2
    exit 2
fi
baseline=$(cd -- "$(dirname -- "$baseline")" && printf '%s/%s\n' "$PWD" "$(basename -- "$baseline")")
current=$(cd -- "$(dirname -- "$current")" && printf '%s/%s\n' "$PWD" "$(basename -- "$current")")
repo=$(cd -- "$repo" && pwd -P)
all_workloads=(luals-j1 luals-j10 ge-tts-j1 ge-tts-j10 boolean128 boolean256 predicate128 predicate256 environment128 environment256 environment-bare128 environment-bare256)
if ((${#selected[@]} == 0)); then selected=("${all_workloads[@]}"); fi
need_lua=false
need_pi=false
for workload in "${selected[@]}"; do
    found=false
    for candidate in "${all_workloads[@]}"; do [[ $candidate != "$workload" ]] || found=true; done
    $found || { printf 'Unknown workload: %s\n' "$workload" >&2; exit 2; }
    case "$workload" in luals-*) need_lua=true ;; ge-tts-*) need_pi=true ;; esac
done
if $need_lua; then
    [[ -d $lua/script && -d $lua/test ]] || { printf 'LuaLS workloads require --luals-root\n' >&2; exit 2; }
    lua=$(cd -- "$lua" && pwd -P)
fi
if $need_pi; then
    [[ -d $pi/lib/ge_tts && -d $pi/lib/tts-types ]] || { printf 'ge_tts workloads require --prototype-root\n' >&2; exit 2; }
    pi=$(cd -- "$pi" && pwd -P)
fi
if [[ -z $output ]]; then output=$(mktemp -d "${TMPDIR:-/tmp}/luanalysis-predicate-benchmark.XXXXXX"); rmdir -- "$output"; fi
[[ ! -e $output ]] || { printf 'Output must be a new directory: %s\n' "$output" >&2; exit 2; }
mkdir -p -- "$output/runs"
output=$(cd -- "$output" && pwd -P)
helper=$harness/metrics.py
python_bin=${PYTHON_BIN:-python3}
command -v "$python_bin" >/dev/null
workloads=$output/workloads
bash "$harness/generate-workloads.sh" "$workloads"
source_paths=("$workloads")
if $need_lua; then source_paths+=("$lua/script" "$lua/test" "$lua/tools" "$lua/make.lua" "$lua/main.lua" "$lua/debugger.lua" "$lua/test.lua"); fi
if $need_pi; then source_paths+=("$pi/lib/ge_tts" "$pi/lib/tts-types"); fi
cd -- "$repo"

snapshot_environment() {
    local phase=$1
    {
        date -u '+UTC %Y-%m-%dT%H:%M:%SZ'
        uname -a
        /usr/bin/sw_vers 2>/dev/null || true
        /usr/sbin/sysctl hw.ncpu hw.physicalcpu hw.memsize machdep.cpu.brand_string 2>/dev/null || true
        uptime
        printf 'LC_ALL=%s\nLANG=%s\nLUANALYSIS_PROFILE=unset\n' "$LC_ALL" "$LANG"
        for variable in RAYON_NUM_THREADS RUSTFLAGS CARGO_BUILD_JOBS CARGO_PROFILE_RELEASE_LTO CARGO_PROFILE_RELEASE_CODEGEN_UNITS; do
            if [[ -n ${!variable-} ]]; then printf '%s=%s\n' "$variable" "${!variable}"; fi
        done
    } > "$output/environment-$phase.txt"
    ps -Ao pid,ppid,pcpu,pmem,comm > "$output/process-load-$phase.txt"
    /usr/bin/vm_stat > "$output/vm-stat-$phase.txt" 2>&1 || true
}
snapshot_git() {
    local label=$1 directory=$2 phase=$3
    git -C "$directory" rev-parse HEAD > "$output/$label-$phase.commit.txt"
    git -C "$directory" status --porcelain=v1 > "$output/$label-$phase.status.txt"
}
snapshot_inputs() {
    local phase=$1
    snapshot_environment "$phase"
    snapshot_git checker "$repo" "$phase"
    if $need_lua; then snapshot_git luals "$lua" "$phase"; fi
    if $need_pi; then
        snapshot_git prototype "$pi" "$phase"
        snapshot_git ge-tts "$pi/lib/ge_tts" "$phase"
        snapshot_git tts-types "$pi/lib/tts-types" "$phase"
    fi
    "$python_bin" "$helper" inventory --output "$output/sources-$phase.json" "${source_paths[@]}"
    shasum -a 256 "$baseline" "$current" > "$output/binaries.$phase.sha256"
}
snapshot_inputs before
git -C "$repo" diff --binary HEAD > "$output/checker.patch"
shasum -a 256 "$repo/Cargo.lock" "$helper" "$harness/bench.sh" "$harness/generate-workloads.sh" > "$output/harness-and-lock.sha256"
printf 'rounds=%s\nwarmups=%s\ntimeout=%s\nbaseline=%s\ncurrent=%s\ncwd=%s\nluals=%s\nprototype=%s\n' "$rounds" "$warmups" "$timeout" "$baseline" "$current" "$repo" "$lua" "$pi" > "$output/configuration.txt"
printf '%s\n' "${selected[@]}" > "$output/workloads.txt"

run_one() {
    local workload=$1 variant=$2 phase=$3 round=$4 order=$5 checker jobs
    local command=()
    if [[ $variant == baseline ]]; then checker=$baseline; else checker=$current; fi
    case "$workload" in
        luals-j*)
            jobs=${workload#luals-j}
            command=("$checker" check --format json --lua-version 5.4 --jobs "$jobs"
                --module-root "$lua/script" --module-root "$lua/test" --package-path '?.lua;?/init.lua'
                "$lua/script" "$lua/test" "$lua/tools" "$lua/make.lua" "$lua/main.lua" "$lua/debugger.lua" "$lua/test.lua") ;;
        ge-tts-j*)
            jobs=${workload#ge-tts-j}
            command=("$checker" check --format json --lua-version 5.2 --jobs "$jobs"
                --library "$pi/lib/tts-types" --module-root "$pi/lib" --package-path '?.ttslua;?/init.ttslua;?.lua;?/init.lua'
                "$pi/lib/ge_tts") ;;
        boolean*|predicate*|environment*)
            command=("$checker" check --format json --lua-version 5.4 --jobs 1 "$workloads/$workload.lua") ;;
    esac
    "$python_bin" "$helper" run --prefix "$output/runs/$workload/$variant-$phase$round" \
        --workload "$workload" --variant "$variant" --phase "$phase" --round "$round" --order "$order" --timeout "$timeout" -- "${command[@]}"
}
trap 'printf "Interrupted; completed run records remain in %s\n" "$output" >&2; exit 130' INT
trap 'printf "Terminated; completed run records remain in %s\n" "$output" >&2; exit 143' TERM
for workload in "${selected[@]}"; do
    if [[ $workload == predicate* ]]; then variants=(current); else variants=(baseline current); fi
    for ((round=1; round<=warmups; round++)); do
        order=0
        for variant in "${variants[@]}"; do order=$((order+1)); run_one "$workload" "$variant" warmup "$round" "$order"; done
    done
    for ((round=1; round<=rounds; round++)); do
        if [[ $workload == predicate* ]]; then pair=(current)
        elif ((round % 2)); then pair=(baseline current)
        else pair=(current baseline)
        fi
        order=0
        for variant in "${pair[@]}"; do order=$((order+1)); run_one "$workload" "$variant" measure "$round" "$order"; done
    done
done
snapshot_inputs after
cmp "$output/sources-before.json" "$output/sources-after.json" > "$output/source-stability.txt" || { printf 'Corpus changed during measurements\n' >&2; exit 1; }
cmp "$output/binaries.before.sha256" "$output/binaries.after.sha256" > "$output/binary-stability.txt" || { printf 'Binary changed during measurements\n' >&2; exit 1; }
"$python_bin" "$helper" summarize --output "$output" | tee "$output/summary.txt"
printf 'Evidence: %s\n' "$output"
