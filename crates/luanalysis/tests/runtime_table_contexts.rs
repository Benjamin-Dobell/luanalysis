// Keep contextual inference regressions meaningful with missing contracts denied.
fn check(source: &str) -> luanalysis::CheckReport {
    luanalysis::check_with_options(source, luanalysis::CheckOptions {
        no_implicit_any: true,
        ..Default::default()
    })
}

#[test]
fn written_callback_fields_keep_context_through_assignment_member_paths() {
    for target in ["callbacks.map", "callbacks['map']", "(callbacks).map"] {
        let source = format!(
            "local callbacks={{\n---@type fun(value:number):number\nmap=function(value) return value end,\n}}\n{target}=function(replacement) return replacement+1 end\nlocal result=callbacks.map(42)\n"
        );
        let report = check(&source);
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
        assert_eq!(
            report.type_at(source.find("function(replacement)").unwrap() + "function(".len()),
            Some("number")
        );
        let bad = source.replace("replacement+1", "'wrong'");
        let report = check(&bad);
        assert!(
            report.diagnostics.iter().any(|error| error.code == "TS2322"),
            "{bad}\n{:#?}",
            report.diagnostics
        );
        assert!(report.diagnostics.iter().all(|error| error.code != "TS7006"));
    }
}

#[test]
fn replacing_the_root_binding_does_not_contextualize_a_distinct_allocation() {
    let source = "local record={}\nrecord={new='word'}\n---@type string\nlocal result=record.new\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let bad = source.replace("---@type string", "---@type number");
    let report = check(&bad);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
    let incompatible = "local record={old=1}\nrecord={new='word'}\n";
    let report = check(incompatible);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn invalid_constructors_keep_only_the_original_key_diagnostic() {
    for source in ["local value={[nil]=42}", "local key=nil\nlocal value={[key]=42}"] {
        let report = check(source);
        assert_eq!(report.diagnostics.len(), 1, "{source}\n{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, "TS2538");
    }
}

#[test]
fn cli_written_callback_rejects_a_wrong_initializer_and_accepts_the_source_correction() {
    let directory = std::env::temp_dir().join(format!("luanalysis-written-runtime-context-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    let corrected = "local callbacks={\n---@type fun(value:number):number\nmap=function(value) return value end,\n}\nlocal unrelated={}\nunrelated.child={}\nunrelated.child.label='ready'\ncallbacks.map=function(replacement) return replacement+1 end\n---@type number\nlocal result=callbacks.map(42)\n";
    let invoke = || std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--no-stdlib"]).arg(&path).output().unwrap();
    std::fs::write(&path, corrected.replace("replacement+1", "'wrong'")).unwrap();
    let wrong = invoke();
    let errors = String::from_utf8_lossy(&wrong.stderr);
    assert_eq!(wrong.status.code(), Some(1), "{errors}");
    assert!(errors.contains("main.lua:8:") && errors.contains("TS2322"), "{errors}");
    assert!(!errors.contains("TS7006"), "written callback parameter context must remain available: {errors}");
    std::fs::write(&path, corrected).unwrap();
    let fixed = invoke();
    assert!(fixed.status.success(), "{}", String::from_utf8_lossy(&fixed.stderr));
    std::fs::remove_dir_all(directory).unwrap();
}
