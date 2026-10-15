use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const HOST: &str = "---@return nil\nfunction hostEffect() end\n---@class HostObject\n---@field value number\n---@type HostObject\nhostObject=nil\n---@type number\nhostNumber=nil\n";
const CUSTOM: &str = "---@param value unknown\n---@return string\nfunction type(value) end\n---@param name string\n---@return number\nfunction require(name) end\n";
const ORDINARY: &str = r#"---@param name string
---@return number
local function ordinaryLoader(name) return 42 end
---@param value unknown
---@return "number"
local function ordinaryKind(value) return 'number' end

"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis standard unknown effects {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("host.def.lua"), HOST).unwrap();
        fs::write(path.join("module.lua"), "return {value=42}\n").unwrap();
        Self(path)
    }

    fn check(&self, source: &str, standard: bool, custom: Option<&str>) -> Output {
        self.check_options(source, standard, custom, false)
    }

    fn check_options(
        &self,
        source: &str,
        standard: bool,
        custom: Option<&str>,
        strict: bool,
    ) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis"));
        command.current_dir(&self.0).args([
            "check",
            "--format",
            "json",
            "--module-root",
            ".",
            "--library",
            "host.def.lua",
        ]);
        if standard {
            command.args(["--lua-version", "5.4"]);
        } else {
            command.arg("--no-stdlib");
        }
        if let Some(custom) = custom {
            fs::write(self.0.join("custom.def.lua"), custom).unwrap();
            command.args(["--library", "custom.def.lua"]);
        }
        if strict {
            command.arg("--strict-standard-effects");
        }
        command.arg("main.lua").output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn report(output: Output, exit: i32) -> String {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(exit), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("{\"formatVersion\":1,"), "{stdout}");
    stdout
}
fn clean(output: Output) {
    let report = report(output, 0);
    assert!(report.contains("\"diagnostics\":[]"), "{report}");
}
fn error_on(report: &str, source: &str, code: &str, statement: &str, expression: &str) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    let range = format!(
        "\"range\":{{\"start\":{start},\"end\":{}}}",
        start + expression.len()
    );
    let code = format!("\"code\":\"{code}\"");
    assert!(
        report
            .split("},{\"path\":")
            .any(|diagnostic| diagnostic.contains(&code) && diagnostic.contains(&range)),
        "{report}"
    );
}

fn probe(effect: &str, loader: &str, kind: &str) -> String {
    format!(
        "---@param value number|string\nlocal function probe(value)\n{effect}\nlocal module={loader}('module')\n---@type number\nlocal checked_module=module.value\nif {kind}(value)=='number' then\n---@type number\nlocal checked_type=value\nend\nend\nprobe(42)\n"
    )
}

fn trusted_controls(fixture: &Fixture, source: &str) {
    clean(fixture.check(source, true, None));
    let wrong_module = source.replace(
        "---@type number\nlocal checked_module",
        "---@type string\nlocal checked_module",
    );
    let rejected = report(fixture.check(&wrong_module, true, None), 1);
    error_on(
        &rejected,
        &wrong_module,
        "TS2322",
        "local checked_module=module.value",
        "module.value",
    );
    let wrong_type = source.replace(
        "---@type number\nlocal checked_type",
        "---@type string\nlocal checked_type",
    );
    let rejected = report(fixture.check(&wrong_type, true, None), 1);
    error_on(
        &rejected,
        &wrong_type,
        "TS2322",
        "local checked_type=value",
        "value",
    );
}

#[test]
fn unknown_host_calls_property_reads_and_arithmetic_keep_standard_assumptions() {
    let fixture = Fixture::new();
    for effect in [
        "hostEffect()",
        "local ignored=hostObject.value",
        "local ignored=hostNumber+1",
    ] {
        trusted_controls(&fixture, &probe(effect, "require", "type"));
    }
}

#[test]
fn explicit_direct_alias_and_captured_replacements_survive_later_unknown_calls() {
    let fixture = Fixture::new();
    for replacement in [
        "_G.require=ordinaryLoader\n_G.type=ordinaryKind\n",
        "local env=_G\nenv.require=ordinaryLoader\nenv.type=ordinaryKind\n",
        "local env=_G\nlocal function replace()\nenv.require=ordinaryLoader\nenv.type=ordinaryKind\nend\nreplace()\n",
    ] {
        let source = format!("{ORDINARY}{replacement}{}", probe("hostEffect()", "require", "type"));
        let rejected = report(fixture.check(&source, true, None), 1);
        error_on(
            &rejected,
            &source,
            "LUA2036",
            "local module=require('module')",
            "require",
        );
        error_on(
            &rejected,
            &source,
            "TS2322",
            "local checked_type=value",
            "value",
        );
    }
}

const EVENT_MANAGER_VIEW: &str = r#"---@vararg unknown
---@return unknown...
local function replacement(...) return ... end
---@param event string
local function listen(event)
    ;(--[[---@type table]] _G)[event]=replacement
end

---@class Manager
local Manager={}
---@param event string
function Manager.add(event)
    assert(true,'checked')
    local saved=assert
    saved(true,'saved')
    listen(event)
end
return Manager
"#;

#[test]
fn uncalled_sibling_environment_writes_do_not_erase_selected_standard_callable_views() {
    let fixture = Fixture::new();
    clean(fixture.check(EVENT_MANAGER_VIEW, true, None));
    let source = EVENT_MANAGER_VIEW.replace(
        "    assert(true,'checked')",
        "    ---@param value number\n    ---@return number\n    local function checked(value) return value end\n    assert(checked('wrong'),'checked')",
    );
    let rejected = report(fixture.check(&source, true, None), 1);
    error_on(&rejected, &source, "TS2345", "assert(checked('wrong'),'checked')", "'wrong'");
    assert!(!rejected.contains("\"code\":\"TS2349\""), "{rejected}");
    clean(fixture.check(&source.replace("checked('wrong')", "checked(42)"), true, None));
}

#[test]
fn reached_fixed_slot_replacements_are_not_reset_to_standard_contracts() {
    let fixture = Fixture::new();
    for replacement in [
        "assert=42\n",
        "(--[[---@type table]] _G)['assert']=42\n",
    ] {
        let source = format!("{replacement}hostEffect()\nassert(true,'checked')\n");
        let rejected = report(fixture.check(&source, true, None), 1);
        error_on(&rejected, &source, "TS2349", "assert(true,'checked')", "assert");
    }
    let ordinary = "---@generic T\n---@param value T\n---@vararg unknown\n---@return T\nlocal function ordinary(value,...) return value end\n";
    let consumer = "---@param value number|nil\nlocal function consume(value)\nassert(value)\n---@type number\nlocal narrowed=value\nend\n";
    for write in ["assert=ordinary", "(--[[---@type table]] _G)['assert']=ordinary"] {
        let source = format!("{ordinary}local function replace() {write} end\nreplace()\nhostEffect()\n{consumer}");
        let rejected = report(fixture.check(&source, true, None), 1);
        error_on(&rejected, &source, "TS2322", "local narrowed=value", "value");
        clean(fixture.check(&source.replace(write, ""), true, None));
    }
    let source = format!("local function replace() assert=42 end\nreplace()\nhostEffect()\n{consumer}");
    let rejected = report(fixture.check(&source, true, None), 1);
    error_on(&rejected, &source, "TS2322", "assert=42", "42");
    error_on(&rejected, &source, "TS2322", "local narrowed=value", "value");
    // An invalid assignment can retain its written callable contract during
    // recovery. Require the write error and loss of assertion effects, rather
    // than an arbitrary second uncallable-value diagnostic.
    clean(fixture.check(&source.replace("assert=42", ""), true, None));
}

#[test]
fn unresolved_dynamic_environment_keys_retain_the_default_host_policy_boundary() {
    let fixture = Fixture::new();
    let source = "---@param event string\nlocal function replace(event)\n(--[[---@type table]] _G)[event]=42\nassert(true,'checked')\nend\n";
    // An unproven dynamic key is an unknown effect under the ordinary policy;
    // the strict physical proof must still reject its possible replacement.
    clean(fixture.check(source, true, None));
    let rejected = report(fixture.check_options(source, true, None, true), 1);
    error_on(&rejected, source, "TS2349", "assert(true,'checked')", "assert");
    // The bounded scope scan does not transport an actual argument into a
    // writer's parameter. A caller's literal is not a selected-slot proof.
    let called = "---@param event string\nlocal function replace(event)\n(--[[---@type table]] _G)[event]=42\nend\nreplace('assert')\nhostEffect()\nassert(true,'checked')\n";
    clean(fixture.check(called, true, None));
}

#[test]
fn builtin_to_builtin_replacement_uses_the_replacements_original_public_heads() {
    let fixture = Fixture::new();
    // The assignment still violates assert's generic contract. Its reached
    // value nevertheless has type's one-argument public call signature.
    let source = "assert=type\n---@type string\nlocal result=assert(42)\n";
    let rejected = report(fixture.check(source, true, None), 1);
    error_on(&rejected, source, "TS2322", "assert=type", "type");
    assert!(!rejected.contains("\"code\":\"TS2349\""), "{rejected}");
    let wrong = source.replace("assert(42)", "assert(42,'extra')");
    let rejected = report(fixture.check(&wrong, true, None), 1);
    error_on(&rejected, &wrong, "TS2554", "local result=assert(42,'extra')", "assert(42,'extra')");
    let corrected = source.replace("assert=type", "assert=assert");
    clean(fixture.check(&corrected.replace("---@type string", "---@type number"), true, None));
}

#[test]
fn an_ordinary_standard_view_does_not_relax_strict_physical_capability_checks() {
    let fixture = Fixture::new();
    let source = EVENT_MANAGER_VIEW.replace("    assert(true,'checked')", "    hostEffect()\n    assert(true,'checked')");
    clean(fixture.check(&source, true, None));
    let strict = report(fixture.check_options(&source, true, None, true), 1);
    error_on(&strict, &source, "TS2349", "assert(true,'checked')", "assert");
}

#[test]
fn compatible_direct_and_reached_alias_replacements_do_not_acquire_assertion_effects() {
    let fixture = Fixture::new();
    let ordinary = "---@generic T\n---@param value T\n---@vararg unknown\n---@return T\nlocal function ordinary(value,...) return value end\n";
    let probe = "---@param value number|nil\nlocal function probe(value)\nsaved(value)\n---@type number\nlocal checked=value\nend\n";
    for replacement in [
        "saved=ordinary\n",
        "local function replace() saved=ordinary end\nreplace()\n",
    ] {
        let source = format!("{ordinary}local saved=assert\n{replacement}hostEffect()\n{probe}");
        let rejected = report(fixture.check(&source, true, None), 1);
        error_on(&rejected, &source, "TS2322", "local checked=value", "value");
        assert!(!rejected.contains("\"code\":\"TS2349\""), "{rejected}");
        clean(fixture.check(&source.replace(replacement, ""), true, None));
    }
}

#[test]
fn replaced_local_callees_remain_ordinary_after_an_unknown_call() {
    let fixture = Fixture::new();
    let source = format!("{ORDINARY}local loader=require\nlocal kind=type\nloader=ordinaryLoader\nkind=ordinaryKind\nhostEffect()\nlocal result=loader('absent')\n---@type number\nlocal checked_result=result\n---@param value number|string\nlocal function probe(value)\nif kind(value)=='number' then\n---@type number\nlocal checked_type=42\nend\nend\nprobe('text')\n");
    clean(fixture.check(&source, true, None));
    let wrong = source.replace("local checked_type=42", "local checked_type=value");
    let rejected = report(fixture.check(&wrong, true, None), 1);
    error_on(
        &rejected,
        &wrong,
        "TS2322",
        "local checked_type=value",
        "value",
    );
}

#[test]
fn saved_original_values_keep_their_effects_across_replacement_and_unknown_calls() {
    let fixture = Fixture::new();
    let prefix = format!("{ORDINARY}local savedLoader=require\nlocal savedKind=type\n_G.require=ordinaryLoader\n_G.type=ordinaryKind\nhostEffect()\n");
    trusted_controls(
        &fixture,
        &format!(
            "{prefix}{}",
            probe("hostEffect()", "savedLoader", "savedKind")
        ),
    );
}

#[test]
fn literal_field_and_function_return_aliases_keep_original_standard_operations() {
    let fixture = Fixture::new();
    for prefix in [
        "local operations={loader=require,kind=type}\nhostEffect()\nlocal savedLoader=operations.loader\nlocal savedKind=operations.kind\n",
        "local function getLoader() return require end\nlocal function getKind() return type end\nhostEffect()\nlocal savedLoader=getLoader()\nlocal savedKind=getKind()\n",
    ] {
        trusted_controls(
            &fixture,
            &format!(
                "{prefix}{}",
                probe("hostEffect()", "savedLoader", "savedKind")
            ),
        );
    }
}

#[test]
fn replaced_literal_fields_keep_ordinary_results_after_unknown_calls() {
    let fixture = Fixture::new();
    let source = format!("{ORDINARY}local operations={{loader=require,kind=type}}\noperations.loader=ordinaryLoader\noperations.kind=ordinaryKind\nhostEffect()\nlocal loader=operations.loader\nlocal kind=operations.kind\nlocal result=loader('absent')\n---@type number\nlocal checked_result=result\n---@param value number|string\nlocal function probe(value)\nif kind(value)=='number' then\n---@type number\nlocal checked_type=42\nend\nend\nprobe('text')\n");
    clean(fixture.check(&source, true, None));
    let wrong = source.replace("local checked_type=42", "local checked_type=value");
    let rejected = report(fixture.check(&wrong, true, None), 1);
    error_on(
        &rejected,
        &wrong,
        "TS2322",
        "local checked_type=value",
        "value",
    );
}

#[test]
fn the_strict_option_keeps_unknown_calls_as_standard_effect_barriers() {
    let fixture = Fixture::new();
    let source = probe("hostEffect()", "require", "type");
    clean(fixture.check(&source, true, None));
    let rejected = report(fixture.check_options(&source, true, None, true), 1);
    error_on(
        &rejected,
        &source,
        "LUA2036",
        "local module=require('module')",
        "require",
    );
    error_on(
        &rejected,
        &source,
        "TS2322",
        "local checked_type=value",
        "value",
    );
}

#[test]
fn no_standard_environment_does_not_seed_missing_builtin_names_after_unknown_calls() {
    let fixture = Fixture::new();
    let source = "hostEffect()\nlocal kind=type(42)\nlocal module=require('module')\n";
    let rejected = report(fixture.check(source, false, None), 1);
    error_on(&rejected, source, "TS2304", "local kind=type(42)", "type");
    error_on(
        &rejected,
        source,
        // The module host still recognizes a require request without library
        // declarations; the unknown effect must block its loader contract.
        "LUA2036",
        "local module=require('module')",
        "require",
    );
}

#[test]
fn custom_same_name_declarations_keep_ordinary_results_and_no_type_predicate() {
    let fixture = Fixture::new();
    let source = "hostEffect()\nlocal module=require('absent')\n---@type number\nlocal checked_module=module\n---@param value number|string\nlocal function probe(value)\nif type(value)=='number' then\n---@type number\nlocal checked_type=42\nend\nend\nprobe('text')\n";
    clean(fixture.check(source, false, Some(CUSTOM)));
    let wrong_module = source.replace(
        "---@type number\nlocal checked_module",
        "---@type string\nlocal checked_module",
    );
    let rejected = report(fixture.check(&wrong_module, false, Some(CUSTOM)), 1);
    error_on(
        &rejected,
        &wrong_module,
        "TS2322",
        "local checked_module=module",
        "module",
    );
    let wrong_type = source.replace("local checked_type=42", "local checked_type=value");
    let rejected = report(fixture.check(&wrong_type, false, Some(CUSTOM)), 1);
    error_on(
        &rejected,
        &wrong_type,
        "TS2322",
        "local checked_type=value",
        "value",
    );
}

#[test]
fn a_custom_type_declaration_cannot_regain_standard_narrowing_from_unknown_effects() {
    let fixture = Fixture::new();
    // An ordinary declaration can match the API without inheriting the
    // selected original declaration's runtime predicate capability.
    let custom = "---@param v any\n---@return 'nil'|'number'|'string'|'boolean'|'table'|'function'|'thread'|'userdata'\nfunction type(v) end\n";
    let source = probe("hostEffect()", "require", "type");
    let rejected = report(fixture.check(&source, true, Some(custom)), 1);
    error_on(
        &rejected,
        &source,
        "TS2322",
        "local checked_type=value",
        "value",
    );
    let corrected = source.replace("local checked_type=value", "local checked_type=42");
    clean(fixture.check(&corrected, true, Some(custom)));
}
