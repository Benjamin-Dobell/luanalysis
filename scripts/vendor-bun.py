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
        return {path: (temporary / path).read_bytes() for path in all_paths}


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
