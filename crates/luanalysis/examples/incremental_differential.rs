//! Checks a program again after a series of edits, incrementally and from scratch, and compares
//! every file's diagnostics: the incremental check must find exactly what a whole check finds.
//!
//! cargo run --release --example incremental_differential -- [check options] <paths>...
//!
//! Each edit is made, checked, then undone and checked again, in up to `EDITS` (default 40)
//! files spread over the program, or those `FILES` (comma-separated names) selects:
//! - a local before the file's final `return` (no other file can see it);
//! - a type error there;
//! - a new field on the module the file returns (which its users see);
//! - a field on the first class the file declares (which every user of the class sees).
use luanalysis::cli::{Parsed, parse_arguments};
use luanalysis::incremental::ProjectWorkspace;
use luanalysis::project_check::ProjectCheckReport;
use luanalysis_project::Project;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Instant;

fn diagnostics(report: &ProjectCheckReport) -> BTreeSet<String> {
    report.files.iter().flat_map(|file| {
        let path = report.project.file(file.file).unwrap().path.display().to_string();
        file.report.diagnostics.iter().map(move |diagnostic| {
            format!("{path}:{}-{} {} {}", diagnostic.span.start, diagnostic.span.end, diagnostic.code, diagnostic.message)
        })
    }).collect()
}

/// The edits for one file's text: the probes, each a replacement of the whole text.
fn edits(text: &str, edit: usize) -> Vec<(&'static str, String)> {
    let mut found = Vec::new();
    let tail = text.trim_end();
    let before_return = tail.rfind('\n').map(|at| at + 1).filter(|&at| tail[at..].starts_with("return "));
    let insert = |at: usize, line: String| format!("{}{line}\n{}", &text[..at], &text[at..]);
    match before_return {
        Some(at) => {
            found.push(("private local", insert(at, format!("local __probe_{edit} = 1"))));
            found.push(("type error", insert(at, format!("---@type string\nlocal __probe_{edit} = 1"))));
            let returned = tail[at + "return ".len()..].trim();
            if returned.chars().all(|ch| ch.is_alphanumeric() || ch == '_') && !returned.is_empty() {
                found.push(("module field", insert(at, format!("{returned}.__probe_{edit} = 1"))));
            }
        }
        None => {
            found.push(("private local", format!("{text}\nlocal __probe_{edit} = 1\n")));
            found.push(("type error", format!("{text}\n---@type string\nlocal __probe_{edit} = 1\n")));
        }
    }
    if let Some(at) = text.find("---@class ") {
        let line_end = text[at..].find('\n').map_or(text.len(), |end| at + end + 1);
        found.push(("class field", format!("{}---@field __probe_{edit} number\n{}", &text[..line_end], &text[line_end..])));
    }
    found
}

fn main() {
    let mut arguments = match parse_arguments(std::env::args_os().skip(1).collect()) {
        Ok(Parsed::Run(arguments)) => arguments,
        Ok(Parsed::Help) => return,
        Err(error) => panic!("{error}"),
    };
    arguments.inputs.configure(&mut arguments.options);
    let options = arguments.options.clone();
    let discovery = arguments.inputs.discover().unwrap();
    let resolver = arguments.inputs.resolver(&discovery).unwrap();
    let load = |workspace: &mut ProjectWorkspace, overlays: &HashMap<PathBuf, String>| {
        Project::load_plan_with_overlays(&resolver, discovery.plan.clone(), overlays, |source| workspace.static_require_requests(source))
    };
    let mut incremental = ProjectWorkspace::new();
    let started = Instant::now();
    let project = load(&mut incremental, &HashMap::new());
    let first = incremental.check_project_incrementally(project, options.clone());
    println!("first check: {:.2}s, {} diagnostics", started.elapsed().as_secs_f64(), diagnostics(&first).len());
    let mut paths: Vec<PathBuf> = first.project.files.iter()
        .filter(|file| file.role == luanalysis_project::SourceRole::Implementation && !first.standard_library_files.contains(&file.id))
        .map(|file| file.path.clone())
        .collect();
    paths.sort();
    // FILES=a,b limits the edits to paths containing one of those names.
    if let Ok(only) = std::env::var("FILES") {
        paths.retain(|path| only.split(',').any(|name| path.to_string_lossy().contains(name)));
    }
    let limit: usize = std::env::var("EDITS").ok().and_then(|value| value.parse().ok()).unwrap_or(40);
    let step = (paths.len() / limit.max(1)).max(1);
    let (mut checks, mut mismatches, mut incremental_seconds, mut whole_seconds) = (0, 0, 0.0, 0.0);
    for (edit, path) in paths.iter().step_by(step).take(limit).enumerate() {
        let text = std::fs::read_to_string(path).unwrap();
        for (kind, changed) in edits(&text, edit) {
            for (state, overlay) in [("edited", Some(changed)), ("undone", None)] {
                let overlays: HashMap<PathBuf, String> = overlay.into_iter().map(|text| (path.clone(), text)).collect();
                let started = Instant::now();
                let project = load(&mut incremental, &overlays);
                let report = incremental.check_project_incrementally(project, options.clone());
                let incremental_elapsed = started.elapsed().as_secs_f64();
                let mut whole = ProjectWorkspace::new();
                let started = Instant::now();
                let project = load(&mut whole, &overlays);
                let expected = whole.check_project(project, options.clone());
                let whole_elapsed = started.elapsed().as_secs_f64();
                checks += 1;
                incremental_seconds += incremental_elapsed;
                whole_seconds += whole_elapsed;
                let (found, wanted) = (diagnostics(&report), diagnostics(&expected));
                if found != wanted {
                    mismatches += 1;
                    println!("MISMATCH {} {kind} {state}:", path.display());
                    for missing in wanted.difference(&found).take(5) { println!("  missing {missing}"); }
                    for extra in found.difference(&wanted).take(5) { println!("  extra   {extra}"); }
                } else {
                    println!("ok {:.3}s (whole {:.3}s) {} {kind} {state}", incremental_elapsed, whole_elapsed, path.display());
                }
            }
        }
    }
    println!("{checks} checks, {mismatches} mismatches; incremental {:.2}s, whole {:.2}s in all", incremental_seconds, whole_seconds);
    if mismatches > 0 { std::process::exit(1); }
}
