//! Global guard subjects must use the same environment addresses as their reads.
use luanalysis::{CheckReport, check};

const OBJECT: &str = r#"---@class FlowObject
---@field deal fun(count: number): boolean

---@return FlowObject | nil
local function maybeFlowObject()
    return nil
end

flowObject = maybeFlowObject()
"#;

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn narrowed_type(report: &CheckReport, source: &str, declaration: &str, expected: &str) {
    let offset = source.find(declaration).unwrap() + declaration.find('=').unwrap() + 1;
    assert_eq!(report.type_at(offset), Some(expected), "{report:#?}");
}

#[test]
fn nil_guards_narrow_globals_at_top_level_and_inside_functions() {
    for (before, after) in [
        ("", ""),
        ("function guardedGlobal()\n", "end\n"),
        (
            "local function outer()\nlocal function inner()\n",
            "end\nend\n",
        ),
    ] {
        let source = format!(
            r#"{OBJECT}{before}
if flowObject ~= nil then
    local present=flowObject
    flowObject.deal(1)
else
    local absent=flowObject
    ---@type nil
    local checked=flowObject
end
{after}"#
        );
        let report = clean(&source);
        narrowed_type(&report, &source, "local present=flowObject", "FlowObject");
        narrowed_type(&report, &source, "local absent=flowObject", "nil");
    }
}

#[test]
fn truthiness_and_early_return_guards_narrow_written_global_contracts() {
    for guard in [
        "if not flowObject then return end",
        "if flowObject == nil then return end",
    ] {
        let source = format!(
            r#"{OBJECT}
---@type FlowObject | nil
writtenObject = maybeFlowObject()
function guardedGlobal()
    {guard}
    local present=flowObject
    flowObject.deal(1)
    if writtenObject then
        local written=writtenObject
        writtenObject.deal(1)
    end
end
"#
        );
        let report = clean(&source);
        narrowed_type(&report, &source, "local present=flowObject", "FlowObject");
        narrowed_type(
            &report,
            &source,
            "local written=writtenObject",
            "FlowObject",
        );
    }
}

#[test]
fn runtime_type_guards_narrow_global_values() {
    let source = r#"---@type number | string
globalValue = --[[---@type number | string]] 42
function inspectGlobal()
    if type(globalValue) == 'string' then
        ---@type string
        local text=globalValue
    else
        ---@type number
        local number=globalValue
    end
end
"#;
    let report = clean(source);
    narrowed_type(&report, source, "local text=globalValue", "string");
    narrowed_type(&report, source, "local number=globalValue", "number");
}

#[test]
fn explicit_type_predicate_guards_narrow_global_arguments() {
    let source = format!(
        r#"{OBJECT}
---@param value FlowObject | nil
---@return value is FlowObject
local function isFlowObject(value)
    return value ~= nil
end
function inspectGlobal()
    if isFlowObject(flowObject) then
        local present=flowObject
        flowObject.deal(1)
    else
        ---@type nil
        local absent=flowObject
    end
end
"#
    );
    let report = clean(&source);
    narrowed_type(&report, &source, "local present=flowObject", "FlowObject");
    narrowed_type(&report, &source, "local absent=flowObject", "nil");
}

#[test]
fn a_global_write_after_its_guard_invalidates_the_narrowing() {
    let source = format!(
        r#"{OBJECT}
function inspectGlobal()
    if flowObject ~= nil then
        flowObject = nil
        flowObject.deal(1)
    end
end
"#
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
}

#[test]
fn a_guard_on_another_global_does_not_narrow_the_receiver() {
    let source = format!(
        r#"{OBJECT}
otherObject = maybeFlowObject()
function inspectGlobal()
    if otherObject ~= nil then
        flowObject.deal(1)
    end
end
"#
    );
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
}

#[test]
fn a_lexical_environment_guard_keeps_its_own_reference() {
    let source = format!(
        r#"{OBJECT}
local original=flowObject
local _ENV={{flowObject=original}}
if flowObject ~= nil then
    local present=flowObject
    flowObject.deal(1)
end
"#
    );
    let report = clean(&source);
    narrowed_type(&report, &source, "local present=flowObject", "FlowObject");

    let replaced = source.replace(
        "local present=flowObject",
        "flowObject=nil\n    local present=flowObject",
    );
    let report = check(&replaced);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
}
