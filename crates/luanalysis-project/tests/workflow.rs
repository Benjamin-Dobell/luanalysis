use luanalysis_project::{
    DependencyOutcome, FileRequests, Project, ProjectConfig, RequireRequest, ResolveError,
    Resolver,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis project {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn resolver(root: &Path) -> Resolver {
    Resolver::new(ProjectConfig::new([root.to_path_buf()])).unwrap()
}

#[test]
fn dotted_module_and_init_resolution_use_real_canonical_files_with_spaces() {
    let temp = Temp::new();
    let direct = temp.file("modules/part.lua", "return 1");
    let init = temp.file("package name/init.lua", "return 2");
    let resolver = resolver(&temp.0);
    assert_eq!(
        resolver.resolve("modules.part").unwrap().path,
        fs::canonicalize(direct).unwrap()
    );
    assert_eq!(
        resolver.resolve("package name").unwrap().path,
        fs::canonicalize(init).unwrap()
    );
}

#[test]
fn root_order_and_package_path_substitution_are_explicit() {
    let temp = Temp::new();
    let first = temp.file("first/a/unit.ttslua", "return 1");
    temp.file("second/a/unit.ttslua", "return 2");
    let mut config = ProjectConfig::new([temp.0.join("first"), temp.0.join("second")]);
    config.set_package_path("?.ttslua;?/init.ttslua");
    config.discovery_extensions = vec!["ttslua".into()];
    let resolver = Resolver::new(config).unwrap();
    assert_eq!(
        resolver.resolve("a.unit").unwrap().path,
        fs::canonicalize(first).unwrap()
    );
    assert_eq!(resolver.discover().files.len(), 2);
    let fixed = temp.file("first/fixed.lua", "return 3");
    let mut config = ProjectConfig::new([temp.0.join("first")]);
    config.set_package_path("fixed.lua;?/?.lua");
    assert_eq!(
        Resolver::new(config)
            .unwrap()
            .resolve("unrelated")
            .unwrap()
            .path,
        fs::canonicalize(fixed).unwrap()
    );
}

#[test]
fn every_question_mark_is_substituted_and_no_importer_directory_is_inferred() {
    let temp = Temp::new();
    let repeated = temp.file("src/a/b/a/b.lua", "return 1");
    temp.file("caller/neighbor.lua", "return 2");
    let mut config = ProjectConfig::new([temp.0.join("src")]);
    config.set_package_path("?/?.lua");
    let resolver = Resolver::new(config).unwrap();
    assert_eq!(
        resolver.resolve("a.b").unwrap().path,
        fs::canonicalize(repeated).unwrap()
    );
    assert!(matches!(
        resolver.resolve("neighbor"),
        Err(ResolveError::Missing { .. })
    ));
}

#[test]
fn discovery_is_sorted_deduplicated_and_honors_explicit_exclusions() {
    let temp = Temp::new();
    let a = temp.file("src/a.lua", "a");
    let z = temp.file("src/z.lua", "z");
    temp.file("src/excluded/no.lua", "no");
    temp.file("src/readme.txt", "no");
    let mut config = ProjectConfig::new([temp.0.join("src"), temp.0.join("src")]);
    config.excluded.push(temp.0.join("src/excluded"));
    let discovered = Resolver::new(config).unwrap().discover();
    assert!(discovered.errors.is_empty());
    assert_eq!(
        discovered.files,
        vec![fs::canonicalize(a).unwrap(), fs::canonicalize(z).unwrap()]
    );
}

#[test]
fn loader_records_cycles_and_missing_dependencies_without_reloading_files() {
    let temp = Temp::new();
    let entry = temp.file("a.lua", "a source");
    temp.file("b.lua", "b source");
    let mut extracted = Vec::new();
    let project = Project::load(&resolver(&temp.0), [entry.clone(), entry], |file| {
        extracted.push(file.path.clone());
        match file.path.file_stem().unwrap().to_str().unwrap() {
            "a" => vec![
                RequireRequest::new("b", 0..1),
                RequireRequest::new("absent", 2..3),
            ],
            "b" => vec![RequireRequest::new("a", 0..1)],
            _ => unreachable!(),
        }
    });
    assert_eq!(project.files.len(), 2);
    assert_eq!(project.entries.len(), 1);
    assert_eq!(extracted.len(), 2);
    assert_eq!(project.cycles.len(), 1);
    assert_eq!(
        project.cycles[0].files.first(),
        project.cycles[0].files.last()
    );
    let absent = project
        .dependencies
        .iter()
        .find(|edge| edge.request.module == "absent")
        .unwrap();
    let DependencyOutcome::Unresolved(ResolveError::Missing { attempted, .. }) = &absent.outcome
    else {
        panic!("missing")
    };
    assert_eq!(attempted.len(), 2);
    assert_eq!(
        &project.file(absent.from).unwrap().source[absent.request.span.clone()],
        "s"
    );
}

#[test]
fn source_snapshots_remain_owned_and_ids_reject_other_projects() {
    let temp = Temp::new();
    let entry = temp.file("a.lua", "original");
    let first = Project::load(&resolver(&temp.0), [entry.clone()], |_| vec![]);
    fs::write(&entry, "edited").unwrap();
    let second = Project::load(&resolver(&temp.0), [entry.clone()], |_| vec![]);
    assert_eq!(first.file(first.entries[0]).unwrap().source, "original");
    assert_eq!(second.file(second.entries[0]).unwrap().source, "edited");
    assert!(second.file(first.entries[0]).is_none());
    fs::remove_file(&entry).unwrap();
    assert_eq!(first.file_by_path(&entry).unwrap().source, "original");
}

#[test]
fn malformed_spans_and_non_utf8_sources_report_owned_errors() {
    let temp = Temp::new();
    let entry = temp.file("a.lua", "é");
    fs::write(temp.0.join("bad.lua"), [0xff]).unwrap();
    let project = Project::load(&resolver(&temp.0), [entry], |_| {
        vec![
            RequireRequest::new("bad", 0..2),
            RequireRequest::new("bad", 0..1),
        ]
    });
    assert!(
        matches!(&project.dependencies[0].outcome,DependencyOutcome::Unreadable(error) if error.kind==std::io::ErrorKind::InvalidData)
    );
    assert_eq!(
        project.dependencies[1].outcome,
        DependencyOutcome::InvalidSpan
    );
    assert!(matches!(
        resolver(&temp.0).resolve(""),
        Err(ResolveError::InvalidModule(_))
    ));
}

#[test]
fn missing_entry_and_directory_candidates_are_not_successful_modules() {
    let temp = Temp::new();
    fs::create_dir(temp.0.join("dir.lua")).unwrap();
    assert!(matches!(
        resolver(&temp.0).resolve("dir"),
        Err(ResolveError::Missing { .. })
    ));
    let project = Project::load(&resolver(&temp.0), [temp.0.join("absent.lua")], |_| vec![]);
    assert!(project.files.is_empty());
    assert_eq!(project.entry_errors.len(), 1);
}

#[cfg(unix)]
#[test]
fn symlinked_roots_and_directory_cycles_deduplicate_canonical_sources() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let real = temp.file("real/a.lua", "a");
    symlink(temp.0.join("real"), temp.0.join("alias")).unwrap();
    symlink(temp.0.join("real"), temp.0.join("real/cycle")).unwrap();
    let resolver = Resolver::new(ProjectConfig::new([
        temp.0.join("real"),
        temp.0.join("alias"),
    ]))
    .unwrap();
    assert_eq!(resolver.discover().files, [fs::canonicalize(real).unwrap()]);
    let project = Project::load(
        &resolver,
        [temp.0.join("real/a.lua"), temp.0.join("alias/a.lua")],
        |_| vec![],
    );
    assert_eq!(project.files.len(), 1);
    assert_eq!(project.entries.len(), 1);
}

#[test]
fn a_module_an_entry_provides_by_name_resolves_before_its_path() {
    let temp = Temp::new();
    // `main` is read first, yet finds the module that `meta/native.lua` provides.
    let main = temp.file("main.lua", "require 'native'");
    let provider = temp.file("meta/native.lua", "---@meta native");
    temp.file("native.lua", "return 1");
    let project = Project::load(&resolver(&temp.0), [main.clone(), provider.clone()], |source| {
        if source.path.ends_with("main.lua") {
            FileRequests { requires: vec![RequireRequest::new("native", 8..16)], provides: None }
        } else {
            FileRequests { requires: Vec::new(), provides: Some("native".into()) }
        }
    });
    let provider = project.file_by_path(&provider).unwrap().id;
    assert_eq!(project.provider("native"), Some(provider));
    assert_eq!(project.dependencies[0].outcome, DependencyOutcome::Resolved(provider));
    assert_eq!(project.files.len(), 2, "the path candidate is never loaded");
}
