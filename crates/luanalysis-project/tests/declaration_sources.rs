use luanalysis_project::{
    DeclarationSource, DeclarationSourceError, Project, ProjectConfig, Resolver, SourceOrigin,
    SourceRole,
};
use std::{fs, path::PathBuf};

fn empty_project() -> Project {
    Project::load(&Resolver::new(ProjectConfig::new([])).unwrap(), [], |_| -> Vec<luanalysis_project::RequireRequest> {
        panic!("a declaration resource must not become a runtime source")
    })
}

fn resource(identity: &str, path: &str, source: &str) -> DeclarationSource {
    DeclarationSource {
        identity: identity.to_owned(),
        path: PathBuf::from(path),
        source: source.to_owned(),
    }
}

#[test]
fn declaration_resources_retain_original_text_without_filesystem_or_runtime_membership() {
    let mut project = empty_project();
    let source = "---@return number\nfunction count() end\nrequire('never-a-dependency')\n";
    let input = resource("lua54/base@revision", "stdlib/Lua54/base.def.lua", source);
    assert!(!input.path.exists());
    let file = project.add_declaration_source(input).unwrap();
    let original = project.file(file).unwrap();
    assert_eq!(&*original.source, source);
    assert_eq!(original.path, PathBuf::from("stdlib/Lua54/base.def.lua"));
    assert_eq!(original.role, SourceRole::Declaration);
    assert_eq!(
        original.origin,
        SourceOrigin::DeclarationLibrary {
            identity: "lua54/base@revision".into()
        }
    );
    assert_eq!(project.libraries, [file]);
    assert!(project.entries.is_empty());
    assert!(project.dependencies.is_empty());
    assert!(project.entry_errors.is_empty());
    assert!(project.library_errors.is_empty());
    assert!(project.file_by_path(&original.path).is_none());
    assert!(empty_project().file(file).is_none());
}

#[test]
fn declaration_identity_deduplicates_identical_bytes_and_rejects_conflicting_origins() {
    let mut project = empty_project();
    let input = resource("lua54/math@revision", "stdlib/math.def.lua", "original");
    let first = project.add_declaration_source(input.clone()).unwrap();
    assert_eq!(
        project.add_declaration_source(input.clone()).unwrap(),
        first
    );
    for conflicting in [
        DeclarationSource {
            source: "changed".into(),
            ..input.clone()
        },
        DeclarationSource {
            path: PathBuf::from("other.def.lua"),
            ..input.clone()
        },
    ] {
        let requested_path = conflicting.path.clone();
        assert_eq!(
            project.add_declaration_source(conflicting),
            Err(DeclarationSourceError::ConflictingIdentity {
                identity: input.identity.clone(),
                existing: first,
                existing_path: input.path.clone(),
                requested_path,
            })
        );
    }
    assert_eq!(project.files.len(), 1);
    assert_eq!(project.libraries, [first]);
    assert_eq!(project.file(first).unwrap().source.as_ref(), "original");
    assert_eq!(
        project.add_declaration_source(resource("", "empty", "")),
        Err(DeclarationSourceError::EmptyIdentity)
    );
    assert_eq!(project.files.len(), 1);
}

#[test]
fn embedded_display_paths_do_not_replace_canonical_filesystem_sources_or_roles() {
    let path = std::env::temp_dir().join(format!(
        "luanalysis profile identity {}.lua",
        std::process::id()
    ));
    fs::write(&path, "return 42").unwrap();
    let mut project = Project::load(
        &Resolver::new(ProjectConfig::new([])).unwrap(),
        [path.clone()],
        |_| vec![],
    );
    let physical = project.entries[0];
    let bundled = project
        .add_declaration_source(DeclarationSource {
            identity: "different-original-resource".into(),
            path: fs::canonicalize(&path).unwrap(),
            source: "---@return string\nfunction identity() end".into(),
        })
        .unwrap();
    assert_ne!(physical, bundled);
    assert_eq!(project.file_by_path(&path).unwrap().id, physical);
    assert_eq!(
        project.file(physical).unwrap().origin,
        SourceOrigin::Filesystem
    );
    assert_eq!(
        project.file(physical).unwrap().role,
        SourceRole::Implementation
    );
    assert_eq!(project.file(bundled).unwrap().role, SourceRole::Declaration);
    assert_eq!(project.entries, [physical]);
    assert_eq!(project.libraries, [bundled]);
    fs::remove_file(path).unwrap();
}
