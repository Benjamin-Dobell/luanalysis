# IntelliJ regression corpus

The 70 files under `resources/` are byte-for-byte Lua resources from
[Benjamin-Dobell/IntelliJ-Luanalysis](https://github.com/Benjamin-Dobell/IntelliJ-Luanalysis)
at commit `d796bb67d5135d152b9f627b69e57377c83d1a66`, originally under
`src/test/resources/`. No Kotlin files are copied. The 27 original Kotlin
sources are read from Git history solely to extract tests into Lua files and
Rust assertions. `UPSTREAM-SOURCES.tsv` identifies those extraction sources and
hashes; `NOTICE.txt` retains their copyright/attribution notices. The upstream
Apache 2.0 license, including its Lua reference manual notice, is in
[`../fixtures/LICENSE.txt`](../fixtures/LICENSE.txt).

`embedded/` contains Lua extracted from the Kotlin tests. This adaptation
preserves Kotlin raw strings, applies `trimIndent` only where the old harness
applied it (including significant blank-line indentation), and splits FileTree
inputs into the original separate files. `IDE-INPUTS.tsv` also retains the six
editor/refactoring methods' input and expected files, actions and settings. It
retains every diagnostic and caret marker. Extraction attribution (original
Kotlin path, method and line), input groups, Lua versions and checking settings
are recorded in `cases.tsv`. `inventory.tsv` accounts for all 103 old test
methods, including IDE-only and unsupported operations. `unused-resources.tsv`
accounts for resources the old automated harness did not reference, and the
before/after file-move inputs exercised separately by native file_operations
tests. Merely preserving an input in that inventory does not count as a passing
native test.

`SHA256SUMS.tsv` records the 70 Lua resource source paths and SHA-256 hashes;
`UPSTREAM-SOURCES.tsv` records extraction-source hashes without retaining Kotlin.
Reproduce or verify the import without reading or changing the source working
tree:

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

See `README.txt` for extraction verification, regression tests and the complete
compatibility audit command. Audit reports are local development output.

The native editor-feature regressions also replay the original CodeStyleTest
before/after buffers in `embedded/CodeStyleTest/`, the original RenameTest
before/after files in `embedded/RenameTest/`, and the file-move resource tree.
Their input bytes and expected output bytes are preserved, including empty files
and absent final newlines. `NOTICE.txt` retains the extraction attribution;
`crates/luanalysis/tests/formatting.rs` and the `file_operations` unit tests apply
actual native edits and verify the original outcomes. These do not count the
original IDE actions as runnable cases in the 99-case audit driver.
