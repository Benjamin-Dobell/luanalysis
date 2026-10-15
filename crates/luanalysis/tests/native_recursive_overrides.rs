use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

#[test]
fn recursive_ambient_parameter_overrides_report_the_contract_error_without_schema_recursion() {
    for returned in ["number", "self"] {
        for callable in [false, true] {
            for narrowed in ["Derived", "Derived|Sibling"] {
                let directory = std::env::temp_dir().join(format!(
                    "luanalysis recursive override {} {}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed),
                ));
                fs::create_dir_all(&directory).unwrap();
                let library = directory.join("original.def.lua");
                let main = directory.join("main.lua");
                fs::write(&main, "---@type Derived\nlocal value\n").unwrap();
                let heads = if callable {
                    "---@overload fun():number\n"
                } else {
                    ""
                };
                let source = format!(
                    "---@class Base\n{heads}Base={{}}\n---@class Derived:Base\nDerived={{}}\n---@class Sibling:Base\nSibling={{}}\n---@param object Base\n---@return {returned}\nfunction Base.putObject(object) end\n---@param object {narrowed}\n---@return {returned}\nfunction Derived.putObject(object) end\n",
                );
                let run = || {
                    Command::new(env!("CARGO_BIN_EXE_luanalysis"))
                        .args(["check", "--no-stdlib", "--format", "json", "--library"])
                        .arg(&library)
                        .arg(&main)
                        .output()
                        .unwrap()
                };
                fs::write(&library, &source).unwrap();
                let output = run();
                let report = String::from_utf8(output.stdout).unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(1),
                    "{returned}, callable={callable}: {report}"
                );
                assert_eq!(report.matches("\"code\":\"TS2416\"").count(), 1, "{report}");
                assert!(!report.contains("TS2589"), "{report}");
                assert!(!report.contains("LUA2031"), "{report}");
                fs::write(
                    &library,
                    source.replace("---@param object Derived", "---@param object Base"),
                )
                .unwrap();
                let output = run();
                assert!(
                    output.status.success(),
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                fs::remove_dir_all(directory).unwrap();
            }
        }
    }
}
