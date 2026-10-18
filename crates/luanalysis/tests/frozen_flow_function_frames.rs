use luanalysis::check;

const ORIGINAL_FACTORY: &str = "local function make()\n local t={}\n local function install()\n  function t.callback() return 1 end\n end\n install()\n return t\nend\nlocal t=make()\nlocal function run() return t.callback() end\nrun()\n";

const GENERIC_FACTORY: &str = "---@generic T\n---@param value T\nlocal function make(value)\n local t={}\n local function install()\n  ---@return T\n  function t.callback() return value end\n end\n install()\n return t\nend\n";

// Immediate reads see only stores in the caller's own flow (calls are not
// replayed, as in TypeScript), so these factories define the callback directly.
const GENERIC_DIRECT_FACTORY: &str = "---@generic T\n---@param value T\nlocal function make(value)\n local t={}\n ---@return T\n function t.callback() return value end\n return t\nend\n";

const DEFERRED_CALLS: &str = "local numbers=make(1)\nlocal strings=make('word')\nlocal function run()\n---@type number\nlocal numeric=numbers.callback()\n---@type string\nlocal text=strings.callback()\nend\nrun()\n";

#[test]
fn replaying_a_nested_installer_selects_its_original_frozen_function_frame() {
    // This original program panicked at lexical_parent_capture before the fix.
    // The assertion remains enabled; the frozen callback now owns its frame.
    let report = check(ORIGINAL_FACTORY);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn separate_generic_factory_instances_keep_their_lexical_callback_results() {
    let source = format!("{GENERIC_DIRECT_FACTORY}local numbers=make(1)\nlocal strings=make('word')\n---@type number\nlocal numeric=numbers.callback()\n---@type string\nlocal text=strings.callback()\n");
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let wrong = source.replace("local text=strings.callback()", "local text=numbers.callback()");
    let report = check(&wrong);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"), "{:#?}", report.diagnostics);
    let wrong_body = source.replace("return value end", "return 'wrong' end");
    let report = check(&wrong_body);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"), "{:#?}", report.diagnostics);
    assert!(check(&source).diagnostics.is_empty());
}

#[test]
fn imported_factory_callbacks_reject_wrong_results_then_accept_the_original_source_correction() {
    let directory = std::env::temp_dir().join(format!("luanalysis-frozen-parent-factory-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("factory.lua"), format!("{GENERIC_DIRECT_FACTORY}return make\n")).unwrap();
    let path = directory.join("main.lua");
    let corrected = "local make=require('factory')\nlocal numbers=make(42)\nlocal strings=make('word')\n---@type number\nlocal numeric=numbers.callback()\n---@type string\nlocal text=strings.callback()\n";
    let invoke = || std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .current_dir(&directory)
        .args(["check", "--no-stdlib", "--stdlib", "require", "--module-root", ".", "main.lua"])
        .output().unwrap();
    std::fs::write(&path, corrected.replace("local text=strings.callback()", "local text=numbers.callback()")).unwrap();
    let wrong = invoke();
    let errors = String::from_utf8_lossy(&wrong.stderr);
    assert_eq!(wrong.status.code(), Some(1), "{errors}");
    assert!(errors.contains("main.lua:7:") && errors.contains("TS2322"), "{errors}");
    std::fs::write(&path, corrected).unwrap();
    let fixed = invoke();
    assert!(fixed.status.success(), "{}", String::from_utf8_lossy(&fixed.stderr));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_deferred_body_keeps_distinct_generic_factories_and_checks_the_callback_body() {
    let source = format!("{GENERIC_FACTORY}{DEFERRED_CALLS}");
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    for wrong in [
        source.replace("local text=strings.callback()", "local text=numbers.callback()"),
        source.replace("return value end", "return 'wrong' end"),
    ] {
        let report = check(&wrong);
        assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"), "{:#?}", report.diagnostics);
    }
    assert!(check(&source).diagnostics.is_empty());
}

#[test]
fn imported_factory_keeps_original_deferred_body_frames_and_result_correction() {
    let directory = std::env::temp_dir().join(format!("luanalysis-frozen-deferred-import-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("factory.lua"), format!("{GENERIC_FACTORY}return make\n")).unwrap();
    let path = directory.join("main.lua");
    let corrected = format!("local make=require('factory')\n{DEFERRED_CALLS}");
    let invoke = || std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .current_dir(&directory)
        .args(["check", "--no-stdlib", "--stdlib", "require", "--module-root", ".", "main.lua"])
        .output().unwrap();
    std::fs::write(&path, corrected.replace("local text=strings.callback()", "local text=numbers.callback()")).unwrap();
    let wrong = invoke();
    let errors = String::from_utf8_lossy(&wrong.stderr);
    assert_eq!(wrong.status.code(), Some(1), "{errors}");
    assert!(errors.contains("main.lua:8:") && errors.contains("TS2322"), "{errors}");
    std::fs::write(&path, corrected).unwrap();
    let fixed = invoke();
    assert!(fixed.status.success(), "{}", String::from_utf8_lossy(&fixed.stderr));
    std::fs::remove_dir_all(directory).unwrap();
}
