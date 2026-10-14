#!/usr/bin/env python3
"""Commit a pristine library snapshot on its own upstream tracking branch.

The working tree and active index are not changed. Advance this branch with a
new source revision before reviewing and merging an upstream library update.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("source", type=Path)
parser.add_argument("--revision")
args = parser.parse_args()
source = args.source.resolve()
provenance = json.loads((root / "vendor/bun/provenance.json").read_text())
revision = args.revision or provenance["revision"]
reference = "refs/heads/upstream/bun-libraries"

def git(directory, arguments, data=None, env=None, check=True):
    return subprocess.run(["git", "-C", str(directory), *arguments], input=data,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          env=env, check=check)

revision = git(source, ["rev-parse", f"{revision}^{{commit}}"]).stdout.decode().strip()
entries = {}
for record in git(source, ["ls-tree", "-rz", revision]).stdout.split(b"\0"):
    if not record:
        continue
    metadata, path = record.split(b"\t", 1)
    mode, kind, oid = metadata.decode().split()
    if kind == "blob":
        entries[path.decode()] = mode

metadata = {
    "repository": provenance["upstream"],
    "revision": revision,
    "branch": reference.removeprefix("refs/heads/"),
    "crates": provenance["crates"],
    "paths": {path: record["upstream"] for path, record in provenance["files"].items()},
    "purpose": "Pristine source snapshot before standalone packaging or library adaptations.",
}

previous = git(root, ["rev-parse", "--verify", reference], check=False)
parent = previous.stdout.decode().strip() if previous.returncode == 0 else None
with tempfile.TemporaryDirectory(prefix="luanalysis-upstream-index-") as temporary:
    environment = os.environ.copy()
    environment["GIT_INDEX_FILE"] = str(Path(temporary) / "index")
    git(root, ["read-tree", "--empty"], env=environment)
    index_entries = []
    for path, record in provenance["files"].items():
        upstream = record["upstream"]
        data = git(source, ["show", f"{revision}:{upstream}"]).stdout
        oid = git(root, ["hash-object", "-w", "--stdin"], data).stdout.decode().strip()
        index_entries.append(f"{entries[upstream]} {oid}\tvendor/bun/{path}\0".encode())
    data = (json.dumps(metadata, indent=2) + "\n").encode()
    oid = git(root, ["hash-object", "-w", "--stdin"], data).stdout.decode().strip()
    index_entries.append(f"100644 {oid}\tvendor/bun/UPSTREAM.json\0".encode())
    git(root, ["update-index", "-z", "--index-info"], b"".join(index_entries), environment)
    tree = git(root, ["write-tree"], env=environment).stdout.decode().strip()
    if parent and git(root, ["rev-parse", f"{parent}^{{tree}}"]).stdout.decode().strip() == tree:
        print(parent)
    else:
        arguments = ["commit-tree", tree]
        if parent:
            arguments.extend(["-p", parent])
        message = (f"Import pristine Bun checker libraries at {revision[:12]}\n\n"
                   f"Upstream: {provenance['upstream']}\nRevision: {revision}\n"
                   f"Selected library count: {len(provenance['crates'])}\n"
                   "All imported bytes and modes match this upstream commit.\n"
                   "Standalone packaging and local adaptations follow separately.\n").encode()
        commit = git(root, arguments, message).stdout.decode().strip()
        git(root, ["update-ref", reference, commit, parent or "0" * 40])
        print(commit)
