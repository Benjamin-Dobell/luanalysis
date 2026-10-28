//! When no overload accepts a call, the error is the overload's that accepts the most arguments,
//! at the argument it rejects, and says that no overload matched.
use luanalysis::project_check::{ProjectCheckOptions, ProjectCheckReport, check_project_with_options};
use luanalysis_project::{Project, ProjectConfig, Resolver};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn check(source: &str) -> (String, ProjectCheckReport) {
    let root = std::env::temp_dir().join(format!("luanalysis overload failure reports {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let main: PathBuf = root.join("main.lua");
    fs::write(&main, source).unwrap();
    let resolver = Resolver::new(ProjectConfig::new([root.clone()])).unwrap();
    let project = Project::load(&resolver, [main], luanalysis::project_check::static_require_requests);
    let report = check_project_with_options(project, ProjectCheckOptions { jobs: 1, ..ProjectCheckOptions::default() });
    fs::remove_dir_all(&root).unwrap();
    (source.to_owned(), report)
}

/// Each diagnostic's code, the source it is reported at and its message.
fn diagnostics(source: &str, report: &ProjectCheckReport) -> Vec<(&'static str, String, String)> {
    report.files.iter().flat_map(|file| &file.report.diagnostics)
        .map(|diagnostic| (diagnostic.code, source[diagnostic.span.start..diagnostic.span.end].to_owned(), diagnostic.message.clone()))
        .collect()
}

const SPAWN: &str = "---@shape Bag
---@field bag true

---@shape Deck
---@field deck true

---@shape Infinite
---@field infinite true

---@shape Callback
---@field callback_function nil | fun(): void

---@overload fun(state: Bag, callback: nil | Callback): Bag
---@overload fun(state: Deck, callback: nil | Callback): Deck
---@overload fun(state: Infinite, callback: nil | Callback): Infinite
---@param state Bag | Deck | Infinite
---@param callback nil | Callback
---@return Bag | Deck | Infinite
local function spawn(state, callback) return state end

---@type Deck
local deck = { deck = true }
";

#[test]
fn the_overload_that_accepts_the_most_arguments_reports_where_the_call_fails() {
    let (source, report) = check(&format!("{SPAWN}local spawned = spawn(deck, function() end)\n"));
    let found = diagnostics(&source, &report);
    assert_eq!(found.len(), 1, "{found:#?}");
    let (code, at, message) = &found[0];
    assert_eq!((*code, at.as_str()), ("TS2769", "function() end"));
    assert!(message.starts_with("No overload matches this call.\n  Overload 2 of 3, 'fun(state: Deck, callback: nil | Callback): Deck', gave the following error.\n    Argument of type "), "{message}");
    assert!(message.ends_with("is not assignable to parameter of type 'nil | Callback'."), "{message}");
}

#[test]
fn overloads_that_fail_alike_report_the_last_and_one_reports_alone() {
    // Each overload rejects the first argument: the last, as TypeScript reports.
    let (source, report) = check(&format!("{SPAWN}local spawned = spawn(42, nil)\n"));
    let found = diagnostics(&source, &report);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!((found[0].0, found[0].1.as_str()), ("TS2769", "42"));
    assert!(found[0].2.contains("Overload 3 of 3, 'fun(state: Infinite, callback: nil | Callback): Infinite'"), "{}", found[0].2);
    // Only one overload takes two arguments, so its error is the call's.
    let (source, report) = check("---@overload fun(a: number): number\n---@overload fun(a: string, b: number): string\n---@param a number | string\n---@param b? number\n---@return number | string\nlocal function g(a, b) return a end\n\nlocal parsed = g('x', 'y')\n");
    let found = diagnostics(&source, &report);
    assert_eq!(found.iter().map(|(code, at, _)| (*code, at.as_str())).collect::<Vec<_>>(), [("TS2345", "'y'")]);
    // A function without overloads reports as before.
    let (source, report) = check("---@param value number\nlocal function f(value) end\nf('text')\n");
    let found = diagnostics(&source, &report);
    assert_eq!(found.iter().map(|(code, at, _)| (*code, at.as_str())).collect::<Vec<_>>(), [("TS2345", "'text'")]);
}
