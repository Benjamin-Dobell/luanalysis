//! A literal field of a table without a contract has the wider type that a declared mutable
//! view of the table may write into it.
use luanalysis::project_check::{ProjectCheckOptions, check_project_with_options, static_require_requests};
use luanalysis::standard_environment::StandardEnvironmentProfile;
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// `code text` for each diagnostic, with inspections.
fn check(source: &str) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("luanalysis escaped fields {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main: PathBuf = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let project = Project::load_plan(
        &Resolver::new(ProjectConfig::new([root.clone()])).unwrap(),
        ProjectLoadPlan::new([main]),
        static_require_requests,
    );
    let report = check_project_with_options(project, ProjectCheckOptions {
        environment: StandardEnvironmentProfile::Lua54,
        inspections: true,
        ..ProjectCheckOptions::default()
    });
    fs::remove_dir_all(&root).unwrap();
    let main = report.file(report.project.entries[0]).unwrap();
    main.report.diagnostics.iter().map(|diagnostic| format!("{} {} | {}", diagnostic.code, &source[diagnostic.span.start..diagnostic.span.end], diagnostic.message)).collect()
}

/// Each diagnostic's code and text.
fn found(source: &str) -> Vec<String> {
    check(source).into_iter().map(|diagnostic| diagnostic.split(" | ").next().unwrap().to_owned()).collect()
}

const ANY: &str = "TS7044 s";

#[test]
fn a_literal_field_takes_the_wider_type_a_declared_view_may_assign() {
    for source in [
        // The README example.
        "local state = { ready = false }\n\n---@param s { ready: boolean }\nlocal function enable(s) s.ready = true end\n\nenable(state)\nif state.ready then print(\"ready\") end -- state.ready is boolean\n",
        // Before the call, too: the field has one type.
        "local state = { ready = false }\n---@param s { ready: boolean }\nlocal function enable(s) s.ready = true end\nif state.ready then print('maybe') end\nenable(state)\n",
        "local Config = {}\n---@param s { ready: boolean }\nfunction Config:apply(s) s.ready = true end\nlocal state = { ready = false }\nConfig:apply(state)\nif state.ready then print('taken') end\n",
        "local state = { ready = false }\n---@param s? { ready: boolean }\nlocal function enable(s) if s then s.ready = true end end\nenable(state)\nif state.ready then print('taken') end\n",
        "local state = { ready = false }\n---@type { ready: boolean }\nlocal alias = state\nalias.ready = true\nif state.ready then print('taken') end\n",
        "local state = { inner = { ready = false } }\n---@param s { inner: { ready: boolean } }\nlocal function enable(s) s.inner.ready = true end\nenable(state)\nif state.inner.ready then print('taken') end\n",
        "local state = { inner = { ready = false } }\n---@param s { ready: boolean }\nlocal function enable(s) s.ready = true end\nenable(state.inner)\nif state.inner.ready then print('taken') end\n",
        "local state = { mode = 'a' }\n---@param s { mode: 'a' | 'b' }\nlocal function flip(s) s.mode = 'b' end\nflip(state)\nif state.mode == 'b' then print('taken') end\n",
        "local counter = { count = 0 }\n---@param c { count: number }\nlocal function bump(c) c.count = c.count + 1 end\nbump(counter)\ncounter.count = counter.count + 1\n",
        // An optional field of the view does not make the table's field optional.
        "local counter = { count = 0 }\n---@param c { count: number | nil }\nlocal function reset(c) c.count = 1 end\nreset(counter)\ncounter.count = counter.count + 1\n",
        // A declared view that only reads may still be given a function that assigns.
        "local state = { ready = false }\n---@param s { ready: boolean }\nlocal function show(s) print(s.ready) end\nshow(state)\nstate.ready = true\n",
    ] {
        assert_eq!(found(source), [] as [&str; 0], "{source}");
    }
    let wider = "local state = { ready = false, mode = 'a' }\n---@param s { ready: boolean, mode: 'a' | 'b' }\nlocal function enable(s) s.ready = true end\nenable(state)\n---@type string\nlocal ready = state.ready\n---@type 'a'\nlocal mode = state.mode\nreturn ready, mode\n";
    assert_eq!(check(wider), [
        "TS2322 state.ready | Type 'boolean' is not assignable to type 'string'.",
        "TS2322 state.mode | Type '\"a\" | \"b\"' is not assignable to type '\"a\"'.",
    ]);
}

#[test]
fn an_untyped_parameter_widens_the_fields_its_function_assigns() {
    let enable = "local state = { ready = false }\nlocal function enable(s) s.ready = true end\nenable(state)\nif state.ready then print('taken') end\n";
    assert_eq!(found(enable), [ANY]);
    let own = "local state = { ready = false }\nfunction state.enable(s) s.ready = true end\nstate.enable(state)\nif state.ready then print('taken') end\n";
    assert_eq!(found(own), [ANY]);
    let reader = "local state = { ready = false }\nlocal function show(s) print(s.ready) end\nshow(state)\nif state.ready then print('never') end\n";
    assert_eq!(found(reader), [ANY, "LUA2873 state.ready"]);
}

#[test]
fn a_literal_field_stays_literal_without_a_wider_view_that_can_assign_it() {
    for source in [
        "local state = { ready = false }\n---@param s { readonly ready: boolean }\nlocal function show(s) print(s.ready) end\nshow(state)\nif state.ready then print('never') end\n",
        "local state = { ready = false }\n---@generic T\n---@param s T\n---@return T\nlocal function id(s) return s end\nid(state)\nif state.ready then print('never') end\n",
        "local state = { ready = false }\nprint(state)\nif state.ready then print('never') end\n",
        "local state = { ready = false }\n---@param s { other: boolean }\nlocal function show(s) print(s.other) end\nshow({ other = true })\nif state.ready then print('never') end\n",
    ] {
        assert_eq!(found(source), ["LUA2873 state.ready"], "{source}");
    }
    let direct = "local state = { ready = false }\nstate.ready = true\n";
    assert_eq!(check(direct), ["TS2322 true | Type 'true' is not assignable to type 'false'."]);
}
