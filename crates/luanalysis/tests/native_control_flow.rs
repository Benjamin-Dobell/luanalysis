//! End-to-end original-Lua source gates for the shared native flow worker.
//! These exercise the public checker/CLI, rather than joining graph types here.
use luanalysis::{CheckReport, check};
use std::{fs, process::Command};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn binding_type<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap_or_else(|| panic!("missing binding {name}: {report:#?}"))
        .ty
}

fn one_assignment_error(source: &str, rejected: &str) -> CheckReport {
    let report = check(source);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2322", "{diagnostic:#?}");
    assert_eq!(
        &source[diagnostic.span.start..diagnostic.span.end],
        rejected
    );
    report
}

#[test]
fn literal_conditions_use_lua_truthiness_and_only_reachable_returns() {
    for (condition, expected, opposite) in [
        ("0", "number", "string"),
        ("\"\"", "number", "string"),
        ("nil", "string", "number"),
        ("false", "string", "number"),
    ] {
        let source = format!(
            "local function choose()\nif {condition} then return 42 else return \"other\" end\nend\nlocal result=choose()\n---@type {expected}\nlocal accepted=result\n---@type {opposite}\nlocal rejected=result"
        );
        let report = one_assignment_error(&source, "result");
        assert_eq!(binding_type(&report, "result"), expected, "{condition}");
    }
}

#[test]
fn named_reference_truthiness_narrows_both_original_source_branches() {
    let source = "---@param value number|nil|false\nlocal function narrow(value)\nif value then\n---@type number\nlocal positive=value\nelse\n---@type nil|false\nlocal negative=value\nend\nend\nnarrow(0)\nnarrow(nil)\nnarrow(false)";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("positive=value").unwrap() + 9),
        Some("number")
    );
    let falsey = report
        .type_at(source.find("negative=value").unwrap() + 9)
        .unwrap();
    assert!(
        falsey.contains("nil") && falsey.contains("false"),
        "{falsey}"
    );
    assert!(!falsey.contains("number"), "{falsey}");
}

#[test]
fn named_equality_predicates_narrow_nil_without_lua_coercion() {
    let source = "---@param value number|nil\nlocal function narrow(value)\nif value ~= nil then\n---@type number\nlocal present=value\nelse\n---@type nil\nlocal absent=value\nend\nend\nnarrow(0)\nnarrow(nil)";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("present=value").unwrap() + 8),
        Some("number")
    );
    assert_eq!(
        report.type_at(source.find("absent=value").unwrap() + 7),
        Some("nil")
    );
}

#[test]
fn optional_field_predicates_narrow_the_original_named_access_path() {
    let source = "---@param object {child:{x:number}|nil}\nlocal function read(object)\nif object.child then\n---@type number\nlocal checked=object.child.x\nreturn checked\nend\nreturn nil\nend\nlocal result=read({child={x=1}})\n---@type number|nil\nlocal accepted=result\n---@type number\nlocal rejected=result";
    let report = one_assignment_error(source, "result");
    assert_eq!(
        report.type_at(
            source.find("checked=object.child.x").unwrap() + "checked=object.child.".len()
        ),
        Some("number")
    );
    let result = binding_type(&report, "result");
    assert!(
        result.contains("number") && result.contains("nil"),
        "{result}"
    );
}

#[test]
fn branch_assignments_join_through_native_flow_and_keep_declared_constraints() {
    let source = "---@param flag boolean\nlocal function choose(flag)\n---@type number|string\nlocal value=1\nif flag then value=\"text\" else value=2 end\nreturn value\nend\nlocal result=choose(true)\n---@type number|string\nlocal accepted=result\n---@type boolean\nlocal rejected=result";
    let report = one_assignment_error(source, "result");
    let result = binding_type(&report, "result");
    assert!(
        result.contains("number") && result.contains("string"),
        "{result}"
    );
    assert!(!result.contains("nil"), "{result}");
}

#[test]
fn loop_rhs_reenters_at_frozen_input_without_a_return_inference_cycle() {
    let source = "---@param keepGoing boolean\nlocal function append(keepGoing)\n---@type number|string\nlocal value=1\nwhile keepGoing do value=value .. \"x\" end\nreturn value\nend\nlocal first=append(true)\nlocal second=append(false)\n---@type number|string\nlocal accepted=first\n---@type boolean\nlocal rejected=second";
    let report = one_assignment_error(source, "second");
    for name in ["first", "second"] {
        let result = binding_type(&report, name);
        assert!(
            result.contains("number") && result.contains("string"),
            "{name}: {result}"
        );
        assert!(
            !result.contains("any") && !result.contains("unknown"),
            "{name}: {result}"
        );
    }
}

#[test]
fn provisional_loop_declaration_demands_do_not_publish_a_narrow_constraint() {
    let source = "---@param keepGoing boolean\n---@param chooseText boolean\nlocal function carry(keepGoing, chooseText)\n---@type number|string\nlocal value=1\nwhile keepGoing do\nlocal copied=value\nif chooseText then value=\"text\" else value=copied end\n---@type number|string\nlocal accepted=copied\n---@type number\nlocal rejected=copied\nend\nreturn value\nend\nlocal result=carry(true, false)\n---@type number|string\nlocal acceptedResult=result";
    let report = one_assignment_error(source, "copied");
    let copied = binding_type(&report, "copied");
    assert!(
        copied.contains("number") && copied.contains("string"),
        "{copied}"
    );
    let result = binding_type(&report, "result");
    assert!(
        result.contains("number") && result.contains("string"),
        "{result}"
    );
}

#[test]
fn repeat_body_runs_before_exit_and_break_keeps_its_written_type() {
    let source = "---@param stop boolean\nlocal function once(stop)\n---@type number|string\nlocal value=1\nrepeat\nlocal visible=42\nvalue=\"visited\"\nif stop then break end\nuntil visible>0\nreturn value\nend\nlocal result=once(true)\n---@type string\nlocal accepted=result\n---@type number\nlocal rejected=result";
    let report = one_assignment_error(source, "result");
    assert_eq!(binding_type(&report, "result"), "string");
    assert_eq!(
        report.type_at(source.find("visible>0").unwrap()),
        Some("number")
    );
}

#[test]
fn repeat_local_is_visible_in_until_but_does_not_escape_its_scope() {
    let source = "local function scoped()\nrepeat local visible=42 until visible>0\nreturn visible\nend\nscoped()";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2304");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "visible");
    assert_eq!(span.start, source.rfind("visible").unwrap());
}

#[test]
fn declared_never_calls_remove_later_returns_and_reachable_fallthrough() {
    let source = "---@return never\nlocal function stop() stop() end\nlocal function inferred()\nstop()\nreturn \"unreachable\"\nend\n---@return number\nlocal function declared() stop() end\nlocal result=inferred()\n---@type number\nlocal accepted=result\ndeclared()";
    let report = clean(source);
    assert_eq!(binding_type(&report, "result"), "never");
}

#[test]
fn captured_declared_never_signature_handles_keep_nonreturning_effects() {
    let source = "---@generic T\n---@param value T\nlocal function make(value)\n---@return never\nlocal function stopped()\nlocal copied=value\nstopped()\nend\nreturn stopped\nend\nlocal stopped=make(1)\n---@return number\nlocal function endpoint() stopped() end\nendpoint()";
    clean(source);
}

#[test]
fn inferred_returns_aggregate_every_reachable_branch_and_fallthrough_nil() {
    let source = "---@param flag boolean\nlocal function choose(flag)\nif flag then return 1 else return \"text\" end\nend\n---@param flag boolean\nlocal function optional(flag) if flag then return 1 end end\nlocal either=choose(true)\nlocal maybe=optional(false)\n---@type number|string\nlocal acceptedEither=either\n---@type number|nil\nlocal acceptedMaybe=maybe\n---@type number\nlocal rejected=maybe";
    let report = one_assignment_error(source, "maybe");
    let either = binding_type(&report, "either");
    assert!(
        either.contains("number") && either.contains("string"),
        "{either}"
    );
    assert!(
        !either.contains("nil") && !either.contains("void"),
        "{either}"
    );
    let maybe = binding_type(&report, "maybe");
    assert!(maybe.contains("number") && maybe.contains("nil"), "{maybe}");
    assert!(!maybe.contains("void"), "{maybe}");
}

#[test]
fn captured_generic_return_bodies_keep_distinct_source_flow_facets() {
    let source = "---@generic T\n---@param value T\n---@param choose boolean\nlocal function make(value, choose)\nreturn function()\nif choose then return value end\nreturn nil\nend\nend\nlocal numberGetter=make(1, true)\nlocal stringGetter=make(\"text\", false)\nlocal numberResult=numberGetter()\nlocal stringResult=stringGetter()\n---@type number|nil\nlocal acceptedNumber=numberResult\n---@type string|nil\nlocal acceptedString=stringResult\n---@type boolean\nlocal rejected=numberResult";
    let report = one_assignment_error(source, "numberResult");
    let number = binding_type(&report, "numberResult");
    assert!(
        number.contains("number") && number.contains("nil"),
        "{number}"
    );
    assert!(
        !number.contains("string") && !number.contains("any"),
        "{number}"
    );
    let string = binding_type(&report, "stringResult");
    assert!(
        string.contains("string") && string.contains("nil"),
        "{string}"
    );
    assert!(
        !string.contains("number") && !string.contains("any"),
        "{string}"
    );
}

#[test]
fn simultaneous_writes_in_a_branch_freeze_all_rhs_values() {
    let source = "---@param flag boolean\nlocal function swap(flag)\n---@type number|string\nlocal left=1\n---@type number|string\nlocal right=\"text\"\nif flag then\nleft,right=right,left\n---@type string\nlocal text=left\n---@type number\nlocal number=right\nend\nreturn left\nend\n---@type number|string\nlocal accepted=swap(true)";
    clean(source);
}

#[test]
fn simultaneous_owner_replacement_in_a_branch_keeps_old_field_addresses() {
    for assignment in ["item,item.x={x=\"new\"},2", "item.x,item=2,{x=\"new\"}"] {
        let source = format!(
            "---@param flag boolean\nlocal function replace(flag)\n---@type {{x:number}}|{{x:string}}\nlocal item={{x=1}}\nif flag then\n{assignment}\n---@type string\nlocal seen=item.x\nend\nreturn item.x\nend\n---@type number|string\nlocal accepted=replace(true)"
        );
        clean(&source);
    }
}

#[test]
fn iterator_predicates_are_explicitly_unsupported_in_this_named_reference_slice() {
    for source in [
        "for index=1,2 do local copy=index end",
        "local function iterator() return 1 end\nfor value in iterator do local copy=value end",
    ] {
        let report = check(source);
        assert!(
            report.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic.code,
                "LUA2020" | "LUA2023"
            ) && diagnostic
                .message
                .contains("iteration")),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn dynamic_indexed_writes_report_the_key_integration_gate() {
    let source = "---@param key string\nlocal function write(key)\nlocal record={x=1}\nrecord[key]=2\nreturn record.x\nend\nwrite(\"x\")";
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.code,
            "LUA2019" | "LUA2020"
        ) && diagnostic.message.contains("dynamic")),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn evolving_nil_initialization_reports_its_explicit_native_flow_gate() {
    for declaration in ["local value", "local value=nil"] {
        let source = format!(
            "---@param flag boolean\nlocal function initialize(flag)\n{declaration}\nif flag then value=42 end\nreturn value\nend\ninitialize(true)"
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}\n{:#?}",
            report.diagnostics
        );
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "LUA2005", "{diagnostic:#?}");
        assert!(diagnostic.message.contains("nil-initialized"));
        assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "42");
    }
}

#[test]
fn cli_reports_flow_inferred_original_lua_error_and_accepts_its_correction() {
    let directory =
        std::env::temp_dir().join(format!("luanalysis native flow {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("conditional.ttslua");
    let source = "local function choose()\nif 0 then return 42 else return \"unreachable\" end\nend\n---@type string\nlocal result=choose()\n";
    fs::write(&path, source).unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--dump-types"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1));
    let stderr = String::from_utf8(invalid.stderr).unwrap();
    assert!(
        stderr.contains("conditional.ttslua:5:14: error[TS2322]"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("LUA2020") && !stderr.contains("TS7023"),
        "{stderr}"
    );
    fs::write(&path, source.replace("---@type string", "---@type number")).unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .arg("check")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert_eq!(
        String::from_utf8(corrected.stdout).unwrap(),
        "No type errors in 1 file(s).\n"
    );
    fs::remove_dir_all(directory).unwrap();
}
