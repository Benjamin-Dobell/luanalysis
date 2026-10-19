//! Global guard subjects must use the same environment addresses as their reads.
use luanalysis::project_check::{
    ProjectCheckOptions, check_project_with_options, static_require_requests,
};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis::{CheckReport, check};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

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

static NEXT: AtomicU64 = AtomicU64::new(0);
const STANDARD_PROFILES: [StandardEnvironmentProfile; 2] = [
    StandardEnvironmentProfile::Lua52,
    StandardEnvironmentProfile::Lua54,
];

struct StandardFixture(PathBuf);
impl StandardFixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "luanalysis guarded standard globals {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn check(&self, source: &str, environment: StandardEnvironmentProfile) -> CheckReport {
        let entry = self.0.join("main.lua");
        fs::write(&entry, source).unwrap();
        let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
        let project = Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([entry]),
            static_require_requests,
        );
        let report = check_project_with_options(
            project,
            ProjectCheckOptions {
                environment,
                ..Default::default()
            },
        );
        assert!(report.environment_errors.is_empty(), "{report:#?}");
        let main = report.project.entries[0];
        for file in &report.files {
            if file.file != main {
                assert!(file.report.diagnostics.is_empty(), "{report:#?}");
            }
        }
        report.file(main).unwrap().report.clone()
    }
}
impl Drop for StandardFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn clean_standard(source: &str, environment: StandardEnvironmentProfile) -> CheckReport {
    let report = StandardFixture::new().check(source, environment);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn standard_error(source: &str, environment: StandardEnvironmentProfile, code: &str) {
    let report = StandardFixture::new().check(source, environment);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].code, code,
        "{:#?}",
        report.diagnostics
    );
}

fn diagnostics_at(report: &CheckReport, source: &str, expected: &[(&str, &str, &str)]) {
    assert_eq!(
        report.diagnostics.len(),
        expected.len(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    for (code, occurrence, expression) in expected {
        let start = source.find(occurrence).unwrap() + occurrence.find(expression).unwrap();
        let end = start + expression.len();
        assert!(
            report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == *code
                    && diagnostic.span.start == start
                    && diagnostic.span.end == end
            }),
            "expected {code} at {start}..{end} ({expression}): {:#?}",
            report.diagnostics
        );
    }
}

fn binding_type(report: &CheckReport, name: &str, expected: &str) {
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == name)
            .map(|binding| binding.ty.as_str()),
        Some(expected),
        "{name}: {report:#?}"
    );
}

#[test]
fn original_global_dot_and_index_nil_guards_narrow_both_branches() {
    let source = format!(
        r#"{OBJECT}
function inspectGlobal()
    if _G.flowObject ~= nil then
        local presentDot=_G['flowObject']
        _G['flowObject'].deal(1)
    else
        ---@type nil
        local absentDot=_G['flowObject']
    end
    if _G['flowObject'] == nil then
        ---@type nil
        local absentIndex=_G.flowObject
    else
        local presentIndex=_G.flowObject
        _G.flowObject.deal(1)
    end
end
"#
    );
    for profile in STANDARD_PROFILES {
        let report = clean_standard(&source, profile);
        binding_type(&report, "presentDot", "FlowObject");
        binding_type(&report, "presentIndex", "FlowObject");
        binding_type(&report, "absentDot", "nil");
        binding_type(&report, "absentIndex", "nil");
    }
}

#[test]
fn original_global_guards_and_reads_share_the_bare_global_address() {
    let source = format!(
        r#"{OBJECT}
function inspectGlobal()
    if _G.flowObject ~= nil then
        local bare=flowObject
        flowObject.deal(1)
    else
        ---@type nil
        local absentBare=flowObject
    end
    if flowObject ~= nil then
        local explicit=_G['flowObject']
        _G['flowObject'].deal(1)
    else
        ---@type nil
        local absentExplicit=_G.flowObject
    end
end
"#
    );
    for profile in STANDARD_PROFILES {
        let report = clean_standard(&source, profile);
        binding_type(&report, "bare", "FlowObject");
        binding_type(&report, "explicit", "FlowObject");
        binding_type(&report, "absentBare", "nil");
        binding_type(&report, "absentExplicit", "nil");
    }
}

#[test]
fn original_global_truthiness_early_returns_and_predicate_calls_keep_the_same_subject() {
    let source = format!(
        r#"{OBJECT}
---@param value FlowObject | nil
---@return value is FlowObject
local function isFlowObject(value) return value ~= nil end
function truthyGlobal()
    if _G.flowObject then
        local truthy=flowObject
        flowObject.deal(1)
    else
        ---@type nil
        local falsey=_G['flowObject']
    end
end
function earlyGlobal()
    if not _G['flowObject'] then return end
    local afterReturn=flowObject
    _G.flowObject.deal(1)
end
function predicateGlobal()
    if isFlowObject(_G.flowObject) then
        local predicate=flowObject
        flowObject.deal(1)
    else
        ---@type nil
        local opposite=flowObject
    end
end
function assertedGlobal()
    assert(_G.flowObject ~= nil)
    local asserted=flowObject
    flowObject.deal(1)
end
function assertedPredicateGlobal()
    assert(isFlowObject(_G.flowObject))
    local assertedPredicate=flowObject
    flowObject.deal(1)
end
"#
    );
    for profile in STANDARD_PROFILES {
        let report = clean_standard(&source, profile);
        for name in [
            "truthy",
            "afterReturn",
            "predicate",
            "asserted",
            "assertedPredicate",
        ] {
            binding_type(&report, name, "FlowObject");
        }
        for name in ["falsey", "opposite"] {
            binding_type(&report, name, "nil");
        }
    }
}

#[test]
fn an_original_global_loop_guard_narrows_the_body_before_a_global_write() {
    let source = format!(
        r#"{OBJECT}
function drainGlobal()
    while _G.flowObject ~= nil do
        local inside=flowObject
        flowObject.deal(1)
        _G.flowObject=nil
    end
end
"#
    );
    let report = clean_standard(&source, StandardEnvironmentProfile::Lua54);
    binding_type(&report, "inside", "FlowObject");
}

#[test]
fn writes_through_either_global_spelling_reset_the_other_spellings_guard() {
    for (guard, write, read) in [
        ("_G.flowObject", "flowObject = nil", "_G.flowObject"),
        ("flowObject", "_G['flowObject'] = nil", "flowObject"),
    ] {
        let source = format!(
            r#"{OBJECT}
function inspectGlobal()
    if {guard} ~= nil then
        {write}
        {read}.deal(1)
    end
end
"#
        );
        for profile in STANDARD_PROFILES {
            standard_error(&source, profile, "TS2339");
        }
    }
}

#[test]
fn rebinding_the_global_view_after_a_guard_does_not_narrow_its_new_receiver() {
    let source = format!(
        r#"{OBJECT}
function inspectGlobal()
    if _G.flowObject ~= nil then
        _G={{flowObject=maybeFlowObject()}}
        _G.flowObject.deal(1)
    end
end
"#
    );
    let report = StandardFixture::new().check(&source, StandardEnvironmentProfile::Lua54);
    diagnostics_at(
        &report,
        &source,
        &[
            (
                "TS2322",
                "_G={flowObject=maybeFlowObject()}",
                "{flowObject=maybeFlowObject()}",
            ),
            ("TS2339", "_G.flowObject.deal(1)", "deal"),
        ],
    );
}

#[test]
fn an_asserted_parenthesized_global_view_keeps_its_physical_environment_identity() {
    let source = format!(
        r#"{OBJECT}
function inspectGlobal()
    if (--[[---@type {{flowObject: FlowObject | nil}}]] _G)['flowObject'] ~= nil then
        local physical=flowObject
        flowObject.deal(1)
    end
end
"#
    );
    let report = clean_standard(&source, StandardEnvironmentProfile::Lua54);
    binding_type(&report, "physical", "FlowObject");
}

#[test]
fn shadowed_and_rebound_global_views_do_not_guard_the_original_environment() {
    for (prefix, declaration, rebound) in [
        (
            "local _G={flowObject=maybeFlowObject()}",
            "local function inspectGlobal()",
            false,
        ),
        (
            "_G={flowObject=maybeFlowObject()}",
            "local function inspectGlobal()",
            true,
        ),
        (
            "---@param _G {flowObject: FlowObject | nil}",
            "local function inspectGlobal(_G)",
            false,
        ),
    ] {
        let source = format!(
            r#"{OBJECT}
{prefix}
{declaration}
    if _G.flowObject ~= nil then
        ---@type FlowObject
        local own=_G.flowObject
        flowObject.deal(1)
    end
end
"#
        );
        for profile in STANDARD_PROFILES {
            let report = StandardFixture::new().check(&source, profile);
            let mut expected = vec![("TS2339", "flowObject.deal(1)", "deal")];
            if rebound {
                expected.push(("TS2322", prefix, "{flowObject=maybeFlowObject()}"));
            }
            diagnostics_at(&report, &source, &expected);
            binding_type(&report, "own", "FlowObject");
        }
    }
}

#[test]
fn a_saved_original_global_view_guards_the_original_after_global_view_rebinding() {
    let source = format!(
        r#"{OBJECT}
local original=_G
_G={{flowObject=maybeFlowObject()}}
function inspectGlobal()
    if original['flowObject'] ~= nil then
        local present=flowObject
        flowObject.deal(1)
    else
        ---@type nil
        local absent=flowObject
    end
end
"#
    );
    for profile in STANDARD_PROFILES {
        let report = StandardFixture::new().check(&source, profile);
        diagnostics_at(
            &report,
            &source,
            &[(
                "TS2322",
                "_G={flowObject=maybeFlowObject()}",
                "{flowObject=maybeFlowObject()}",
            )],
        );
        binding_type(&report, "present", "FlowObject");
        binding_type(&report, "absent", "nil");
    }
}

#[test]
fn a_computed_key_call_cannot_guard_a_rebound_global_receiver() {
    let source = format!(
        r#"{OBJECT}
local replacement={{flowObject=maybeFlowObject()}}
---@return 'flowObject'
local function replaceGlobalView()
    _G=replacement
    return 'flowObject'
end
function inspectGlobal()
    if _G[replaceGlobalView()] ~= nil then
        ---@type FlowObject
        local stale=_G.flowObject
    end
end
"#
    );
    for profile in STANDARD_PROFILES {
        let report = StandardFixture::new().check(&source, profile);
        diagnostics_at(
            &report,
            &source,
            &[
                ("TS2322", "_G=replacement", "replacement"),
                ("TS2322", "local stale=_G.flowObject", "_G.flowObject"),
            ],
        );
    }
}
