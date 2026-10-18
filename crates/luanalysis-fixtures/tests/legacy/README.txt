The complete legacy regression audit
====================================

Source: IntelliJ-Luanalysis d796bb67d5135d152b9f627b69e57377c83d1a66.
See ORIGIN.md for attribution, reproduction, and the exact range adapters.

Run from the standalone repository root:

  scripts/cargo.sh test --locked -j2 -p luanalysis-fixtures -- --test-threads=2

This runs 50 ported cases that currently satisfy the original expectations, eight
harness adaptation/inventory regressions, the 13 marker-reader unit tests, and the
six existing compatibility/correction tests. There are no ignored tests.

To run EVERY runnable case, including the failing compatibility cases:

  scripts/cargo.sh run --locked -j2 -p luanalysis-fixtures --example legacy_suite -- \
    --report /tmp/luanalysis-legacy-results.txt

The full audit exits 1 while any case mismatches, crashes, times out, or a
non-IDE operation remains unavailable. It never treats the recorded failures as
accepted expectations. --case substring isolates a case/family; --timeout-seconds
N changes its 30-second process limit. Each case runs in a separate process with
an isolated temporary project; the parent removes its files even after timeout
or crash. Siblings share the standard Cargo build directory. For an isolated
Git worktree, set CARGO_TARGET_DIR to the main checkout's target directory.

Latest full result (2026-10-19): 99 cases executed; 50 matched, 49 mismatched;
zero crashed or timed out. Passing: 24 diagnostic, 24 completion and two
language-level acceptance cases. Mismatched: 38 diagnostic, eight completion,
two language-level rejection cases and one document-symbol hierarchy case.
The 858 checked diagnostic expectations include intentionally separate tests of
the same resource in different project configurations; this is not 858 distinct
source markers. RESULTS.txt contains every missing/unexpected diagnostic with
original file, line, column, span, code, snippet and old description. AUDIT.tsv
classifies each case and records the investigation, including unresolved defects.

There are 103 old test methods, expanded into 99 runnable cases because two
completion methods each perform two requests. Four methods test IntelliJ-only
lookup classes, formatting/settings, or the editor Enter action. Two file
move/rename methods require workspace file-operation edits, which the native
server does not implement; native symbol Rename is a different operation.
All six are explicit inventory-only entries, never passing cases. The original
six before/after move files are retained, as are three unreferenced resources
(including the prose-comment-only typeSafety.lua).

What was already covered
------------------------

Before this import, compatibility.rs ran four full resource inputs: local
assignment, lambda parameters, trailing type annotations and completion-hint
checking. Their correction tests remain. Three additional primitive reductions
are not complete imports. Existing native tests also cover reductions or
related contracts, including native_indexers.rs, native_result_packs.rs,
v1_field_literals.rs, structural_width.rs, native_generic_metadata.rs,
native_generic_self.rs and the LSP integration tests. Those tests remain useful,
but were not credited as running the old complete files or embedded test cases.

The new complete import adds all 70 resource Lua files and 27 Kotlin sources,
extracts the embedded cases, and supplies the multi-file project and native
language-service host. Fifty complete cases now have normal Cargo regressions;
46 of those were not among the previous four complete imports. The full audit
still exercises all other cases and returns failure, rather than concealing them
behind ignored tests, reduced fixtures, or changed expected messages.

Fixes and remaining compatibility
---------------------------------

Two native bugs exposed by the complete inputs were repaired:

* Completion's ASCII-only identifier filter hid valid Unicode names. It now uses
  the Lua lexer, and partially typed Unicode identifiers are replaced in full.
  The original Chinese fixture passes; the stdio LSP test also checks UTF-16
  positions and edits. Invalid names and keywords are still excluded.
* A class annotation on an uninitialized single local was wrongly classified as
  runtime class construction. It now remains a declaration. The complete
  generic-class covariance fixture matches all nine negative cases without
  unsupported errors. Actual initialized generic class templates still diagnose
  their genuine unsupported capability.

The remaining differences are NOT all intentional. AUDIT.tsv distinguishes
confirmed native policies (flow narrowing, repeated-class merging, overload
implementation validation), reporting differences, source defects, missing
capabilities and unresolved regressions. In particular, later inferred global
fields/methods, indexed class-owner method writes, numeric-string unary
coercion, and a callback/recursive-alias negative case still need repairs.
Missing capabilities include old module()/@module ownership, generic runtime
class templates, discarded-result inspections, strict/permissive unknown and
non-strict nil modes, language-version syntax gates, and several IDE completion
providers. The old malformed annotations and misspelled pair() call were kept
unchanged; accepting them silently would be an inappropriate parser change.
There were no standard-library or ge_tts changes.

Experiments and verification
----------------------------

The first serial inspection probe loaded complete cases using Lua53/Lua51
standard declarations and LegacyExposeImplementation. It showed that the small
existing reductions substantially overestimated compatibility. A type-describing
probe spent over two minutes on self.lua; it was terminated, and that temporary
probe source/executable was removed. The final diagnostic runner follows check's
normal describe_types=false path and completes self.lua. This does not establish
that --dump-types performance on that input has been repaired.

The shared Cargo directory initially supplied stale engine/syntax artifacts from
another worktree. Compilation then failed with missing type-predicate APIs.
Touching the source roots rebuilt the correct committed dependencies in the same
shared target directory; no second build cache or vendored source edit was made.

Integration against current main (5effe6c, including native Rename) passed 105
native unit tests and 1,451 native integration tests. The final fixture target
passed 13 marker-reader tests, six existing compatibility tests and 58 added
tests (50 corpus cases plus eight harness checks). The full 99-case audit above
remains deliberately nonzero. Run the commands above for current results; the
committed RESULTS.txt is an observation, not an oracle.
