#!/usr/bin/env python3
"""Extract the pinned parser-independent bun_sema dependency closure.

Rust source adaptations are explicit, hash-verified patches. Cargo manifests
lose development-only edges and standalone binary/test targets so the extracted
libraries do not pull in Bun's parser or runtime. The upstream checkout is
read-only.
"""

import argparse
import hashlib
import json
import re
import subprocess
import tempfile
import tomllib
from pathlib import Path

REVISION = "408c9374b266b45fe6b8260e09be73aa82a957cd"
ROOT = Path(__file__).resolve().parent.parent
SOURCE_ADAPTATIONS = [{
    "path": "src/sema/check/flow.rs",
    "patch": "vendor/patches/bun-sema-assignment-visibility.patch",
    "reason": "Expose the existing type-only assignment transfer to the Lua adapter; body unchanged.",
}, {
    "path": "src/sema/check/call.rs",
    "patch": "vendor/patches/bun-sema-resolved-calls.patch",
    "reason": "Add a scalar resolved-type call boundary using native signature, arity, argument-relation and return workers.",
}, {
    "path": "src/sema/check/errors_call.rs",
    "patch": "vendor/patches/bun-sema-resolved-calls.patch",
    "reason": "Share the native applicability relation step between TS calls and resolved-type calls.",
}, {
    "path": "src/sema/check/mod.rs",
    "patch": "vendor/patches/bun-sema-resolved-calls.patch",
    "reason": "Export the resolved-type call result and error records.",
}, {
    "path": "src/sema/check/errors_flow.rs",
    "patch": "vendor/patches/bun-sema-return-policy.patch",
    "reason": "Expose the native empty-function return policy and share its unchanged predicate with existing TS checks.",
}]
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-signatures.patch",
    "reason": "Add source-identified lazy native signatures, provider dispatch and shared return query/aggregation support.",
} for path in [
    "check/call.rs", "check/context.rs", "check/decl.rs", "check/explain_relation.rs",
    "check/infer.rs", "check/instantiate.rs", "check/mod.rs", "check/print.rs",
    "check/relate.rs", "check/shape.rs", "check/symbols.rs", "types.rs",
])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-type-parameters.patch",
    "reason": "Add source-identified native generic parameters using shared constraint/default, relation and sibling-clone mapper workers.",
} for path in [
    "check/call.rs", "check/decl.rs", "check/infer.rs", "check/instantiate.rs",
    "check/mapped.rs", "check/mod.rs", "check/print.rs", "check/relate.rs",
    "check/shape.rs", "check/unions.rs", "types.rs",
])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-generic-calls.patch",
    "reason": "Route resolved native generic calls through shared overload selection, type argument inference, defaults, constraints and signature instantiation.",
} for path in ["call.rs", "errors_call.rs"])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-contextual-expressions.patch",
    "reason": "Add original-source expression dispatch and contextual signature facets using shared call inference and scoped provider contexts.",
} for path in [
    "types.rs", "check/call.rs", "check/context.rs", "check/decl.rs", "check/errors_call.rs",
    "check/errors_jsx.rs", "check/explain_relation.rs", "check/expr.rs", "check/infer.rs",
    "check/instantiate.rs", "check/mod.rs", "check/print.rs", "check/relate.rs",
    "check/shape.rs", "check/symbols.rs",
])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-flow.patch",
    "reason": "Extract shared TS/native flow traversal and add original-source graph providers with native loop, narrowing and reachability workers.",
    **({"added": True} if path in {"flow_walk.rs", "native_flow.rs"} else {}),
} for path in ["flow.rs", "flow_walk.rs", "native_flow.rs", "mod.rs"])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-captured-environments.patch",
    "reason": "Add immutable native binding environments, explicit declaration/checked type origins, and scoped capture/context instantiation using shared signature caches and mappers.",
} for path in [
    "types.rs", "check/call.rs", "check/context.rs", "check/decl.rs",
    "check/explain_relation.rs", "check/infer.rs", "check/instantiate.rs", "check/mod.rs",
    "check/print.rs", "check/relate.rs", "check/shape.rs", "check/symbols.rs",
])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-runtime-predicates.patch",
    "reason": "Add original-source primitive runtime predicates using the native typeof narrowing worker and shared flow/reachability traversal.",
} for path in ["flow.rs", "native_flow.rs"])
SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_flow.rs",
    "patch": "vendor/patches/bun-sema-numeric-for.patch",
    "reason": "Add original-source numeric iteration facts and reachability with exact Lua numeric-entry evaluation within the shared flow worker.",
})
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-lua-packs-tables.patch",
    "reason": "Add native Lua table/key relations and correlated result packs using intrinsic tuple/rest storage, shared signature caches, mappers and call inference workers.",
    **({"added": True} if path in {"check/native_packs.rs", "check/native_tables.rs"} else {}),
} for path in [
    "check/call.rs", "check/decl.rs", "check/explain_relation.rs", "check/infer.rs",
    "check/instantiate.rs", "check/mod.rs", "check/print.rs", "check/relate.rs",
    "check/shape.rs", "check/symbols.rs", "types.rs", "check/native_packs.rs",
    "check/native_tables.rs",
])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-lua-pack-calls-flow-sequences.patch",
    "reason": "Add lazy final-pack arguments through shared call/rest inference, whole-pack flow projections and predicates, and exact ranged table sequence/collision policies independent of freshness.",
} for path in [
    "check/call.rs", "check/errors_call.rs", "check/infer.rs", "check/instantiate.rs",
    "check/mod.rs", "check/native_flow.rs", "check/native_packs.rs", "check/native_tables.rs",
    "check/print.rs", "check/relate.rs", "check/shape.rs", "types.rs",
])
SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_tables.rs",
    "patch": "vendor/patches/bun-sema-closed-table-key-coverage.patch",
    "reason": "Require an actual indexer or ranged-tail domain match before augmenting dynamic constructor lookups with fixed-field collision candidates; closed records retain complete key-domain checks.",
})


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-generic-iteration.patch",
    "reason": "Add correlated Lua generic-for hidden frames and nil-only iteration filtering through the shared call and flow workers; retain Any tail values during fixed assignment adjustment.",
    **({"added": True} if path == "native_iterators.rs" else {}),
} for path in ["mod.rs", "native_flow.rs", "native_packs.rs", "native_iterators.rs"])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-named-types.patch",
    "reason": "Add original-source lazy aliases and structural object heads with native declaration query guards, generic instantiation, member relations and inference; preserve explicit unsupported nominal and owner-context boundaries.",
    **({"added": True} if path == "check/native_named.rs" else {}),
} for path in [
    "check/check_source_file.rs", "check/infer.rs", "check/instantiate.rs", "check/mod.rs",
    "check/native_named.rs", "check/native_tables.rs", "check/print.rs", "check/relate.rs",
    "check/shape.rs", "check/sink.rs", "types.rs",
])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-scalar-return-contracts.patch",
    "reason": "Separate pure written scalar return contracts from physical result-count queries, preserve correlated annotated count recovery and actual body cardinality, and check guaranteed argument prefixes before unknown Any tails through the shared call workers.",
} for path in ["decl.rs", "infer.rs", "mod.rs", "native_packs.rs"])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-automatic-variables.patch",
    "reason": "Share native automatic assignment transfer with TS flow and resolve original-source automatic captures through the shared walker using lexical creation/write metadata, retaining opaque unresolved constraints until actual reads.",
} for path in ["flow.rs", "flow_walk.rs", "native_flow.rs", "native_iterators.rs"])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-nominal-class-origins.patch",
    "reason": "Retain original nominal class declarations through native table descriptions, mapping, lookup, inference and structural relation demands while preserving structural shape semantics and explicit unsupported inheritance and visibility boundaries.",
} for path in [
    "types.rs", "check/native_named.rs", "check/native_tables.rs", "check/relate.rs",
    "check/instantiate.rs", "check/shape.rs", "check/print.rs",
])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-inline-assertions.patch",
    "reason": "Add original-source inline assertion overlap and exclusion operations using native comparability, non-nullability, assignability and whole correlated pack storage.",
    **({"added": True} if path == "native_assertions.rs" else {}),
} for path in ["mod.rs", "native_assertions.rs"])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-source-callable-signatures.patch",
    "reason": "Add original-source callable declaration signatures and overload groups with independent runtime implementations, shared overload selection and compatibility checks, and preserved generic owner/mapper contexts.",
    **({"added": True} if path == "check/native_signatures.rs" else {}),
} for path in [
    "check/call.rs", "check/decl.rs", "check/explain_relation.rs", "check/infer.rs",
    "check/instantiate.rs", "check/mod.rs", "check/native_named.rs", "check/native_packs.rs",
    "check/native_signatures.rs", "check/print.rs", "check/relate.rs", "check/shape.rs",
    "check/symbols.rs", "types.rs",
])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-lua-runtime-top-types.patch",
    "reason": "Add distinct broad Lua table/function runtime categories with native structural carriers, recursive relation guards and original-source runtime narrowing, preserving named table origins even when callable.",
    **({"added": True} if path == "check/native_runtime.rs" else {}),
} for path in [
    "types.rs", "check/mod.rs", "check/native_runtime.rs", "check/native_flow.rs",
    "check/native_tables.rs", "check/relate.rs", "check/shape.rs", "check/print.rs",
])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-module-queries.patch",
    "reason": "Add original-source chunk/module/require queries using native query guards, cycle/provisional publication and Stored caches, preserving lexical module roots and explicit standard-loader policies.",
    **({"added": True} if path == "native_project.rs" else {}),
} for path in ["mod.rs", "context.rs", "sink.rs", "native_project.rs"])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-global-declarations.patch",
    "reason": "Add environment-qualified global and original value-declaration queries with guarded raw contracts, deterministic identity checks, widened inferred introductions and declaration-only callable groups.",
    **({"added": True} if path == "native_globals.rs" else {}),
} for path in ["mod.rs", "sink.rs", "native_signatures.rs", "native_globals.rs"])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-checked-class-definitions.patch",
    "reason": "Add attested original class definition values with partial actual views, checked initializer contexts and retained nominal origin, call heads, runtime table category and mapper identity without weakening complete instance requirements.",
} for path in [
    "types.rs", "check/native_named.rs", "check/native_tables.rs", "check/native_runtime.rs",
    "check/instantiate.rs", "check/shape.rs", "check/relate.rs", "check/print.rs", "check/infer.rs",
])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-named-inheritance.patch",
    "reason": "Resolve original declared record bases through guarded native queries, retain mapped ancestor identity and inherited member owners, and validate overrides without replacing partial definition views or complete instance requirements.",
} for path in ["mod.rs", "native_named.rs", "native_tables.rs", "relate.rs", "shape.rs"])


SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/call.rs",
    "patch": "vendor/patches/bun-sema-native-overload-failure-diagnostics.patch",
    "reason": "Report native overload failures against actual failed public heads, retaining argument-type and arity correlation rather than checking a synthesized recovery signature; shared overload selection remains unchanged.",
})


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-recovery-checkpoint.patch",
    "reason": "Record the consolidated receiver/self mapping, definition-write flow, declared field origins, primitive string namespace, trusted assertion result/continuation and fixed spread inference checkpoint; this provenance snapshot does not assert complete Lua type-checking equivalence.",
    **({"added": True} if path == "check/native_intrinsics.rs" else {}),
} for path in [
    "check/call.rs", "check/decl.rs", "check/errors_call.rs", "check/infer.rs",
    "check/instantiate.rs", "check/mod.rs", "check/native_flow.rs",
    "check/native_globals.rs", "check/native_intrinsics.rs", "check/native_named.rs",
    "check/native_packs.rs", "check/native_runtime.rs", "check/native_signatures.rs",
    "check/native_tables.rs", "check/relate.rs", "check/shape.rs", "types.rs",
])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-inherited-field-contracts.patch",
    "reason": "Preserve inherited property contracts and original member context while checking constructor-origin fields, without publishing inferred implementation fields as independent schema contracts.",
} for path in ["native_named.rs", "sink.rs"])


SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-native-semantic-self-identity.patch",
    "reason": "Keep the original semantic self identity stable across generic definition schema expansion and instantiation.",
})


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-recursive-override-classification.patch",
    "reason": "Classify recursive nominal overrides using original lazy declaration metadata, suppressing inference side effects while preserving shared call, flow and relation workers.",
} for path in ["call.rs", "flow.rs", "native_named.rs", "relate.rs"])


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-environment-value-flow.patch",
    "reason": "Preserve original runtime environment/function capabilities, guarded loop-owned call-effect provisionals and complete reachability, expose declaration-only callable contracts, and retain shared v2 overload compatibility and native table member behavior without changing TypeScript flow joins.",
} for path in [
    "check/errors_heritage.rs", "check/errors_isolated_declarations.rs",
    "check/flow.rs", "check/flow_walk.rs", "check/infer.rs", "check/instantiate.rs",
    "check/mapped.rs", "check/mod.rs", "check/native_flow.rs", "check/native_globals.rs",
    "check/native_intrinsics.rs", "check/native_named.rs", "check/native_signatures.rs",
    "check/native_tables.rs", "check/print.rs", "check/relate.rs", "check/related.rs",
    "check/services/symbols.rs", "check/shape.rs", "check/sink.rs", "types.rs",
])


SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-native-bodyless-member-declarations.patch",
    "reason": "Group pure written member overloads by their canonical original member keys while retaining each source declaration and guarded signature demand.",
})


SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-native-member-overload-visibility.patch",
    "reason": "Require matching source visibility before grouping pure written member overloads.",
})


SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-void-callback-results.patch",
    "reason": "Preserve TypeScript void callback variance for native signatures with an empty declared result row, excluding Never and preserving ordinary physical result-pack assignment.",
} for path in ["native_packs.rs", "relate.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_intrinsics.rs",
    "patch": "vendor/patches/bun-sema-setmetatable-provenance.patch",
    "reason": "Carry selected original setmetatable value identity without replacing its declared call signature or adding assertion effects.",
})

SOURCE_ADAPTATIONS.extend({
    "path": path,
    "patch": "vendor/patches/bun-sema-native-lexical-parent-capabilities.patch",
    "reason": "Preserve exact lexical creation signature provenance through mapping and cloning without treating parent provenance as a semantic inference dependency.",
} for path in [
    "src/sema/check/call.rs",
    "src/sema/check/context.rs",
    "src/sema/check/instantiate.rs",
    "src/sema/check/relate.rs",
    "src/sema/types.rs",
])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-union-table-reads.patch",
    "reason": "Preserve Lua nil for absent union table members and filter original parent references using existing native flow truthiness and equality workers.",
} for path in ["native_tables.rs", "native_flow.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-native-fixed-alias-method-owners.patch",
    "reason": "Prepare canonical original owner scopes before publication and apply fixed alias augmentations only to matching native argument mappings.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-caller-declaration-frames.patch",
    "reason": "Preserve exact caller capabilities around delayed original arguments and establish canonical declaration frames for ordinary native signature provider callbacks.",
} for path in ["call.rs", "decl.rs", "native_packs.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-method-variance.patch",
    "reason": "Preserve original method bivariance while keeping Lua private overload implementations contravariant and constructing complete guarded native schemas before override validation.",
} for path in ["infer.rs", "mod.rs", "native_named.rs", "native_tables.rs", "relate.rs", "shape.rs", "sink.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-complete-provider-frames.patch",
    "reason": "Retain pure original lexical parent chains in canonical signature frames and isolate both contextual and captured capabilities for every native provider callback.",
} for path in ["call.rs", "decl.rs", "mod.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-runtime-schema-origin-checkpoint.patch",
    "reason": "Record verified allocation-backed runtime schemas, original member owner applications, alias/call-origin certificates and guarded environment/projected flow through kernel 207aabc.",
    **({"added": True} if path in ["check/native_aliases.rs", "check/native_runtime_tables.rs"] else {}),
} for path in [
    "check/decl.rs", "check/infer.rs", "check/instantiate.rs", "check/mod.rs",
    "check/native_aliases.rs", "check/native_flow.rs", "check/native_globals.rs",
    "check/native_named.rs", "check/native_runtime.rs", "check/native_runtime_tables.rs",
    "check/native_signatures.rs", "check/native_tables.rs", "check/print.rs",
    "check/relate.rs", "check/shape.rs", "check/sink.rs", "check/symbols.rs", "types.rs",
])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-fresh-namespace-environment-members.patch",
    "reason": "Prove raw writes to original fresh namespace allocations before any dispatch and resolve original member reads only after proving their physical global environment receiver through kernel 5e72664.",
} for path in ["mod.rs", "native_aliases.rs", "native_flow.rs", "sink.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-raw-slots-callable-loader.patch",
    "reason": "Retain original non-nil scalar raw-slot presence and all fresh receiver alternatives, guard cyclic loader inheritance, and prove reached literal __call execution through kernel e21b3cd.",
} for path in ["mod.rs", "native_aliases.rs", "sink.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-call-state-runtime-bounds-loops.patch",
    "reason": "Record original captured call-state replay, correlated result packs, reached metatable proofs, runtime storage bounds and bounded loop provenance through kernel 0cbe803; expose the host policy API without later policy behavior.",
    **({"added": True} if path in ["check/native_alias_loops.rs", "check/native_flow_calls.rs"] else {}),
} for path in [
    "check/infer.rs", "check/instantiate.rs", "check/mod.rs", "check/native_alias_loops.rs",
    "check/native_aliases.rs", "check/native_flow.rs", "check/native_flow_calls.rs",
    "check/native_globals.rs", "check/native_named.rs", "check/native_runtime_tables.rs",
    "check/sink.rs", "types.rs",
])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-standard-operation-policy.patch",
    "reason": "Separate selected host standard-operation assumptions from strict physical provenance, retain reached loader and explicit loop-write invalidation, preserve primitive member query errors and guarded declared owners, and report demand-local project limits through kernel bf8b84a.",
    **({"added": True} if path == "native_standard_assumptions.rs" else {}),
} for path in [
    "mod.rs", "native_aliases.rs", "native_flow.rs", "native_globals.rs", "native_named.rs",
    "native_project.rs", "native_standard_assumptions.rs", "native_tables.rs", "shape.rs", "sink.rs",
])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-namespace-capture-lua-printing.patch",
    "reason": "Retain attested namespace contracts in deferred bodies, resolve proven immutable captured callees before local loop effects, and expose a structural Lua nil printer mode while preserving default TypeScript formatting through kernel 696582a.",
} for path in ["native_named.rs", "native_standard_assumptions.rs", "print.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-query-completion-lua-nil-keys.patch",
    "reason": "Keep host-policy member classification and zero-argument replay bounded, distinguish usable uncached native results from unfinished queries, conservatively finish unsupported projected provenance, and retain Lua nil exclusion and union-key validation through kernel b6a9fbf.",
    **({"added": True} if path == "native_query_context.rs" else {}),
} for path in [
    "mod.rs", "native_aliases.rs", "native_flow.rs", "native_flow_calls.rs",
    "native_query_context.rs", "native_standard_assumptions.rs", "native_tables.rs",
])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-flow-declaration-contracts.patch",
    "reason": "Infer native runtime-table carrier domains, retain Comparable-only key overlap, distinguish dead references from correlated iterator frames, constrain parent member predicates, preserve pure declaration context and unresolved provisional status, and reuse guarded mapped written member/indexer contracts through kernel 07ad58d.",
} for path in [
    "flow_walk.rs", "infer.rs", "native_flow.rs", "native_globals.rs",
    "native_iterators.rs", "native_named.rs", "native_tables.rs",
])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-opaque-runtime-domains.patch",
    "reason": "Classify Lua userdata/thread domains from selected original nominal roots and guarded ancestry without expanding members, preserve namespace table facets and pending predicate dependencies, and traverse actual intersection members through kernel 86e96a7.",
} for path in ["mod.rs", "native_flow.rs", "native_named.rs", "native_runtime.rs", "relate.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-iterator-packs-display-inference.patch",
    "reason": "Separate hidden iterator arity, expose fixed written return cardinality, render original anonymous/container/generic contracts, infer full dictionary value domains, and retain reference types for unrelated predicates through kernel d389359.",
} for path in ["call.rs", "infer.rs", "native_flow.rs", "native_iterators.rs", "native_packs.rs", "print.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-structural-arrays-constructor-rows.patch",
    "reason": "Retain structural Lua array form and nil-inclusive constructor result rows through native sequence admission, relations, inference, reached writes, mapping, widening, traversal and printing through kernel 1296189.",
} for path in [
    "check/infer.rs", "check/instantiate.rs", "check/native_named.rs",
    "check/native_runtime_tables.rs", "check/native_tables.rs", "check/print.rs",
    "check/shape.rs", "types.rs",
])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_tables.rs",
    "patch": "vendor/patches/bun-sema-native-array-assertion-overlap.patch",
    "reason": "Separate Comparable numeric array overlap from strict sequence assignment/inference and retain fixed numeric field checks even with a covering source indexer through kernel 3521d3b.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-reached-numeric-stores.patch",
    "reason": "Infer bounded reached numeric stores on eligible unannotated local runtime tables, transport original key/scope recipes, prepare eligibility before RHS demands, and guard written numeric contracts and current-only completion through kernel 73f49a4.",
} for path in ["native_flow.rs", "native_named.rs", "native_runtime_tables.rs", "native_tables.rs", "sink.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-shared-program-publication.patch",
    "reason": "Publish completed snapshot-qualified native module/global/head results through shared Program buffers and provider-aware linking, retain task-local failures and guarded current-only results under unchanged inherited flow, and preserve deterministic union sorting through kernel f6512d7.",
} for path in ["check_source_file.rs", "mod.rs", "native_globals.rs", "native_named.rs", "native_project.rs", "native_query_context.rs", "unions.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-contextual-assignment-lifetimes.patch",
    "reason": "Retain contextual assignment ownership with the originating native query frame, distinguish final/provisional/expired instances, and inspect original callable/lexical identities without resolving parameters or returns through kernel f320c7a.",
} for path in ["check/call.rs", "check/mod.rs", "types.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-v1-contract-parity.patch",
    "reason": "Restore Lua constructor width subtyping, preserve original omitted documentation result contracts through native signature relations, and retain gradual Any applicability for native callable unions while keeping explicit never and ordinary TS calls strict through kernel e55f418.",
} for path in ["errors_call.rs", "mod.rs", "native_signatures.rs", "native_tables.rs", "relate.rs", "shape.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/print.rs",
    "patch": "vendor/patches/bun-sema-native-runtime-table-printing.patch",
    "reason": "Render unnamed native runtime-table declarations through the existing anonymous object printer, preserving named declaration formatting and cycle elision through kernel 5950739.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-check-once-determinism.patch",
    "reason": "Reuse context-free native argument results per mode and frame (forgotten with discarded body reports), remove call-state replay (deleting native_flow_calls.rs) as Bun does not replay callee stores, cache declaration kinds and runtime domains, check type-argument constraints apart from reference resolution, resolve members without relations, and order native unions by declaration name and type arguments, through kernel e5e554f.",
} for path in [
    "call.rs", "infer.rs", "mod.rs", "native_alias_loops.rs", "native_aliases.rs", "native_flow.rs",
    "native_flow_calls.rs", "native_globals.rs", "native_named.rs", "native_runtime.rs",
    "native_runtime_tables.rs", "native_standard_assumptions.rs", "native_tables.rs", "sink.rs", "unions.rs",
])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-language-service-members.patch",
    "reason": "List the members of a Lua value for the language service, as properties_of_type lists a TypeScript type's: names from every native table representation, kept where a native table read accepts them, through kernel a6f9b25.",
} for path in ["native_named.rs", "native_tables.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-luals-pathologies.patch",
    "reason": "Remove the exponential paths LuaLS's own sources exposed: a flow value computed inside an unfinished query is cached provisionally, a literal-key runtime-table read resolves its own key's members, and calls preserve standard bindings and the loader instead of proving each reached body free of writes, through kernel 0345818.",
} for path in ["mod.rs", "native_flow.rs", "native_named.rs", "native_standard_assumptions.rs", "sink.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-declared-operators.patch",
    "reason": "Resolve a binary or unary operator through the operator signatures a class declares with ---@operator, selecting the declaration by the other operand's type through the shared native signature workers, through kernel db66071.",
} for path in ["call.rs", "native_named.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/bun_core/thread_id.rs",
    "patch": "vendor/patches/bun-core-thread-id-syscall.patch",
    "reason": "Read the Linux thread id through the gettid system call, which glibc 2.28 has no wrapper for, through kernel 5425aee.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-luals-classes.patch",
    "reason": "Reopen a class's definition on another module's value when the class is declared in several places, keep a class table stored in its own field as the class, and build a sequence from stores at the length operator's next index, through kernel 37dd96c.",
} for path in ["native_flow.rs", "native_named.rs", "native_runtime_tables.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-member-cycles.patch",
    "reason": "Leave a member, a class's members or its validation requested again while in progress provisional, as TypeScript has a class's member symbols before their types: members without a type yet are unresolved, a read of a key no field has reads only the indexers, and a pending owner application taints from its own cycle, through kernel 239121f.",
} for path in ["mod.rs", "native_named.rs", "native_runtime_tables.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-string-operators.patch",
    "reason": "Resolve an operator on a string through the operator signatures its library's class declares (LuaLS's ---@class string with ---@operator), as the string library supplies a string's members, through kernel 9c1410f.",
} for path in ["call.rs", "native_globals.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/call.rs",
    "patch": "vendor/patches/bun-sema-any-default.patch",
    "reason": "Infer `any` for a type parameter a Lua call infers nothing for, as TypeScript does for a JavaScript call (InferenceFlags.AnyDefault), through kernel e1f2434.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-declared-indexer-views.patch",
    "reason": "Keep a class's declared indexers as declared in its partial definition view, as a TypeScript index signature answers for every key, through kernel 3f56cb6.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_intrinsics.rs",
    "patch": "vendor/patches/bun-sema-assert-any.patch",
    "reason": "Return the results of a call to `any` unchanged from the assert intrinsic, through kernel a66fe8a.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_runtime_tables.rs",
    "patch": "vendor/patches/bun-sema-runtime-member-cycles.patch",
    "reason": "Infer a runtime-table store whose value reads the member it stores with that member unresolved, as a class member is, through kernel 0c301c2.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-task-independence.patch",
    "reason": "Use module exports, global declarations and loader states completed under an uncacheable context, uncached; follow straight runs of flow nodes in the loader walk without a frame each; and order unnamed Lua types in unions by declaration and structure rather than creation, so reports do not depend on how files are grouped into tasks, through kernel 8b93185.",
} for path in ["native_globals.rs", "native_project.rs", "native_standard_assumptions.rs", "unions.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_tables.rs",
    "patch": "vendor/patches/bun-sema-nil-declared-keys.patch",
    "reason": "Let a written key type include nil, as Luanalysis v1 does (table<nil | string, V>): lookups whose key may be nil check against it, while constructor keys and nil-only key types stay invalid, through kernel 20c113d.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-merged-field-declarations.patch",
    "reason": "Merge an identical field redeclaration, let a written field outrank a definition table's value for it, and report only a conflicting redeclaration, once at its declaration, through kernel 5ad4ad0.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-class-table-as-class.patch",
    "reason": "Read a member a class definition table has not been given yet as the class member, and relate the table as its class, as v1 types a class table, through kernel b3bfd0b.",
} for path in ["native_named.rs", "relate.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-literal-computed-keys.patch",
    "reason": "Store the field a computed key of one string-literal type names, in a runtime table's schema, its literal reads and its flow, as TypeScript reads a late-bound assignment, through kernel 905e017.",
} for path in ["native_flow.rs", "native_named.rs", "native_runtime_tables.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_tables.rs",
    "patch": "vendor/patches/bun-sema-void-key-type.patch",
    "reason": "Accept a written void key type as an index that holds no keys, as Luanalysis v1 does, through kernel f5eda3f.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_flow.rs",
    "patch": "vendor/patches/bun-sema-computed-key-writes.patch",
    "reason": "Treat a store through a computed key as a possible write of exactly the sibling field it may name, nothing for a field whose key it cannot be, and exactly that field for a single literal key, through kernel d1d2c39.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-nested-member-writes.patch",
    "reason": "Apply a store through a fixed path below a member to the table that member holds in a runtime table's or definition's flow view, through kernel 2146387.",
} for path in ["native_flow.rs", "native_named.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-written-assignment-overrides.patch",
    "reason": "Let an assignment member whose type is written declare the member despite an existing contract, so it overrides an inherited member as v1 indexes it, through kernel 20942e3.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-written-assignment-origin.patch",
    "reason": "Keep an explicitly written assignment's declared origin when it overrides an inherited contract, matching the existing written-assignment demand policy; otherwise its own narrowed member silently disappears.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-usable-invalid-overrides.patch",
    "reason": "Report an incompatible override or base once and keep the class's declared shape for its uses, while its validation still fails, as TypeScript keeps such a class, through kernel 3bf26e1.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_flow.rs",
    "patch": "vendor/patches/bun-sema-generic-correlated-slots.patch",
    "reason": "Keep a part of a correlated binding's view only if it relates to a type-variable slot by assignability, not comparability, as TypeScript does not compare undefined with an unconstrained parameter, through kernel ffe75c9.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_runtime_tables.rs",
    "patch": "vendor/patches/bun-sema-evolving-sequences.patch",
    "reason": "Keep a runtime table a sequence through numeric and literal-index stores, as TypeScript evolves an array by an element store, through kernel 8f44124.",
})

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_flow.rs",
    "patch": "vendor/patches/bun-sema-absent-member-narrowing.patch",
    "reason": "Leave a union unnarrowed by a member predicate when no alternative has the member, as TypeScript does not narrow by a property no constituent has, through kernel e9ee7fd.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-union-argument-distribution.patch",
    "reason": "Combine the candidates one union-typed source's alternatives give a type parameter as their union, and distribute a Lua call's result over those alternatives, through kernel 740a6c3.",
} for path in ["call.rs", "infer.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_named.rs",
    "patch": "vendor/patches/bun-sema-member-origins.patch",
    "reason": "Expose the members member lookup applies to a named object by name, own then inherited with overloads, for the language service's origins and documentation, through kernel c0224e3.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-intersection-reads.patch",
    "reason": "Read an intersection's member from the parts that declare it, and other names from the parts' index signatures, as TypeScript does, and list an intersection's parts' members for completion, through kernel 5006205.",
} for path in ["native_named.rs", "native_tables.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_globals.rs",
    "patch": "vendor/patches/bun-sema-standard-global-precedence.patch",
    "reason": "Type a global by its first declaration in program order when that is the standard library's or a declaration file's, reporting each later conflicting declaration where it is, as TypeScript does, through kernel 8cae069.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-type-predicates.patch",
    "reason": "Resolve written native type predicates through signature caches and mappers, retain boolean call results, and narrow their original arguments through the shared TypeScript predicate worker.",
} for path in ["call.rs", "decl.rs", "mod.rs", "native_flow.rs", "native_packs.rs", "native_signatures.rs", "sink.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-predicate-environment-references.patch",
    "reason": "Canonicalize native predicate references at frozen receiver inputs through a flow-only typed provider demand, with a pure presence gate, guarded epoch cache and incomplete-query propagation; preserve structural reachability without receiver type demands.",
} for path in ["mod.rs", "native_flow.rs", "sink.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-table-contracts.patch",
    "reason": "Reject unrelated dictionary domains and opaque table impostors while preserving partial indexer views, every shared value contract, exact-field precedence and sound dynamic intersection reads and writes.",
} for path in ["native_named.rs", "native_runtime.rs", "native_tables.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-generic-result-packs.patch",
    "reason": "Infer and instantiate the complete physical producer result pack independently of its first scalar value when a native generic parameter is declared in callable result position.",
} for path in ["types.rs", "check/call.rs", "check/decl.rs", "check/infer.rs", "check/instantiate.rs", "check/mod.rs", "check/native_packs.rs", "check/print.rs", "check/relate.rs", "check/shape.rs"])

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-nil-compatibility.patch",
    "reason": "Provide explicit non-strict nil assignment compatibility while preserving strict defaults, physical nil flow, required member presence, never, and separate strict physical relation caches.",
} for path in ["mod.rs", "relate.rs", "native_tables.rs", "native_assertions.rs", "native_flow.rs", "flow.rs"])

SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_nil.rs",
    "patch": "vendor/patches/bun-sema-native-nil-compatibility.patch",
    "reason": "Keep nil representation strict and implement only the requested compatibility relation; physical exclusions and narrowing use a distinct recursively strict relation.",
    "added": True,
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-runtime-schema-initialization.patch",
    "reason": "Allow an original allocation to initialize only its own prospective runtime schema without asserting future field presence, retaining structural checks against every unrelated target.",
} for path in ["native_named.rs", "relate.rs"])


SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/instantiate.rs",
    "patch": "vendor/patches/bun-sema-native-generic-pack-alias-substitution.patch",
    "reason": "Preserve an aliased producer's symbolic result facet only when its mapped generic destination is itself a producer pack; scalar generic aliases keep their original single-value contract.",
})

SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/" + path,
    "patch": "vendor/patches/bun-sema-native-generic-pack-repetition.patch",
    "reason": "Represent repeated generic producer results with a rigid physical-element facet, preserving first-slot types, nullable repetition cardinality, full source packs and checked fixed prefixes without widening arbitrary generic-body values.",
} for path in ["types.rs", "check/decl.rs", "check/infer.rs", "check/instantiate.rs", "check/native_packs.rs", "check/print.rs", "check/relate.rs", "check/shape.rs"])



SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-runtime-completion-seed.patch",
    "reason": "Enumerate original constructor keys together with later store metadata when a cross-file runtime allocation is published as its declaration schema, preserving ordinary checked member reads and lazy function values.",
} for path in ["native_tables.rs", "native_runtime_tables.rs"])



SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-generic-storage-variance.patch",
    "reason": "Require invariant mapped native class arguments when observable through writable storage, preserving phantom parameters, readonly and callable contracts with finite demand-local profiles and fail-closed incomplete proofs.",
} for path in ["native_named.rs", "relate.rs"])
SOURCE_ADAPTATIONS.append({
    "path": "src/sema/check/native_generic_storage.rs",
    "patch": "vendor/patches/bun-sema-native-generic-storage-variance.patch",
    "reason": "Analyze native generic writable storage through original mapped declarations, captured parameters and finite recursive observability profiles without publishing provisional variance masks.",
    "added": True,
})
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-type-operators.patch",
    "reason": "Give keyof and indexed access types Lua table keys: literal field keys of every kind, indexer key types and sequence numbers, reads through the native table read, and generic table reads at their own generic keys as deferred T[K].",
} for path in ["mapped.rs", "native_tables.rs"])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-assertion-predicates.patch",
    "reason": "Carry written assertion predicates (asserts x is T, asserts x) through native signatures, narrowing an asserted argument by its type or truthiness wherever the call returns.",
} for path in ["decl.rs", "native_flow.rs", "native_signatures.rs"])
SOURCE_ADAPTATIONS.extend({
    "path": "src/sema/check/" + path,
    "patch": "vendor/patches/bun-sema-native-overload-failure-report.patch",
    "reason": "When several overloads reject a native call's arguments, report the error of the one that accepts the most of them (the last on a tie, as TypeScript reports the last), with its position and signature, so the frontend says that no overload matches at the argument where the call goes wrong.",
} for path in ["call.rs", "mod.rs"])


def git(source, *args):
    return subprocess.check_output(["git", "-C", str(source), *args])


def sections(text):
    return re.split(r"(?m)(?=^\[)", text)


def patched_sources(source):
    """Apply each reviewed patch once to pristine files in an isolated directory."""
    by_patch = {}
    for adaptation in SOURCE_ADAPTATIONS:
        by_patch.setdefault(adaptation["patch"], set()).add(adaptation["path"])
    all_paths = set().union(*by_patch.values())
    added_paths = {a["path"] for a in SOURCE_ADAPTATIONS if a.get("added")}
    if added_paths and git(source, "ls-tree", "-r", "--name-only", REVISION,
                           *sorted(added_paths)).strip():
        raise SystemExit("an explicit source addition already exists in the pinned upstream tree")
    with tempfile.TemporaryDirectory(prefix="luanalysis-bun-extract-") as directory:
        temporary = Path(directory)
        for path in all_paths:
            if path in added_paths:
                continue
            target = temporary / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(git(source, "show", f"{REVISION}:{path}"))
        for patch_name, paths in by_patch.items():
            patch = (ROOT / patch_name).read_bytes()
            stats = subprocess.check_output(
                ["git", "apply", "--numstat", "-z"], input=patch, cwd=temporary)
            changed = {record.split(b"\t", 2)[2].decode()
                       for record in stats.split(b"\0") if record}
            if changed != paths:
                raise SystemExit(f"patch paths differ from source adaptations: {patch_name}")
            subprocess.run(["git", "apply", "--whitespace=nowarn"],
                           input=patch, cwd=temporary, check=True)
        # A later patch may delete a file that an earlier one added.
        return {path: (temporary / path).read_bytes() for path in all_paths if (temporary / path).exists()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    source = args.source.resolve()
    head = git(source, "rev-parse", "HEAD").decode().strip()
    if head != REVISION:
        parser.error(f"expected {REVISION}; source is {head}")
    if git(source, "status", "--porcelain", "--untracked-files=no"):
        parser.error("the upstream checkout must have no tracked modifications")
    adapted_sources = patched_sources(source)
    original_root = git(source, "show", f"{REVISION}:Cargo.toml").decode()
    workspace = tomllib.loads(original_root)["workspace"]
    inherited = workspace["dependencies"]
    closure = {}
    used = set()

    def visit(name):
        if name in closure:
            return
        dependency = inherited[name]
        package_path = dependency["path"]
        text = git(source, "show", f"{REVISION}:{package_path}/Cargo.toml").decode()
        # Drop development-only edges from the extracted library manifests.
        kept = [s for s in sections(text) if not re.match(
            r"\[(?:dev-dependencies|target\..*\.dev-dependencies|\[test|\[bench|\[bin)\]", s)]
        manifest = "".join(kept)
        data = tomllib.loads(manifest)
        closure[name] = (package_path, manifest)
        groups = [data.get("dependencies", {}), data.get("build-dependencies", {})]
        groups.extend(t.get("dependencies", {}) for t in data.get("target", {}).values())
        groups.extend(t.get("build-dependencies", {}) for t in data.get("target", {}).values())
        for group in groups:
            for key, dep in group.items():
                if isinstance(dep, dict) and dep.get("workspace"):
                    used.add(key)
                    if isinstance(inherited[key], dict) and "path" in inherited[key]:
                        visit(key)

    visit("bun_sema")
    destination = ROOT / "vendor" / "bun"
    destination.mkdir(parents=True, exist_ok=True)
    hashes = {}
    additions = {}
    tracked = git(source, "ls-tree", "-r", "--name-only", REVISION).decode().splitlines()

    def output(path, contents, upstream_path=None):
        target = destination / path
        if args.verify:
            if not target.is_file() or target.read_bytes() != contents:
                raise SystemExit(f"extraction mismatch: {target}")
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            if not target.is_file() or target.read_bytes() != contents:
                target.write_bytes(contents)
        if upstream_path:
            raw = git(source, "show", f"{REVISION}:{upstream_path}")
            hashes[path] = {"upstream": upstream_path, "sha256": hashlib.sha256(raw).hexdigest(),
                            "extracted_sha256": hashlib.sha256(contents).hexdigest()}

    paths = [path for path, _ in closure.values()]
    for path in tracked:
        containing = next((p for p in paths if path.startswith(p + "/")), None)
        if not containing or path.endswith("Cargo.toml"):
            continue
        if containing == "src/sema" and any(path.startswith(f"src/sema/{p}/")
                                             for p in ["parser", "driver", "baselines", "standalone"]):
            continue
        raw = git(source, "show", f"{REVISION}:{path}")
        output(path, adapted_sources.get(path, raw), path)
    for path, contents in sorted(adapted_sources.items()):
        if path in tracked:
            continue
        introduction = next((a for a in SOURCE_ADAPTATIONS
                             if a["path"] == path and a.get("added")), None)
        if introduction is None:
            raise SystemExit(f"adapted file has no upstream origin or explicit addition: {path}")
        output(path, contents)
        additions[path] = {"extracted_sha256": hashlib.sha256(contents).hexdigest(),
                           "introduced_by_patch": introduction["patch"]}
    for path, manifest in closure.values():
        output(path + "/Cargo.toml", manifest.encode(), path + "/Cargo.toml")
    for original, target in [("LICENSE.md", "LICENSE.md"),
                             ("test/cli/check/typescript-go/LICENSE", "licenses/typescript-go-LICENSE"),
                             ("test/cli/check/typescript-go/NOTICE.txt", "licenses/typescript-go-NOTICE.txt")]:
        output(target, git(source, "show", f"{REVISION}:{original}"), original)
    dependency_section = next(s for s in sections(original_root)
                              if s.startswith("[workspace.dependencies]"))
    dependency_lines = []
    for line in dependency_section.splitlines()[1:]:
        match = re.match(r"^([\w-]+)\s*=", line)
        if match and match[1] in used:
            dependency_lines.append(line)
    manifest = ("# Generated by scripts/vendor-bun.py; see provenance.json for exact adaptations.\n"
                "[workspace]\nresolver = \"2\"\nmembers = " + json.dumps(sorted(paths)) + "\n\n"
                "[workspace.package]\nversion = \"0.0.0\"\nedition = \"2024\"\n\n"
                "[workspace.lints.rust]\nunexpected_cfgs = { level = \"warn\", check-cfg = ["
                "'cfg(bun_asan)', 'cfg(bun_debug)', 'cfg(socket_fault_injection)', "
                "'cfg(bun_disallow_code_generation_from_strings)', 'cfg(bun_sema_mimalloc)'] }\n\n"
                "[workspace.dependencies]\n" + "\n".join(dependency_lines) + "\n")
    output("Cargo.toml", manifest.encode())
    output("Cargo.lock", git(source, "show", f"{REVISION}:Cargo.lock"), "Cargo.lock")
    # bun_core/build.rs consumes runtime build identity constants even when only
    # its utility modules are linked. No Bun runtime or codegen is executed here.
    build_options = f'''// Standalone utility-library build identity, generated by vendor-bun.py.
pub const SHA: &str = "{REVISION}";
pub const REPORTED_NODEJS_VERSION: &str = "";
pub const RELEASE_SAFE: bool = true;
pub const IS_CANARY: bool = false;
pub const CANARY_REVISION: &str = "";
pub const ENABLE_FUZZILLI: bool = false;
pub const FALLBACK_HTML_VERSION: &str = "0000000000000000";
pub const VERSION: crate::Version = crate::Version {{ major: 0, minor: 0, patch: 0 }};
pub const BASE_PATH: &[u8] = b"";
pub const CODEGEN_PATH: &[u8] = b"";
pub const ENABLE_LOGS: bool = false;
pub const ENABLE_ASAN: bool = false;
pub const ENABLE_TINYCC: bool = false;
'''
    output("build/debug/codegen/build_options.rs", build_options.encode())
    # A reference copy for the small standalone-symbol shim in our own crate.
    original_native = "src/sema/standalone/native.rs"
    output("standalone-native.rs", git(source, "show", f"{REVISION}:{original_native}"), original_native)
    reference_metadata = "test/cli/check/typescript-go/version.json"
    reference_revisions = json.loads(git(source, "show", f"{REVISION}:{reference_metadata}"))
    checker_version_source = "src/sema/resolve.rs"
    checker_source = git(source, "show", f"{REVISION}:{checker_version_source}").decode()
    checker_version = re.search(r'pub const VERSION_C: &core::ffi::CStr = c"([^\"]+)";', checker_source)
    if checker_version is None:
        raise SystemExit("cannot identify the pinned checker's TypeScript version")
    provenance = {"upstream": "https://github.com/oven-sh/bun", "revision": REVISION,
                  "typescript_reference": {
                      "checker_version": checker_version[1],
                      "checker_version_source": checker_version_source,
                      "typescript_go_revision": reference_revisions["microsoft/typescript-go"],
                      "typescript_test_revision": reference_revisions["microsoft/TypeScript"],
                      "reference_metadata_source": reference_metadata,
                  },
                  "crates": sorted(closure), "manifest_adaptations": [
                      "Remove development dependency tables and binary/test/bench targets.",
                      "Replace Bun runtime workspace manifest with the library dependency closure."],
                  "source_adaptations": [{"path": adaptation["path"], "patch": adaptation["patch"],
                                          "reason": adaptation["reason"],
                                          **({"added": True} if adaptation.get("added") else {}),
                                          "patch_sha256": hashlib.sha256((ROOT / adaptation["patch"]).read_bytes()).hexdigest()}
                                         for adaptation in SOURCE_ADAPTATIONS],
                  "source_additions": dict(sorted(additions.items())),
                  "files": dict(sorted(hashes.items()))}
    output("provenance.json", (json.dumps(provenance, indent=2) + "\n").encode())
    print(f"{'Verified' if args.verify else 'Extracted'} {len(closure)} libraries; {len(hashes)} pinned files")


if __name__ == "__main__":
    main()
