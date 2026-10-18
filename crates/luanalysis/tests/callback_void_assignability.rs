use luanalysis::{CheckReport, check};
fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}
const CONSUME: &str =
    "---@param callback fun():void\nlocal function consume(callback) callback() end\n";
#[test]
fn void_callback_targets_accept_value_returning_original_functions() {
    for callback in [
        "function() return 42 end",
        "function() return 'text' end",
        "function() return 1,'two' end",
    ] {
        clean(&format!("{CONSUME}consume({callback})\n"));
    }
    clean(&format!(
        "{CONSUME}local function returnedNumber() return 42 end\nconsume(returnedNumber)\n"
    ));
}
#[test]
fn ignored_callback_results_do_not_waive_parameter_or_nonvoid_return_contracts() {
    for source in [
        "---@param callback fun():number\nlocal function consume(callback) local value=callback() return value end\nconsume(function() return 'wrong' end)\n",
        "---@param callback fun(value:number):void\nlocal function consume(callback) callback(1) end\n---@param value string\nlocal function callback(value) end\nconsume(callback)\n",
    ] {
        let report = check(source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "TS2345" | "TS2322" | "TS2769")),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn void_callback_compatibility_does_not_relax_written_body_returns_or_never_targets() {
    let written = "---@return void\nlocal function written() return 42 end\n";
    let report = check(written);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
    for output in ["never", "nil", "(number,string)", "void|string"] {
        let source = format!(
            "---@param callback fun():{output}\nlocal function consume(callback) callback() end\nconsume(function() return 42 end)\n"
        );
        let report = check(&source);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|error| matches!(error.code, "TS2345" | "TS2322" | "TS2769")),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn actual_cli_keeps_bad_callback_parameter_error_and_accepts_original_source_correction() {
    use std::{
        fs,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-callback-void-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("original callback.lua");
    let source = "---@param callback fun(value:number):void\nlocal function consume(callback) callback(1) end\n---@param value string\nlocal function callback(value) return value end\nconsume(callback)\n";
    fs::write(&path, source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--no-stdlib"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let failed = run();
    let errors = String::from_utf8_lossy(&failed.stderr);
    assert_eq!(
        failed.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&failed.stdout),
        errors
    );
    assert!(
        errors.contains("original callback.lua:5:") && errors.contains("TS2345"),
        "{errors}"
    );
    fs::write(
        &path,
        source.replace("---@param value string", "---@param value number"),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&corrected.stdout),
        String::from_utf8_lossy(&corrected.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}
