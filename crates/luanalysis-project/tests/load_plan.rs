use luanalysis_project::{
    DependencyOutcome, Project, ProjectConfig, ProjectInput, ProjectLoadPlan, RequireRequest,
    Resolver, SourceRole,
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
            "luanalysis load plan {} {}",
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
fn declarations_are_eager_original_sources_without_runtime_entry_or_require_demands() {
    let temp = Temp::new();
    let entry = temp.file("src/main.ttslua", "local module = require('module')");
    let module = temp.file("lib/module.ttslua", "return { wait = Wait.time }");
    let wait = temp.file(
        "types/Wait.def.lua",
        "---@class tts__Wait\nWait = {}\n---@return number\nfunction Wait.time() end\nrequire('absent')\n",
    );
    let mut config = ProjectConfig::new([temp.0.join("src"), temp.0.join("lib")]);
    config.set_package_path("?.ttslua;?.lua");
    let mut plan = ProjectLoadPlan::new([entry.clone()]);
    plan.libraries
        .push(ProjectInput::new(wait.clone(), SourceRole::Declaration));
    let mut demanded = Vec::new();
    let project = Project::load_plan(&Resolver::new(config).unwrap(), plan, |file| {
        assert_eq!(file.role, SourceRole::Implementation);
        demanded.push(file.path.clone());
        if file.path.file_name().unwrap() == "main.ttslua" {
            vec![RequireRequest::new("module", 23..31)]
        } else {
            vec![]
        }
    });
    assert!(project.entry_errors.is_empty());
    assert!(project.library_errors.is_empty());
    assert!(project.role_conflicts.is_empty());
    assert_eq!(project.files.len(), 3);
    assert_eq!(project.entries.len(), 1);
    assert_eq!(project.libraries.len(), 1);
    assert_eq!(project.dependencies.len(), 1);
    let request = &project.dependencies[0];
    assert_eq!(
        &project.file(request.from).unwrap().source[request.request.span.clone()],
        "'module'"
    );
    let declaration = project.file(project.libraries[0]).unwrap();
    assert_eq!(declaration.path, fs::canonicalize(wait).unwrap());
    assert_eq!(declaration.role, SourceRole::Declaration);
    assert!(declaration.source.contains("require('absent')"));
    assert!(!project.entries.contains(&declaration.id));
    assert_eq!(
        project.file(project.entries[0]).unwrap().path,
        fs::canonicalize(entry).unwrap()
    );
    let DependencyOutcome::Resolved(target) = project.dependencies[0].outcome else {
        panic!("implementation dependency was not loaded")
    };
    assert_eq!(
        project.file(target).unwrap().path,
        fs::canonicalize(module).unwrap()
    );
    assert_eq!(demanded.len(), 2);
}

#[test]
fn a_dependency_reuses_an_explicit_declaration_without_executing_its_require_recipes() {
    let temp = Temp::new();
    let entry = temp.file("main.lua", "require('contract')");
    let contract = temp.file("contract.lua", "require('missing')");
    let mut plan = ProjectLoadPlan::new([entry]);
    plan.libraries
        .push(ProjectInput::new(contract.clone(), SourceRole::Declaration));
    let mut demands = 0;
    let project = Project::load_plan(&resolver(&temp.0), plan, |file| {
        demands += 1;
        assert_eq!(file.path.file_name().unwrap(), "main.lua");
        vec![RequireRequest::new("contract", 9..17)]
    });
    assert_eq!(demands, 1);
    assert_eq!(project.files.len(), 2);
    assert!(project.role_conflicts.is_empty());
    assert_eq!(
        project.dependencies[0].outcome,
        DependencyOutcome::Resolved(project.libraries[0])
    );
    assert_eq!(
        project.file_by_path(&contract).unwrap().role,
        SourceRole::Declaration
    );
}

#[test]
fn implementation_libraries_and_duplicate_entries_traverse_each_source_once() {
    let temp = Temp::new();
    let library = temp.file("utility.lua", "require('child')");
    let child = temp.file("child.lua", "return 1");
    let mut plan = ProjectLoadPlan::new([library.clone(), library.clone()]);
    plan.libraries = vec![
        ProjectInput::new(library.clone(), SourceRole::Implementation),
        ProjectInput::new(library.clone(), SourceRole::Implementation),
    ];
    let mut demanded = Vec::new();
    let project = Project::load_plan(&resolver(&temp.0), plan, |file| {
        demanded.push(file.path.clone());
        if file.path.file_name().unwrap() == "utility.lua" {
            vec![RequireRequest::new("child", 9..14)]
        } else {
            vec![]
        }
    });
    assert_eq!(project.entries.len(), 1);
    assert_eq!(project.libraries, project.entries);
    assert_eq!(demanded.len(), 2);
    assert_eq!(project.files.len(), 2);
    assert_eq!(
        project.file_by_path(&child).unwrap().role,
        SourceRole::Implementation
    );
    assert!(project.role_conflicts.is_empty());
}

#[test]
fn conflicting_entry_role_reports_original_request_and_keeps_declaration_out_of_entries() {
    let temp = Temp::new();
    let declaration = temp.file("types.def.lua", "---@alias Count number");
    let entry = temp.file("main.lua", "return 1");
    let mut plan = ProjectLoadPlan::new([declaration.clone(), entry.clone()]);
    plan.libraries.push(ProjectInput::new(
        declaration.clone(),
        SourceRole::Declaration,
    ));
    let project = Project::load_plan(&resolver(&temp.0), plan, |file| {
        assert_eq!(file.path, fs::canonicalize(&entry).unwrap());
        vec![]
    });
    assert!(project.entry_errors.is_empty());
    assert_eq!(project.entries.len(), 1);
    assert_eq!(project.role_conflicts.len(), 1);
    let conflict = &project.role_conflicts[0];
    assert_eq!(conflict.path, fs::canonicalize(&declaration).unwrap());
    assert_eq!(conflict.requested_path, declaration);
    assert_eq!(conflict.file, project.libraries[0]);
    assert_eq!(conflict.existing_role, SourceRole::Declaration);
    assert_eq!(conflict.requested_role, SourceRole::Implementation);
    let text = conflict.to_string();
    assert!(text.contains("declaration"));
    assert!(text.contains("implementation"));
    assert!(text.contains("types.def.lua"));
    assert_eq!(
        project.file(conflict.file).unwrap().role,
        SourceRole::Declaration
    );
}

#[test]
fn library_read_failures_are_separate_from_entries_and_other_inputs_still_load() {
    let temp = Temp::new();
    let entry = temp.file("main.lua", "return 1");
    let missing = temp.0.join("missing.def.lua");
    let invalid = temp.0.join("invalid.def.lua");
    fs::write(&invalid, [0xff]).unwrap();
    let mut plan = ProjectLoadPlan::new([entry]);
    plan.libraries = vec![
        ProjectInput::new(missing.clone(), SourceRole::Declaration),
        ProjectInput::new(invalid.clone(), SourceRole::Declaration),
    ];
    let project = Project::load_plan(&resolver(&temp.0), plan, |_| vec![]);
    assert_eq!(project.entries.len(), 1);
    assert_eq!(project.files.len(), 1);
    assert!(project.entry_errors.is_empty());
    assert!(project.libraries.is_empty());
    assert_eq!(project.library_errors.len(), 2);
    assert_eq!(project.library_errors[0].path, missing);
    assert_eq!(project.library_errors[0].kind, std::io::ErrorKind::NotFound);
    assert_eq!(
        project.library_errors[1].path,
        fs::canonicalize(invalid).unwrap()
    );
    assert_eq!(
        project.library_errors[1].kind,
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn declaration_roles_are_explicit_and_belong_to_an_owned_immutable_snapshot() {
    let temp = Temp::new();
    let declaration = temp.file("ordinary.lua", "---@alias Count number");
    let mut plan = ProjectLoadPlan::default();
    plan.libraries.push(ProjectInput::new(
        declaration.clone(),
        SourceRole::Declaration,
    ));
    let first = Project::load_plan(&resolver(&temp.0), plan, |_| -> Vec<RequireRequest> {
        panic!("a declaration has no runtime require demand")
    });
    fs::write(&declaration, "return 'updated'").unwrap();
    let second = Project::load(&resolver(&temp.0), [declaration.clone()], |_| vec![]);
    assert_eq!(first.files[0].role, SourceRole::Declaration);
    assert_eq!(first.files[0].source, "---@alias Count number");
    assert_eq!(second.files[0].role, SourceRole::Implementation);
    assert_eq!(second.files[0].source, "return 'updated'");
    assert!(second.file(first.files[0].id).is_none());
    fs::remove_file(&declaration).unwrap();
    assert_eq!(
        first.file_by_path(&declaration).unwrap().role,
        SourceRole::Declaration
    );
}

#[test]
fn the_compatible_loader_does_not_infer_declaration_role_from_the_def_suffix() {
    let temp = Temp::new();
    let entry = temp.file("ordinary.def.lua", "return 1");
    let mut demands = 0;
    let project = Project::load(&resolver(&temp.0), [entry], |file| {
        demands += 1;
        assert_eq!(file.role, SourceRole::Implementation);
        vec![]
    });
    assert_eq!(demands, 1);
    assert_eq!(project.entries.len(), 1);
    assert!(project.libraries.is_empty());
    assert!(project.library_errors.is_empty());
    assert!(project.role_conflicts.is_empty());
}

#[cfg(unix)]
#[test]
fn canonical_library_aliases_deduplicate_and_conflicting_alias_roles_are_precise() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let real = temp.file("types/contract.def.lua", "---@alias Count number");
    let alias = temp.0.join("contract-alias.lua");
    symlink(&real, &alias).unwrap();
    let plan = ProjectLoadPlan {
        entries: vec![],
        libraries: vec![
            ProjectInput::new(real.clone(), SourceRole::Declaration),
            ProjectInput::new(alias.clone(), SourceRole::Declaration),
            ProjectInput::new(alias.clone(), SourceRole::Implementation),
        ],
    };
    let project = Project::load_plan(&resolver(&temp.0), plan, |_| -> Vec<RequireRequest> {
        panic!("only declarations were successfully loaded")
    });
    assert_eq!(project.files.len(), 1);
    assert_eq!(project.libraries.len(), 1);
    assert_eq!(project.role_conflicts.len(), 1);
    assert_eq!(
        project.role_conflicts[0].path,
        fs::canonicalize(real).unwrap()
    );
    assert_eq!(project.role_conflicts[0].requested_path, alias);
    assert_eq!(project.role_conflicts[0].file, project.libraries[0]);
    assert!(project.entries.is_empty());
}
