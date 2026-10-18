#!/usr/bin/env python3
"""Data/measurement helper; bench.sh owns workload generation and scheduling."""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import signal
import statistics
import sys
import time


def write_json(path, value):
    pathlib.Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def report_info(stdout, stderr, status):
    info = {
        "stdout_sha256": sha256(stdout),
        "stdout_bytes": pathlib.Path(stdout).stat().st_size,
        "stderr_bytes": pathlib.Path(stderr).stat().st_size,
        "report_valid": False,
    }
    try:
        report = json.loads(pathlib.Path(stdout).read_text())
        info["report_valid"] = (
            isinstance(report, dict)
            and report.get("formatVersion") == 1
            and report.get("exitCode") == status
            and status in (0, 1)
            and isinstance(report.get("diagnostics"), list)
        )
        if isinstance(report, dict):
            diagnostics = report.get("diagnostics", [])
            info["files_checked"] = report.get("filesChecked")
            info["diagnostics_count"] = len(diagnostics)
            counts = {}
            for diagnostic in diagnostics:
                code = diagnostic.get("code", "<missing>")
                counts[code] = counts.get(code, 0) + 1
            info["diagnostics_by_code"] = counts
        # This preserves ALL paths, array order and fields. Only JSON object key
        # order/whitespace are canonicalized; there are no speculative path maps.
        canonical = json.dumps(report, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        info["full_report_digest"] = hashlib.sha256(canonical.encode()).hexdigest()
    except (OSError, UnicodeError, ValueError, TypeError, AttributeError) as error:
        info["report_error"] = str(error)
    return info


def measure(args):
    prefix = pathlib.Path(args.prefix)
    prefix.parent.mkdir(parents=True, exist_ok=True)
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        raise SystemExit("missing measured command")
    stdout = str(prefix) + ".stdout.json"
    stderr = str(prefix) + ".stderr.txt"
    start_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
    started = time.monotonic_ns()
    timed_out = False
    interrupted_signal = None
    pid = os.fork()
    if pid == 0:
        try:
            os.setsid()
            outfd = os.open(stdout, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
            errfd = os.open(stderr, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
            os.dup2(outfd, 1)
            os.dup2(errfd, 2)
            os.close(outfd)
            os.close(errfd)
            os.execvpe(command[0], command, os.environ)
        except BaseException as error:
            os.write(2, ("exec failed: " + str(error) + "\n").encode())
            os._exit(127)

    def stop_child(signum, _frame):
        nonlocal timed_out, interrupted_signal
        if signum == signal.SIGALRM:
            timed_out = True
        else:
            interrupted_signal = signum
        try:
            os.killpg(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    old_handlers = {signum: signal.signal(signum, stop_child) for signum in (signal.SIGALRM, signal.SIGINT, signal.SIGTERM)}
    signal.setitimer(signal.ITIMER_REAL, args.timeout)
    try:
        while True:
            try:
                _, raw_status, usage = os.wait4(pid, 0)
                break
            except InterruptedError:
                continue
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        for signum, old_handler in old_handlers.items():
            signal.signal(signum, old_handler)
    elapsed = (time.monotonic_ns() - started) / 1e9
    status = os.waitstatus_to_exitcode(raw_status)
    value = {
        "workload": args.workload,
        "variant": args.variant,
        "phase": args.phase,
        "round": args.round,
        "order_in_pair": args.order,
        "command": command,
        "cwd": os.getcwd(),
        "start_utc": start_utc,
        "wall_seconds": elapsed,
        "user_seconds": usage.ru_utime,
        "system_seconds": usage.ru_stime,
        "cpu_seconds": usage.ru_utime + usage.ru_stime,
        "max_rss_bytes": usage.ru_maxrss if sys.platform == "darwin" else usage.ru_maxrss * 1024,
        "max_rss_native_unit": "bytes" if sys.platform == "darwin" else "KiB",
        "exit_status": status,
        "termination_signal": -status if status < 0 else None,
        "timed_out": timed_out,
        "interrupted_signal": interrupted_signal,
        "timeout_seconds": args.timeout,
        "stdout": stdout,
        "stderr": stderr,
    }
    value.update(report_info(stdout, stderr, status))
    write_json(str(prefix) + ".metrics.json", value)
    print("{workload} {variant} {phase}{round}: wall={wall_seconds:.4f}s cpu={cpu_seconds:.4f}s rss={max_rss_bytes} exit={exit_status} diagnostics={diagnostics_count}".format(diagnostics_count=value.get("diagnostics_count", "unparsed"), **{k: v for k, v in value.items() if k != "diagnostics_count"}), flush=True)
    if interrupted_signal:
        raise SystemExit(128 + interrupted_signal)


def inventory(args):
    files = set()
    for name in args.paths:
        path = pathlib.Path(name)
        if path.is_file():
            files.add(path)
        else:
            files.update(p for p in path.rglob("*") if p.is_file() and p.suffix in (".lua", ".ttslua"))
    records = [{"path": str(path), "bytes": path.stat().st_size, "sha256": sha256(path)} for path in sorted(files)]
    write_json(args.output, records)


def median(values):
    return statistics.median(values) if values else None


def ratio(a, b):
    return a / b if a is not None and b else None


def summarize(args):
    root = pathlib.Path(args.output)
    records = [json.loads(p.read_text()) for p in sorted((root / "runs").rglob("*.metrics.json"))]
    measurements = [record for record in records if record["phase"] == "measure"]
    groups = {}
    for record in measurements:
        groups.setdefault(record["workload"], {}).setdefault(record["variant"], []).append(record)
    summary = {
        "method": {
            "timing": "monotonic wall clock and os.wait4 child rusage; fresh CLI process per run",
            "rss": "ru_maxrss converted to bytes (already bytes on macOS)",
            "normalization": "JSON object key order/whitespace only; no path replacements; full arrays and fields preserved",
            "comparison": "current / baseline; ratios over 1 mean more time or memory",
        },
        "all_runs": records,
        "workloads": {},
        "feature_cost": {},
        "scaling": {},
    }
    fields = ("wall_seconds", "user_seconds", "system_seconds", "cpu_seconds", "max_rss_bytes")
    lines = ["workload\tvariant\truns\twall_median_s\tcpu_median_s\trss_median_bytes\treport_digests\tvalid"]
    for workload, variants in sorted(groups.items()):
        entry = {"variants": {}}
        for variant, runs in sorted(variants.items()):
            stats = {"runs": len(runs), "valid": all(r.get("report_valid") and not r["timed_out"] and r["stderr_bytes"] == 0 for r in runs)}
            stats["exit_statuses"] = sorted({r["exit_status"] for r in runs})
            stats["diagnostics_counts"] = sorted({r.get("diagnostics_count") for r in runs}, key=str)
            stats["files_checked"] = sorted({r.get("files_checked") for r in runs}, key=str)
            stats["report_digests"] = sorted({r.get("full_report_digest") for r in runs}, key=str)
            stats["deterministic"] = len(stats["report_digests"]) == 1 and None not in stats["report_digests"]
            for field in fields:
                values = [r[field] for r in runs]
                stats[field] = {"median": median(values), "min": min(values), "max": max(values), "samples": values}
            entry["variants"][variant] = stats
            lines.append("{}\t{}\t{}\t{:.6f}\t{:.6f}\t{}\t{}\t{}".format(workload, variant, len(runs), stats["wall_seconds"]["median"], stats["cpu_seconds"]["median"], stats["max_rss_bytes"]["median"], len(stats["report_digests"]), stats["valid"]))
        if "baseline" in variants and "current" in variants:
            before, after = entry["variants"]["baseline"], entry["variants"]["current"]
            entry["diagnostics_and_full_report_parity"] = before["deterministic"] and after["deterministic"] and before["report_digests"] == after["report_digests"]
            entry["current_over_baseline"] = {field: ratio(after[field]["median"], before[field]["median"]) for field in fields}
            brounds = {r["round"]: r for r in variants["baseline"]}
            arounds = {r["round"]: r for r in variants["current"]}
            entry["paired_ratios"] = [{"round": n, **{field: ratio(arounds[n][field], brounds[n][field]) for field in fields}} for n in sorted(brounds.keys() & arounds.keys())]
        summary["workloads"][workload] = entry
    for size in (128, 256):
        boolean = summary["workloads"].get(f"boolean{size}", {}).get("variants", {}).get("current")
        predicate = summary["workloads"].get(f"predicate{size}", {}).get("variants", {}).get("current")
        if boolean and predicate:
            summary["feature_cost"][str(size)] = {field: ratio(predicate[field]["median"], boolean[field]["median"]) for field in fields}
    for kind in ("boolean", "predicate"):
        for variant in ("baseline", "current"):
            small = summary["workloads"].get(f"{kind}128", {}).get("variants", {}).get(variant)
            large = summary["workloads"].get(f"{kind}256", {}).get("variants", {}).get(variant)
            if small and large:
                summary["scaling"][f"{kind}-{variant}-256-over-128"] = {field: ratio(large[field]["median"], small[field]["median"]) for field in fields}
    summary["checks"] = {
        "all_reports_valid": all(r.get("report_valid") and not r["timed_out"] and r["stderr_bytes"] == 0 for r in records),
        "all_paired_full_reports_match": all(w.get("diagnostics_and_full_report_parity", True) for w in summary["workloads"].values()),
        "all_reports_deterministic": all(s["deterministic"] for w in summary["workloads"].values() for s in w["variants"].values()),
    }
    write_json(root / "results.json", summary)
    (root / "summary.tsv").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(json.dumps({"checks": summary["checks"], "feature_cost": summary["feature_cost"], "scaling": summary["scaling"]}, indent=2))


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="operation", required=True)
    run = commands.add_parser("run")
    run.add_argument("--prefix", required=True)
    run.add_argument("--workload", required=True)
    run.add_argument("--variant", required=True)
    run.add_argument("--phase", choices=("warmup", "measure"), required=True)
    run.add_argument("--round", type=int, required=True)
    run.add_argument("--order", type=int, required=True)
    run.add_argument("--timeout", type=float, required=True)
    run.add_argument("command", nargs=argparse.REMAINDER)
    run.set_defaults(function=measure)
    inv = commands.add_parser("inventory")
    inv.add_argument("--output", required=True)
    inv.add_argument("paths", nargs="+")
    inv.set_defaults(function=inventory)
    report = commands.add_parser("summarize")
    report.add_argument("--output", required=True)
    report.set_defaults(function=summarize)
    args = parser.parse_args()
    args.function(args)


if __name__ == "__main__":
    main()
