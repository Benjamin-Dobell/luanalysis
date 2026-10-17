use std::{fs, path::PathBuf, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("luanalysis environment flow {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.file("api.def.lua", "---@type number\nanswer=nil\n");
        fixture.file("module.lua", "return {value=42}\n");
        fixture
    }
    fn file(&self, name: &str, source: &str) { fs::write(self.0.join(name), source).unwrap(); }
    fn check(&self, source: &str) -> Output {
        self.file("main.lua", source);
        Command::new(env!("CARGO_BIN_EXE_luanalysis")).current_dir(&self.0)
            .args(["check", "--lua-version", "5.4", "--library", "api.def.lua", "--dump-types", "main.lua"]).output().unwrap()
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn text(output: &Output) -> String { format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)) }
fn clean(output: &Output) { assert!(output.status.success(), "{}", text(output)); }

#[test]
fn original_global_environment_and_its_aliases_share_guarded_property_contracts() {
    let fixture = Fixture::new();
    clean(&fixture.check("local g=_G\nlocal e=_ENV\n---@type number\nlocal a=g.answer\n---@type number\nlocal b=e['answer']\n---@type number\nlocal c=answer\n"));
    let invalid = fixture.check("_G.answer='wrong'\n");
    assert!(text(&invalid).contains("TS2322"), "{}", text(&invalid));
}

#[test]
fn a_callback_global_write_does_not_execute_during_declaration_or_poison_other_importers() {
    let fixture = Fixture::new();
    clean(&fixture.check("local before=require('module')\n---@param event string\nlocal function listen(event)\n---@vararg unknown\nlocal handler=function(...) return ... end\n;(--[[---@type table]] _G)[event]=handler\nend\nlocal after=require('module')\n---@type number\nlocal a=before.value\n---@type number\nlocal b=after.value\n"));
}

#[test]
fn a_direct_loader_write_invalidates_only_its_original_continuation() {
    let fixture = Fixture::new();
    let output = fixture.check("local before=require('module')\n_G.require=function(name) return {} end\nlocal after=require('module')\n");
    assert!(!output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("main.lua:3:") && text(&output).contains("LUA2036"), "{}", text(&output));
    assert!(!text(&output).contains("main.lua:1:7: error[LUA2036]"), "{}", text(&output));
}

#[test]
fn a_frozen_environment_alias_write_reaches_the_original_global() {
    let fixture = Fixture::new();
    let output = fixture.check("local g=_G\ng.require=function(name) return {} end\nlocal after=require('module')\n");
    assert!(text(&output).contains("LUA2036"), "{}", text(&output));
}

#[test]
fn a_nominal_assertion_cannot_hide_environment_identity_while_fresh_allocations_stay_separate() {
    let fixture = Fixture::new();
    let header = "---@class LoaderView\n---@field require fun(name: string): table\n\n";
    let invalid = fixture.check(&format!("{header}local alias=--[[---@type LoaderView]] _G\nalias.require=function(name) return {{}} end\nlocal after=require('module')\n"));
    assert!(text(&invalid).contains("LUA2036"), "{}", text(&invalid));
    clean(&fixture.check(&format!("{header}local fresh=--[[---@type LoaderView]] {{require=function(name) return {{}} end}}\nlocal alias=fresh\nalias.require=function(name) return {{}} end\nlocal after=require('module')\n---@type number\nlocal checked=after.value\n")));
}

// Loader and standard-name replacements are found from original stores into
// the environment or a local that may hold it. An environment kept inside
// another table is not traced (as TypeScript does not trace `globalThis`).
#[test]
fn later_alias_replacements_are_found_while_nested_environment_containers_are_not_traced() {
    let fixture = Fixture::new();
    let nested = fixture.check("local wrapper={env=_G}\nwrapper.env.require=function(name) return {} end\nlocal after=require('module')\n");
    assert!(!text(&nested).contains("LUA2036"), "{}", text(&nested));
    let replaced = fixture.check("local fresh={}\nlocal alias=fresh\nalias=_G\nalias.require=function(name) return {} end\nlocal after=require('module')\n");
    assert!(text(&replaced).contains("LUA2036"), "{}", text(&replaced));
}

#[test]
fn local_environment_nil_keeps_cached_locals_and_rejects_actual_global_reads() {
    let fixture = Fixture::new();
    clean(&fixture.check("local cached=answer\nlocal _ENV=nil\n---@type number\nlocal copied=cached\n"));
    let invalid = fixture.check("local _ENV=nil\nlocal value=answer\n");
    assert!(!invalid.status.success(), "{}", text(&invalid));
    assert!(!text(&invalid).contains("requires environment-identity integration"), "{}", text(&invalid));
}

#[test]
fn lexical_environment_table_redirects_global_lookup_without_rewriting_the_tree() {
    let fixture = Fixture::new();
    clean(&fixture.check("local _ENV={answer=42}\n---@type number\nlocal checked=answer\n"));
    let invalid = fixture.check("local _ENV={answer='wrong'}\n---@type number\nlocal checked=answer\n");
    assert!(text(&invalid).contains("TS2322"), "{}", text(&invalid));
}

#[test]
fn loader_alias_candidates_keep_their_original_module_request_spans() {
    let fixture = Fixture::new();
    clean(&fixture.check("local loader=require\nlocal module=loader('module')\n---@type number\nlocal checked=module.value\n"));
}

#[test]
fn loader_alias_assigned_after_declaration_keeps_export_types() {
    let fixture = Fixture::new();
    let invalid = fixture.check("local loader\nloader=require\nlocal value=loader('module')\n---@type string\nlocal wrong=value.value\n");
    assert!(text(&invalid).contains("TS2322"), "{}", text(&invalid));
}

#[test]
fn replaced_alias_is_an_ordinary_call_and_does_not_publish_candidate_module_failures() {
    let fixture = Fixture::new();
    clean(&fixture.check("local loader=require\nloader=function(name) return 42 end\nlocal result=loader('absent')\n---@type number\nlocal checked=result\n"));
}

#[test]
fn rejected_loader_candidate_keeps_its_own_result_while_its_target_is_a_program_file() {
    let fixture = Fixture::new();
    fixture.file("poison.lua", "---@type number\nlocal invalid='wrong'\nreturn 42\n");
    // As a Bun program includes every file its syntactic requires name, the
    // target is checked; the replaced loader's call still returns its own value.
    let replacement = "local loader=require\nloader=function(name) return 42 end\nlocal result=loader('poison')\n---@type number\nlocal checked=result\n";
    let rejected = fixture.check(replacement);
    assert!(text(&rejected).contains("poison.lua:2:15: error[TS2322]"), "{}", text(&rejected));
    assert!(!text(&rejected).lines().any(|line| line.starts_with("main.lua") && line.contains("error[")), "{}", text(&rejected));

    let proven = "local loader=require\nlocal result=loader('poison')\n---@type number\nlocal checked=result\n";
    let invalid = fixture.check(proven);
    assert!(text(&invalid).contains("poison.lua:2:15: error[TS2322]"), "{}", text(&invalid));
    assert_eq!(invalid.status.code(), Some(1));

    fixture.file("poison.lua", "---@type number\nlocal corrected=42\nreturn corrected\n");
    clean(&fixture.check(proven));
}

#[test]
fn nested_package_loader_mutation_through_an_environment_alias_is_guarded() {
    let fixture = Fixture::new();
    let invalid = fixture.check("local env=_G\nenv.package.loaded.module='cached'\nlocal value=require('module')\n");
    assert!(text(&invalid).contains("LUA2036"), "{}", text(&invalid));
}

#[test]
fn a_captured_type_operation_does_not_reset_an_outer_replacement_to_the_standard_value() {
    let fixture = Fixture::new();
    let invalid = fixture.check("_G.type=function(_) return 'number' end\n---@param value number|string\nlocal function use(value)\nif type(value)=='number' then\n---@type number\nlocal wrong=value\nend\nend\nuse('text')\n");
    assert!(text(&invalid).contains("TS2322"), "{}", text(&invalid));
}

#[test]
fn implicit_environment_replacement_is_inherited_by_original_closures() {
    let fixture = Fixture::new();
    clean(&fixture.check("_ENV={answer='text'}\nlocal function use()\n---@type string\nlocal checked=answer\nend\nuse()\n"));
}

#[test]
fn a_string_keyed_view_of_an_environment_alias_retains_its_real_runtime_identity() {
    let fixture = Fixture::new();
    let invalid = fixture.check("local handlers=--[[---@type {[string]: any}]] _G\nhandlers.package.loaded.module='cached'\nlocal value=require('module')\n");
    assert!(text(&invalid).contains("LUA2036"), "{}", text(&invalid));
}

#[test]
fn a_global_type_replacement_through_an_alias_is_not_forgotten_at_a_closure_boundary() {
    let fixture = Fixture::new();
    let invalid = fixture.check("local env=_G\nenv.type=function(_) return 'number' end\n---@param value number|string\nlocal function use(value)\nif type(value)=='number' then\n---@type number\nlocal wrong=value\nend\nend\nuse('text')\n");
    assert!(text(&invalid).contains("TS2322"), "{}", text(&invalid));
}

#[test]
fn event_manager_inline_assertions_preserve_actual_environment_identity() {
    let fixture = Fixture::new();
    clean(&fixture.check("local first=require('module')\nlocal globalHandlers=--[[---@type {[string]: nil | function}]] _G\n---@param event string\nlocal function listen(event)\nlocal previous=globalHandlers[event]\n---@vararg unknown\n;(--[[---@type table]] _G)[event]=function(...) return ... end\nreturn previous\nend\nlocal last=require('module')\n---@type number\nlocal checked=last.value\n"));
}

#[test]
fn cached_type_operation_retains_native_narrowing_after_local_environment_nil() {
    let fixture = Fixture::new();
    clean(&fixture.check("local kind=type\nlocal _ENV=nil\n---@param value number|string\nlocal function use(value)\nif kind(value)=='number' then\n---@type number\nlocal numeric=value\nend\nend\nuse(42)\n"));
}

#[test]
fn an_ordinary_single_argument_string_function_acquires_no_runtime_type_predicate() {
    let fixture = Fixture::new();
    let invalid = fixture.check("---@param value any\n---@return string\nlocal function kind(value) return 'number' end\n---@param value number|string\nlocal function use(value)\nif kind(value)=='number' then\n---@type number\nlocal wrong=value\nend\nend\n");
    assert!(text(&invalid).contains("TS2322"), "{}", text(&invalid));
}

#[test]
fn erased_mutable_environment_views_do_not_silently_reuse_standard_loader_exports() {
    let fixture = Fixture::new();
    let invalid = fixture.check("local env=--[[---@type table]] _G\nenv=--[[---@type table]] _G\nenv.package.loaded.module='cached'\nlocal value=require('module')\n");
    assert!(text(&invalid).contains("LUA2036"), "{}", text(&invalid));
}

#[test]
fn a_forged_table_cast_cannot_turn_an_unrelated_table_into_the_global_environment() {
    let fixture = Fixture::new();
    clean(&fixture.check("local unrelated=--[[---@type table]] {}\nunrelated.package={loaded={}}\nunrelated.package.loaded.module='cached'\nlocal value=require('module')\n---@type number\nlocal checked=value.value\n"));
}
