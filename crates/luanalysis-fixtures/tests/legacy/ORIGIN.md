# IntelliJ regression corpus

All files under `resources/` and `kotlin/` are byte-for-byte copies from
[Benjamin-Dobell/IntelliJ-Luanalysis](https://github.com/Benjamin-Dobell/IntelliJ-Luanalysis)
at commit `d796bb67d5135d152b9f627b69e57377c83d1a66`, originally under
`src/test/`. This is 70 Lua files and 27 Kotlin files. The original copyright
and attribution notices are retained in those files. The upstream Apache 2.0
license, including its Lua reference manual notice, is in
[`../fixtures/LICENSE.txt`](../fixtures/LICENSE.txt).

`embedded/` contains Lua extracted from the Kotlin tests. This adaptation
preserves Kotlin raw strings, applies `trimIndent` only where the old harness
applied it, and splits FileTree inputs into the original separate files. It
retains every diagnostic and caret marker. Extraction attribution (original
Kotlin path, method and line), input groups, Lua versions and checking settings
are recorded in `cases.tsv`. `inventory.tsv` accounts for all 103 old test
methods, including IDE-only and unsupported operations. `unused-resources.tsv`
accounts for resources the old automated harness did not reference, and the
before/after file-move inputs whose operation is unavailable. Nothing in that
inventory is counted as a passing native test.

`SHA256SUMS.tsv` records original Git source paths and SHA-256 hashes. Reproduce
or verify the import without reading or changing the source working tree:

```bash
python3 scripts/import-legacy-tests.py /path/to/IntelliJ-Luanalysis
python3 scripts/import-legacy-tests.py /path/to/IntelliJ-Luanalysis --verify
```

Run these commands from the standalone repository root. The importer reads Git
objects at the pinned revision, including when the source files are staged for
deletion. It never restores, resets, stages or commits anything in that checkout.

The existing marker reader remains the only diagnostic markup parser. Native
adaptations live in `tests/support/legacy.rs`: semantic diagnostic codes replace
obsolete message text; undeclared-variable warnings correspond to native
`TS2304` errors; whole PSI alias/inheritance, return-body and member-access
ranges translate to exact syntax-derived native name, base, return annotation,
member or index ranges. These translations do not consult observed diagnostics.
Every marker still requires a separate diagnostic, and unexpected diagnostics
in positive code fail the case. Whole-record diagnostic aggregation, new type
semantics and missing capabilities remain visible mismatches. Unsupported
feature and parse errors never satisfy a legacy type-error expectation.

The four previously imported complete fixtures (`local_def_assignment.lua`,
`lambda_params.lua`, `trailing_type.lua`, `snippet.lua`) now have one canonical
copy here. Their existing correction tests still run. The three old primitive
reductions remain in `../fixtures/` and are not credited as complete imports.

See `README.txt`, `AUDIT.tsv`, and `RESULTS.txt` for commands, investigation and
observed results. `RESULTS.txt` is a report, never an accepted-output oracle.

The native editor-feature regressions also replay the original CodeStyleTest
before/after buffers in `embedded/CodeStyleTest/`, the original RenameTest
before/after files in `embedded/RenameTest/`, and the file-move resource tree.
Their input bytes and expected output bytes are preserved, including empty files
and absent final newlines. `NOTICE.txt` retains the extraction attribution;
`crates/luanalysis/tests/formatting.rs` and the `file_operations` unit tests apply
actual native edits and verify the original outcomes. These do not count the
original IDE actions as runnable cases in the 99-case audit driver.
