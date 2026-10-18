use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis incremental cli {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        path
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"));
        command.current_dir(&self.0).arg("check");
        command
    }

    fn watch(&self, args: &[&str]) -> WatchProcess {
        let mut child = self
            .command()
            .args(["--watch", "--format", "json", "--jobs", "1"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (sender, reports) = mpsc::channel();
        let stdout_reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let stderr_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).unwrap();
            bytes
        });
        WatchProcess {
            child,
            reports,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct WatchProcess {
    child: Child,
    reports: Receiver<String>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
}

impl WatchProcess {
    fn report(&mut self, exit: i32, files: usize) -> String {
        let report = self
            .reports
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|error| {
                panic!(
                    "watch report not delivered: {error}; process: {:?}",
                    self.child.try_wait()
                )
            });
        assert!(
            report.starts_with("{\"formatVersion\":1,") && report.ends_with('}'),
            "{report}"
        );
        assert!(report.contains(&format!("\"exitCode\":{exit}")), "{report}");
        assert!(
            report.contains(&format!("\"filesChecked\":{files}")),
            "{report}"
        );
        if exit == 0 {
            assert!(
                report.contains("\"success\":true") && report.contains("\"diagnostics\":[]"),
                "{report}"
            );
        }
        report
    }

    fn unchanged(&self) {
        assert!(
            matches!(
                self.reports.recv_timeout(Duration::from_millis(800)),
                Err(RecvTimeoutError::Timeout)
            ),
            "unchanged sources emitted another watch revision"
        );
    }
}

impl Drop for WatchProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let bytes = reader.join().unwrap_or_default();
            if !thread::panicking() {
                assert!(bytes.is_empty(), "{}", String::from_utf8_lossy(&bytes));
            }
        }
    }
}

fn json(output: Output, exit: i32) -> String {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(
        report.starts_with("{\"formatVersion\":1,") && report.ends_with("}\n"),
        "{report}"
    );
    report
}

#[test]
fn jobs_requires_a_positive_integer_and_preserves_normal_reports() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "---@type number\nlocal value='wrong'\n");
    for count in ["0", "-1", "+1", "1.5", "many", "184467440737095516160"] {
        let report = json(
            fixture
                .command()
                .args(["--jobs", count, "--format", "json", "main.lua"])
                .output()
                .unwrap(),
            2,
        );
        assert!(
            report.contains("--jobs requires a positive integer")
                && report.contains("\"code\":\"LUA0000\""),
            "{report}"
        );
    }
    let missing = json(
        fixture
            .command()
            .args(["--format", "json", "--jobs"])
            .output()
            .unwrap(),
        2,
    );
    assert!(
        missing.contains("--jobs requires a positive integer"),
        "{missing}"
    );
    let serial = json(
        fixture
            .command()
            .args([
                "--format",
                "json",
                "--jobs",
                "1",
                "--dump-types",
                "main.lua",
            ])
            .output()
            .unwrap(),
        1,
    );
    let parallel = json(
        fixture
            .command()
            .args([
                "--format",
                "json",
                "--jobs",
                "2",
                "--dump-types",
                "main.lua",
            ])
            .output()
            .unwrap(),
        1,
    );
    assert_eq!(serial, parallel);
}

#[test]
fn watch_detects_equal_length_equal_mtime_source_edits_and_file_recovery() {
    let fixture = Fixture::new();
    let wrong = "---@type number\nlocal value='a'\n";
    let correct = "---@type number\nlocal value=123\n";
    assert_eq!(wrong.len(), correct.len());
    let source = fixture.file("main.lua", wrong);
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    let mut watch = fixture.watch(&["--dump-types", "main.lua"]);
    assert!(watch.report(1, 1).contains("\"code\":\"TS2322\""));
    watch.unchanged();
    fs::write(&source, correct).unwrap();
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let corrected = watch.report(0, 1);
    assert!(
        corrected.contains("\"name\":\"value\",\"type\":\"number\""),
        "{corrected}"
    );
    let cold = json(
        fixture
            .command()
            .args([
                "--format",
                "json",
                "--jobs",
                "1",
                "--dump-types",
                "main.lua",
            ])
            .output()
            .unwrap(),
        0,
    );
    assert_eq!(
        corrected,
        cold.trim_end(),
        "watch differs from a fresh process after correction"
    );
    watch.unchanged();
    fs::remove_file(&source).unwrap();
    assert!(watch.report(2, 0).contains("\"code\":\"LUA0001\""));
    fs::write(&source, wrong).unwrap();
    assert!(watch.report(1, 1).contains("\"code\":\"TS2322\""));
    fs::write(&source, correct).unwrap();
    watch.report(0, 1);
}

#[test]
fn watch_reloads_declarations_and_discovers_added_and_deleted_sources() {
    let fixture = Fixture::new();
    fixture.file("src/main.lua", "---@type Count\nlocal value=42\n");
    let declaration = fixture.file("types/count.def.lua", "---@alias Count number\n");
    let mut watch = fixture.watch(&["--library", "types", "src"]);
    watch.report(0, 2);
    fs::write(&declaration, "---@alias Count string\n").unwrap();
    assert!(watch.report(1, 2).contains("\"code\":\"TS2322\""));
    fs::write(&declaration, "---@alias Count number\n").unwrap();
    watch.report(0, 2);
    let added = fixture.file("src/added.ttslua", "---@type number\nlocal added='wrong'\n");
    assert!(watch.report(1, 3).contains("\"code\":\"TS2322\""));
    fs::remove_file(added).unwrap();
    watch.report(0, 2);
    fs::remove_file(&declaration).unwrap();
    assert!(watch.report(1, 1).contains("\"code\":\"TS2304\""));
    fs::write(&declaration, "---@alias Count number\n").unwrap();
    watch.report(0, 2);
}

#[test]
fn watch_retries_missing_modules_and_observes_package_path_priority_changes() {
    let fixture = Fixture::new();
    fixture.file(
        "main.lua",
        "local module=require('later')\n---@type number\nlocal value=module.value\n",
    );
    let mut watch = fixture.watch(&[
        "--module-root",
        "modules",
        "--package-path",
        "?/init.lua;?.lua",
        "main.lua",
    ]);
    assert!(watch.report(1, 1).contains("\"code\":\"TS2307\""));
    let fallback = fixture.file("modules/later.lua", "return {value=42}\n");
    let corrected = watch.report(0, 2);
    let cold = json(
        fixture
            .command()
            .args([
                "--format",
                "json",
                "--jobs",
                "1",
                "--module-root",
                "modules",
                "--package-path",
                "?/init.lua;?.lua",
                "main.lua",
            ])
            .output()
            .unwrap(),
        0,
    );
    assert_eq!(
        corrected,
        cold.trim_end(),
        "watch differs from a fresh process after module creation"
    );
    fs::write(&fallback, "return {value='wrong'}\n").unwrap();
    assert!(watch.report(1, 2).contains("\"code\":\"TS2322\""));
    let preferred = fixture.file("modules/later/init.lua", "return {value=42}\n");
    watch.report(0, 2);
    fs::remove_file(preferred).unwrap();
    assert!(watch.report(1, 2).contains("\"code\":\"TS2322\""));
    fs::write(&fallback, "return {value=42}\n").unwrap();
    watch.report(0, 2);
    fs::remove_file(fallback).unwrap();
    assert!(watch.report(1, 1).contains("\"code\":\"TS2307\""));
}

#[test]
fn watch_recovers_from_invalid_utf8_without_rechecking_unchanged_invalid_bytes() {
    let fixture = Fixture::new();
    let source = fixture.file("main.lua", "");
    fs::write(&source, [0xff]).unwrap();
    let mut watch = fixture.watch(&["main.lua"]);
    assert!(watch.report(2, 0).contains("\"code\":\"LUA0001\""));
    watch.unchanged();
    fs::write(&source, "local value=42\n").unwrap();
    watch.report(0, 1);
    fs::write(&source, [0xfe]).unwrap();
    assert!(watch.report(2, 0).contains("\"code\":\"LUA0001\""));
    watch.unchanged();
    fs::write(&source, "local value=42\n").unwrap();
    watch.report(0, 1);
}
