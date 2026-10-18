# Predicate benchmark harness

Run from a checkout with Bash, Python 3 and two frozen release binaries:

```bash
bash scripts/benchmark-type-predicates/bench.sh \
  --baseline /path/to/baseline-release \
  --current /path/to/current-release \
  --luals-root /path/to/lua-language-server-3.19.2 \
  --prototype-root /path/to/prototype_inertia \
  --output /path/to/new-evidence-directory
```

The script does not build binaries or modify checker/corpus sources. Complete
builds and tests before measuring. See `--help` for environment-variable
equivalents and optional settings. Corpus roots are needed only for their
selected workloads; for a synthetic-only comparison, pass `--workload
boolean128 --workload predicate128`. `PYTHON_BIN` selects the Python executable.

The defaults are one warmup and five measured rounds per variant, with a
180-second process timeout. Paired workloads run baseline/current, then
current/baseline, alternating for five rounds. Predicate syntax runs current
only because the baseline cannot parse it. `--quick` selects one measured round
and a 60-second timeout for investigation.

`generate-workloads.sh` creates 128/256-function controls, each with sixteen
guard conditions per function and three parameter reads in each branch. The
predicate copies change only the guard's return annotation. Both branches are
valid without narrowing; functional regressions separately verify narrowed APIs.

`metrics.py` measures fresh CLI processes with monotonic wall time and
`os.wait4` child resource usage. It reports user/system/total CPU time and peak
RSS in bytes, preserving the platform's native RSS unit in each record. It uses
only the Python standard library. The measurement helper requires Unix process
APIs; the retained comparison was run on macOS.

Evidence includes exact command arrays, raw stdout/stderr, per-process metrics,
binary hashes, source inventories, Git revisions/status, the checker's tracked
working-tree patch, environment/load snapshots and complete report digests.
Canonical JSON preserves all report fields, paths and array order; it changes
only object-key order and insignificant whitespace. Nonzero diagnostic results
are valid check outcomes. Summary checks separately flag malformed reports,
timeouts, nonempty stderr, nondeterminism and paired report differences. Inspect
`results.json`'s `checks` before accepting a timing comparison; a completed script
alone does not establish report parity.

`summary.tsv` contains measured medians; warmups remain in the evidence but are
excluded from those medians. `results.json` also retains every sample, ranges,
paired ratios, current predicate/boolean cost ratios and 256/128 scaling ratios.
No CPU, RSS or timing threshold is hardcoded. Source/binary changes during the
measurement abort the comparison and preserve the evidence.

The [performance report](../../docs/TYPE-PREDICATES-PERFORMANCE.md) pins the
baseline, corpus revisions, build method and interpretation.
