#!/usr/bin/env bash
# Reproduce the pinned Bun driver's pure task planner without building Rust.
set -euo pipefail

bun_root=
write=false
while (($#)); do
    case "$1" in
        --bun) bun_root=${2:?--bun requires a path}; shift 2 ;;
        --write) write=true; shift ;;
        *) printf 'Usage: %s --bun /path/to/bun [--write]\n' "$0" >&2; exit 2 ;;
    esac
done
if [[ -z $bun_root ]]; then
    printf '%s\n' '--bun /path/to/bun is required' >&2
    exit 2
fi

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
source_path=src/sema/driver/lib.rs
source_file=$bun_root/$source_path
source_sha=cc20dc8c2ac91a8dfc9c8098421f823b0a84a491d9e26819daef5f9843df504a
output=$root/crates/luanalysis-engine/src/native_plan.rs
manifest=$root/crates/luanalysis-engine/bun-plan-provenance.json
hash_file() { shasum -a 256 "$1" | awk '{print $1}'; }
if [[ $(hash_file "$source_file") != "$source_sha" ]]; then
    printf '%s\n' 'Bun driver differs from the pinned original; extraction refused' >&2
    exit 1
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/luanalysis-bun-plan.XXXXXX")
cleanup() {
    rm -f -- "$scratch/defaults" "$scratch/plan.raw" "$scratch/plan" "$scratch/order.raw" "$scratch/order" "$scratch/output" "$scratch/manifest"
    rmdir -- "$scratch"
}
trap cleanup EXIT

for field in step_growth warm_up_files warm_up_max_bytes chunk_bytes min_tasks type_node_cost split_files tasks_on_one_thread one_thread_task_cost one_thread_max_cost; do
    value=$(awk -v field="$field" '$0 ~ "^            " field ": " { print; exit }' "$source_file")
    if [[ -z $value ]]; then
        printf 'Missing pinned default: %s\n' "$field" >&2
        exit 1
    fi
    printf '%s\n' "$value" >> "$scratch/defaults"
done

perl -0777 -ne 'm/(    \/\/\/ The tasks of one thread for `files`.*?)(?=\n\/\/\/ The progress of a check)/s or die "missing pinned plan methods\n"; print "impl Plan {\n$1";' "$source_file" > "$scratch/plan.raw"
perl -pe 's/\bPlanOptions\b/NativePlanOptions/g; s/\bPlan\b/NativeCheckPlan/g; s/\bTask\b/NativeCheckTask/g; s/\bAhead\b/NativeCheckAhead/g; s/^    fn (new|ahead_of|cut)\(/    pub fn $1(/;' "$scratch/plan.raw" > "$scratch/plan"
perl -0777 -ne 'm/let run_round =.*?(            let tasks = step\.len\(\);\n.*?)(?=            let outcomes:)/s or die "missing pinned task start order\n"; print $1;' "$source_file" > "$scratch/order.raw"
perl -pe 's/^    //; s/\.shared_sort_by_key\(/.sort_by_key(/g; s/Reverse\(weight_of\(i\)\)/std::cmp::Reverse(weight_of(i))/g; s/size_of\(it\)/cost_of(it)/g; s/request\.order/order/g;' "$scratch/order.raw" > "$scratch/order"

{
cat <<'RUST'
//! The pinned Bun driver's deterministic source/cost task plan.
//!
//! Reproduce with `scripts/extract-bun-plan.sh --bun /path/to/bun --write`.
//! Upstream: 408c9374b266b45fe6b8260e09be73aa82a957cd, src/sema/driver/lib.rs.
//! Source SHA-256: cc20dc8c2ac91a8dfc9c8098421f823b0a84a491d9e26819daef5f9843df504a.
//! Bun's license is retained in vendor/bun/LICENSE.md. The planner body is
//! extracted unchanged apart from public names and visibility; the source host
//! supplies Lua byte/cost estimates and supported declaration ranges.

/// Consecutive original files checked by one Checker and provider/task.
pub type NativeCheckTask = Vec<usize>;
/// An upstream declaration statement-range task, if the source host supports it.
pub type NativeCheckAhead = Option<(u32, u32)>;

/// The upstream options read by this pure planner or its caller's plan choice.
/// Other driver options are deliberately outside this extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativePlanOptions {
    pub step_growth: usize,
    pub warm_up_files: usize,
    pub warm_up_max_bytes: usize,
    pub chunk_bytes: usize,
    pub min_tasks: usize,
    pub type_node_cost: usize,
    pub split_files: u32,
    pub tasks_on_one_thread: bool,
    pub one_thread_task_cost: usize,
    pub one_thread_max_cost: usize,
}

impl Default for NativePlanOptions {
    fn default() -> Self {
        Self {
RUST
cat "$scratch/defaults"
cat <<'RUST'
        }
    }
}

/// Task/publication order depends on source metadata, never completion time or
/// worker count. `one_thread` deliberately uses Bun's different serial plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCheckPlan {
    pub steps: Vec<Vec<NativeCheckTask>>,
    ahead: Vec<NativeCheckAhead>,
}

RUST
cat "$scratch/plan"
cat <<'RUST'

impl NativePlanOptions {
    /// The exact choice made by the upstream driver before constructing a plan.
    /// `total_cost` also includes any sources that will only be emitted there.
    pub fn uses_one_thread_plan(&self, threads: usize, total_cost: usize) -> bool {
        threads == 1 && !self.tasks_on_one_thread && total_cost <= self.one_thread_max_cost
    }
}

impl NativeCheckPlan {
    /// The upstream one-thread plan, using one Checker per returned task.
    pub fn one_thread(count: usize, cost_of: &dyn Fn(usize) -> usize, options: &NativePlanOptions) -> Self {
        let tasks = Self::of_one_thread((0..count).collect(), cost_of, options);
        Self { steps: if tasks.is_empty() { Vec::new() } else { vec![tasks] }, ahead: Vec::new() }
    }

    /// Original task indices in the upstream execution order. Publication MUST
    /// still use the original indices in `steps`, regardless of completion order.
    pub fn start_order(&self, number: usize, cost_of: &dyn Fn(usize) -> usize, order: u32, is_one_thread: bool) -> Vec<usize> {
        let step = &self.steps[number];
RUST
cat "$scratch/order"
cat <<'RUST'
        start_order
    }
}
RUST
} > "$scratch/output"

plan_sha=$(hash_file "$scratch/plan.raw")
order_sha=$(hash_file "$scratch/order.raw")
output_sha=$(hash_file "$scratch/output")
cat > "$scratch/manifest" <<JSON
{
  "upstream_revision": "408c9374b266b45fe6b8260e09be73aa82a957cd",
  "source_path": "$source_path",
  "source_sha256": "$source_sha",
  "fragments": {
    "plan_methods": "$plan_sha",
    "task_start_order": "$order_sha"
  },
  "adaptations": [
    "Rename Plan/PlanOptions/Task/Ahead into the native public namespace.",
    "Expose new/ahead_of/cut; retain only relevant upstream options/defaults.",
    "Provide a one-thread constructor and the unchanged driver plan-choice predicate.",
    "Use std stable sort_by_key for coordinator start order instead of SharedSort.",
    "Pass start-order cost and order as explicit arguments instead of driver captures."
  ],
  "output_path": "crates/luanalysis-engine/src/native_plan.rs",
  "output_sha256": "$output_sha"
}
JSON

if $write; then
    cp "$scratch/output" "$output"
    cp "$scratch/manifest" "$manifest"
    printf '%s\n' 'Wrote pinned Bun plan and provenance'
else
    cmp -- "$scratch/output" "$output"
    cmp -- "$scratch/manifest" "$manifest"
    printf '%s\n' 'Verified pinned Bun plan and provenance'
fi
