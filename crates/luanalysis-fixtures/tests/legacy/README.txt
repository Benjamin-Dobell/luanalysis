Legacy regression corpus
========================

Lua test inputs and assertions are extracted from the pinned IntelliJ-Luanalysis
suite. No Kotlin source files are retained. ORIGIN.md documents attribution and
extraction; cases.tsv records source groups, checking settings and assertions.
inventory.tsv accounts for every upstream method. IDE-INPUTS.tsv preserves the
input/expected files, actions and settings for editor/refactoring operations.
Native formatting and file_operations regressions replay the original formatter
and file rename/move outcomes separately from the audit driver. Editor Enter
and IntelliJ lookup-element identity remain client-specific operations.

Run the maintained regression tests:

  cargo test --locked -j2 -p luanalysis-fixtures -- --test-threads=2

Exercise every original case and write a local compatibility report:

  cargo run --locked -j2 -p luanalysis-fixtures --example legacy_suite -- \
    --report /tmp/luanalysis-legacy-results.txt

The audit exits nonzero for mismatches or unsupported non-IDE operations.
--case substring selects cases; --timeout-seconds N changes the per-case limit.
Each case runs in a separate process and temporary project with its original
companions and Lua standard profile. Original markers are preserved; generated
reports are development output, not accepted-output test oracles.

Verify the extraction against the pinned upstream Git history:

  python3 scripts/import-legacy-tests.py /path/to/IntelliJ-Luanalysis --verify

For an isolated worktree, set CARGO_TARGET_DIR to the main checkout's shared
target directory. The importer reads Git objects without changing the upstream
working tree.
