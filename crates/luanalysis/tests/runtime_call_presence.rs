use luanalysis::check;

fn clean(source: &str) -> luanalysis::CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}
fn rejects_number(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"), "{source}\n{:#?}", report.diagnostics);
}

#[test]
/// As in TypeScript, a call does not change the caller's view of a table: the
/// callee's stores are not replayed into the caller's flow.
fn a_called_installer_does_not_change_the_callers_table_view() {
    let source = "local function make()\n local value={}\n local function install() value.count=1 end\n install()\n local after=value.count\n return after,value\nend\nlocal after,value=make()\n---@type nil\nlocal absent=after\n---@type nil\nlocal returned=value.count\n";
    clean(source);
    rejects_number(&source.replace("---@type nil\nlocal absent", "---@type number\nlocal absent"));
    rejects_number(&source.replace("---@type nil\nlocal returned", "---@type number\nlocal returned"));
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
fn reusable_callback_returns_do_not_publish_one_installer_invocations_heap() {
    let source = "local record={\n---@type number|string\ncurrent=1,\n---@type number|string\nsaved=1,\n---@type number\nmarker=0,\n}\n---@param callback fun():number|string\n---@return number\nlocal function accept(callback) local ignored=record return 1 end\nlocal function make()\n local function install()\n  record.saved=record.current\n  record.marker=accept(install)\n  return record.saved\n end\n record.current='word'\n install()\n record.current=42\n---@type string\n local wrong=install()\n return record\nend\nlocal outcome=make()\n";
    // v1 preserves constructor field literals; this unrelated marker is
    // explicitly mutable so its write cannot mask the return-cache control.
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(&source[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "install()");
    clean(&source.replace("---@type string\n local wrong", "---@type number|string\n local corrected"));
}

