use luanalysis::check;

fn clean(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
}
fn rejects_number(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"), "{source}\n{:#?}", report.diagnostics);
}

#[test]
fn a_reached_original_installer_changes_the_actual_returned_table() {
    let source = "local function make()\n local value={}\n local function install() value.count=1 end\n local before=value.count\n install()\n local after=value.count\n return before,after,value\nend\nlocal before,after,value=make()\n---@type nil\nlocal absent=before\n---@type number\nlocal reached=after\n---@type number\nlocal exported=value.count\n";
    clean(source);
    rejects_number(&source.replace("---@type nil", "---@type number"));
    rejects_number(&source.replace("local exported=value.count", "local exported='wrong'"));
}

#[test]
fn an_uncalled_installer_does_not_publish_its_future_store() {
    let source = "local value={}\nlocal function install() value.count=1 end\n---@type nil\nlocal absent=value.count\n";
    clean(source);
    rejects_number(&source.replace("---@type nil", "---@type number"));
}

#[test]
fn normal_completions_preserve_conditional_and_early_return_absence() {
    for body in ["if flag then return end\nvalue.count=1", "if flag then value.count=1 end"] {
        let source = format!("---@param flag boolean\nlocal function make(flag)\n local value={{}}\n local function install()\n{body}\n end\n install()\n return value\nend\nlocal value=make(true)\n---@type number\nlocal unproven=value.count\n");
        rejects_number(&source);
        clean(&source.replace("local unproven=value.count", "local proven=value.count or 0"));
    }
}

#[test]
fn a_callee_rhs_reads_the_callers_current_heap_value() {
    let source = "local value={\n---@type number|string\ncurrent=1,\n}\nlocal function install() value.saved=value.current end\nvalue.current='word'\ninstall()\n---@type string\nlocal current=value.saved\n";
    clean(source);
    rejects_number(&source.replace("---@type string\nlocal current", "---@type number\nlocal current"));
}

#[test]
fn repeated_original_calls_keep_each_actual_entry_state_separate() {
    let source = "local value={\n---@type number|string\ncurrent=1,\n}\nlocal function install() value.saved=value.current end\ninstall()\nlocal first=value.saved\nvalue.current='word'\ninstall()\nlocal second=value.saved\n---@type number\nlocal numeric=first\n---@type string\nlocal text=second\n";
    clean(source);
    rejects_number(&source.replace("---@type string\nlocal text", "---@type number\nlocal text"));
    rejects_number(&source.replace("---@type number\nlocal numeric", "---@type string\nlocal numeric"));
}

#[test]
fn original_callee_order_keeps_nil_deletion_and_later_replacement() {
    let source = "local value={\n---@type number|nil\ncount=1,\n}\nlocal function erase() value.count=nil end\nerase()\n---@type nil\nlocal gone=value.count\nlocal function restore() value.count=2 end\nrestore()\n---@type number\nlocal restored=value.count\n";
    clean(source);
    rejects_number(&source.replace("---@type nil", "---@type number"));
    rejects_number(&source.replace("---@type number\nlocal restored", "---@type string\nlocal restored"));
}

#[test]
fn an_unproved_nested_call_cannot_export_an_earlier_new_field_as_definite() {
    let source = "---@param mutate fun(value:table):void\nlocal function make(mutate)\n local value={}\n local function install()\n  value.count=1\n  mutate(value)\n end\n install()\n return value\nend\nlocal value=make(function(value) end)\n---@type number\nlocal unproven=value.count\n";
    rejects_number(source);
    clean(&source.replace("---@type number\nlocal unproven=value.count", "value.count=2\n---@type number\nlocal proven=value.count"));
}

#[test]
fn a_call_cannot_export_a_store_intercepted_by_an_actual_metatable() {
    let directory = std::env::temp_dir().join(format!("luanalysis-runtime-call-metatable-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    let source = "local value={}\nsetmetatable(value,{__newindex=function() end})\nlocal function install() value.count=1 end\ninstall()\n---@type number\nlocal unproven=value.count\n";
    let invoke = |source: &str| {
        std::fs::write(&path, source).unwrap();
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&directory)
            .args(["check", "--lua-version", "5.4", "main.lua"])
            .output().unwrap()
    };
    let wrong = invoke(source);
    let errors = String::from_utf8_lossy(&wrong.stderr);
    assert_eq!(wrong.status.code(), Some(1), "{errors}");
    assert!(errors.contains("TS2322"), "{errors}");
    let corrected = invoke(&source.replace("setmetatable(value,{__newindex=function() end})\n", ""));
    assert!(corrected.status.success(), "{}", String::from_utf8_lossy(&corrected.stderr));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reusable_callback_returns_do_not_publish_one_installer_invocations_heap() {
    let source = "local record={\n---@type number|string\ncurrent=1,\n---@type number|string\nsaved=1,\nmarker=0,\n}\n---@param callback fun():number|string\n---@return number\nlocal function accept(callback) local ignored=record return 1 end\nlocal function make()\n local function install()\n  record.saved=record.current\n  record.marker=accept(install)\n  return record.saved\n end\n record.current='word'\n install()\n record.current=42\n---@type string\n local wrong=install()\n return record\nend\nlocal outcome=make()\n";
    rejects_number(source);
    clean(&source.replace("---@type string\n local wrong", "---@type number|string\n local corrected"));
}

#[test]
fn an_original_missing_read_keeps_real_index_effects_and_its_frozen_receiver() {
    let directory = std::env::temp_dir().join(format!("luanalysis-runtime-call-index-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    let attach = "setmetatable(original,{__index=function() setmetatable(original,{__newindex=function() end}) end})\n";
    for read in [
        "local ignored=value.count\n",
        "local function key() value={} return 'count' end\nlocal ignored=value[key()]\n",
    ] {
        let source = format!("local original={{}}\nlocal value=original\n{attach}local function install() original.count=1 end\n{read}install()\n---@type number\nlocal unproven=original.count\n");
        let invoke = |source: &str| {
            std::fs::write(&path, source).unwrap();
            std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
                .current_dir(&directory)
                .args(["check", "--lua-version", "5.4", "main.lua"])
                .output().unwrap()
        };
        let wrong = invoke(&source);
        let errors = String::from_utf8_lossy(&wrong.stderr);
        assert_eq!(wrong.status.code(), Some(1), "{errors}");
        assert!(errors.contains("TS2322"), "{errors}");
        let corrected = source.replace(attach, "").replace("value[key()]", "value.count");
        let fixed = invoke(&corrected);
        assert!(fixed.status.success(), "{}", String::from_utf8_lossy(&fixed.stderr));
    }
    std::fs::remove_dir_all(directory).unwrap();
}
