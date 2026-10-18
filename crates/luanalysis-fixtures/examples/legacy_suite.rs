//! Run every imported expectation. A mismatch, crash or timeout exits nonzero.
#[path = "../tests/support/legacy.rs"]
mod legacy;
use std::{
    fs,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--worker") {
        let case = legacy::cases()
            .into_iter()
            .find(|c| c.id == args[1])
            .expect("unknown case");
        let result = legacy::run(&case, std::path::Path::new(&args[2]));
        println!(
            "{}\t{}\texpected={} missing={} extra={}\n{}",
            if result.passed { "PASS" } else { "FAIL" },
            case.id,
            result.expected,
            result.missing,
            result.unexpected,
            result.detail
        );
        std::process::exit(if result.passed { 0 } else { 1 });
    }
    if args.iter().any(|a| a == "--help") {
        println!(
            "Usage: legacy_suite [--case substring] [--report path] [--timeout-seconds n]\n\nRuns all original expectations; exits 1 for any mismatch, crash or timeout.\nEach case has an isolated process and temporary project. Default timeout: 30 seconds."
        );
        return;
    }
    let mut filter = String::new();
    let mut report = None;
    let mut timeout = Duration::from_secs(30);
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--case" => filter = arguments.next().expect("--case value").clone(),
            "--report" => report = Some(arguments.next().expect("--report value").clone()),
            "--timeout-seconds" => {
                timeout = Duration::from_secs(
                    arguments
                        .next()
                        .expect("timeout value")
                        .parse::<u64>()
                        .expect("positive timeout")
                        .max(1),
                )
            }
            other => panic!("unknown argument {other}; use --help"),
        }
    }
    let cases: Vec<_> = legacy::cases()
        .into_iter()
        .filter(|case| case.id.contains(&filter))
        .collect();
    assert!(!cases.is_empty(), "no matching cases");
    let mut output = format!(
        "Pinned IntelliJ-Luanalysis {}\n",
        luanalysis_fixtures::LEGACY_REVISION
    );
    let mut counts = [0usize; 4];
    for case in &cases {
        let directory = legacy::Temp::new();
        let stdout_path = directory.0.join("stdout.txt");
        let stderr_path = directory.0.join("stderr.txt");
        let mut worker = Command::new(std::env::current_exe().unwrap())
            .args(["--worker", case.id])
            .arg(&directory.0)
            .stdout(Stdio::from(fs::File::create(&stdout_path).unwrap()))
            .stderr(Stdio::from(fs::File::create(&stderr_path).unwrap()))
            .spawn()
            .unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(status) = worker.try_wait().unwrap() {
                break Some(status);
            }
            if start.elapsed() >= timeout {
                worker.kill().unwrap();
                worker.wait().unwrap();
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let text = match status {
            Some(status) if status.success() => {
                counts[0] += 1;
                fs::read_to_string(&stdout_path).unwrap()
            }
            Some(status) if status.code() == Some(1) => {
                counts[1] += 1;
                fs::read_to_string(&stdout_path).unwrap()
            }
            Some(status) => {
                counts[2] += 1;
                format!(
                    "CRASH\t{}\t{status}\n{}\n",
                    case.id,
                    fs::read_to_string(&stderr_path).unwrap()
                )
            }
            None => {
                counts[3] += 1;
                format!("TIMEOUT\t{}\t{} seconds\n", case.id, timeout.as_secs())
            }
        };
        println!("{}", text.lines().next().unwrap_or("<empty worker report>"));
        output.push_str(&text);
        output.push_str(&format!("  Origin: {}\n  Configuration: Lua {}; strict_nil={}; unknown_callable/indexable={}; weak_warnings={}\n", case.origin, case.version, case.strict_nil, case.unknown, case.weak));
        if !case.notes.is_empty() {
            output.push_str(&format!("  {}\n", case.notes));
        }
        output.push('\n');
    }
    let summary = format!(
        "{} cases: {} passed, {} mismatched, {} crashed, {} timed out.\n",
        cases.len(),
        counts[0],
        counts[1],
        counts[2],
        counts[3]
    );
    print!("{summary}");
    output.push_str(&summary);
    let mut unsupported = 0;
    if filter.is_empty() {
        for line in legacy::INVENTORY.lines().filter(|l| !l.starts_with('#')) {
            let fields: Vec<_> = line.split('\t').collect();
            if fields[2].is_empty() {
                if fields[1] == "unsupported" {
                    unsupported += 1;
                }
                let text = format!(
                    "INVENTORY-ONLY\t{}\t{}: {}\n",
                    fields[0], fields[1], fields[3]
                );
                print!("{text}");
                output.push_str(&text);
            }
        }
    }
    if let Some(path) = report {
        fs::write(&path, output).unwrap();
        println!("Detailed report: {path}");
    }
    std::process::exit(
        if counts[1..].iter().sum::<usize>() == 0 && unsupported == 0 {
            0
        } else {
            1
        },
    );
}
