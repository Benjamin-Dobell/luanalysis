//! Shapes that once took exponential time in their length: each would not finish if a link of
//! the chain were checked again per path to it.
use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn check(source: &str) -> luanalysis::project_check::ProjectCheckReport {
    let root = std::env::temp_dir().join(format!("luanalysis chains {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&root).unwrap();
    let main = root.join("main.lua");
    std::fs::write(&main, source).unwrap();
    let project = Project::load_plan(&Resolver::new(ProjectConfig::new([root.clone()])).unwrap(), ProjectLoadPlan::new([main]), static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions { environment: StandardEnvironmentProfile::Lua54, ..Default::default() });
    std::fs::remove_dir_all(&root).unwrap();
    report
}

/// `util.switch():case 'a':call(f):case 'b':call(f)...`, as LuaLS builds dispatch tables. A
/// method call's receiver is its `self` argument; it was checked again in every overload pass.
#[test]
fn a_long_method_chain_checks_each_link_once() {
    let mut source = String::from("---@class Builder\nlocal Builder = {}\n---@overload fun(name: string): Builder\n---@param name number\n---@return Builder\nfunction Builder:case(name) return self end\n---@return Builder\nlocal function switch() return Builder end\nlocal built = switch()");
    for index in 0..60 {
        source.push_str(&format!("\n    :case '{index}'"));
    }
    source.push('\n');
    let report = check(&source);
    assert_eq!(report.checked_file_count(), 1);
}

/// `Predef` in LPeg's re.lua: fields stored by a nested function, each read from another field
/// of the same table. A read once resolved the whole inferred schema, whose stored values read
/// the table again; reads now resolve their own key alone, as TypeScript properties do.
#[test]
fn fields_stored_from_one_another_resolve_one_key_at_a_time() {
    let mut source = String::from("local any = 1\nlocal Predef = { nl = 1 }\nlocal function update()\n");
    for name in ["a", "c", "d", "g", "l", "p", "s", "u", "w", "x"] {
        source.push_str(&format!("  Predef.{name} = Predef.{name}_source\n"));
    }
    for name in ["a", "c", "d", "g", "l", "p", "s", "u", "w", "x"] {
        source.push_str(&format!("  Predef.{} = any - Predef.{name}\n", name.to_uppercase()));
    }
    source.push_str("end\nupdate()\nlocal S = (Predef.s + 1) * (any - Predef.nl)\nreturn S\n");
    let report = check(&source);
    let main = report.file(report.project.entries[0]).unwrap();
    // Each `_source` field is missing; nothing is excessively deep.
    assert_eq!(main.report.diagnostics.iter().filter(|diagnostic| diagnostic.code == "TS2339").count(), 10, "{:#?}", main.report.diagnostics);
    assert!(main.report.diagnostics.iter().all(|diagnostic| diagnostic.code != "TS2589"), "{:#?}", main.report.diagnostics);
}

/// LuaLS's test/completion/common.lua: stores into a global table between global calls. Each
/// store once asked whether that global might be the environment by reading it at the store,
/// which revisited every store before it.
#[test]
fn stores_into_a_global_table_between_global_reads_are_not_revisited() {
    let mut source = String::from("Cared = {}\n---@param script string\n---@return fun(expect: table)\nfunction TEST(script) return function(expect) end end\n");
    for index in 0..40 {
        source.push_str(&format!("Cared['key{}'] = {}\nTEST [[case {index}]] {{ {{ label = 'x', kind = {index} }} }}\n", index % 3, index % 2 == 0));
    }
    let report = check(&source);
    assert_eq!(report.checked_file_count(), 1);
}
