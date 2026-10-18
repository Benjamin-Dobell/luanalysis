//! Reusable immutable source inputs for repeated project checks.
//!
//! This first stage reuses complete parses and lexical bindings. Every check
//! still creates fresh semantic state; source reuse never certifies an inferred
//! type, diagnostic, module result or project namespace as current.

use crate::bind::{self, BoundFile};
use luanalysis_project::{FileId, FileRequests, Project, RequireRequest, SourceFile, SourceOrigin, SourceRole};
use luanalysis_syntax::File;
use crate::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Work actually performed while preparing one registered project snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreparationStats {
    pub files_seen: usize,
    pub parsed: usize,
    pub parse_reused: usize,
    pub bound: usize,
    pub bind_reused: usize,
    pub evicted: usize,
    /// No native semantic query result is reused by source preparation.
    pub semantic_reused: usize,
}

/// A source revision contains no native checker handles or project file IDs.
/// Its revision ordinal is local to its workspace, not a semantic origin key.
#[derive(Debug)]
pub struct SourceSnapshot {
    revision: u64,
    source: Arc<str>,
    syntax: Arc<File>,
    bound: Option<Arc<BoundFile>>,
    /// The cost of checking this revision, which schedules a project check's tasks, computed
    /// once for it.
    checking_cost: std::sync::OnceLock<usize>,
}

impl SourceSnapshot {
    /// The cost of checking this revision, with `type_node_cost` per annotation type node.
    pub(crate) fn checking_cost(&self, type_node_cost: usize) -> usize {
        *self.checking_cost.get_or_init(|| match self.bound() {
            Some(bound) => crate::project_cost::checking_cost(self.syntax(), bound, type_node_cost),
            None => 1,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn syntax(&self) -> &File {
        &self.syntax
    }

    /// Lua syntax failures retain their parse diagnostics and have no binding.
    pub fn bound(&self) -> Option<&BoundFile> {
        self.bound.as_deref()
    }

    pub fn syntax_arc(&self) -> Arc<File> {
        Arc::clone(&self.syntax)
    }

    pub fn bound_arc(&self) -> Option<Arc<BoundFile>> {
        self.bound.as_ref().map(Arc::clone)
    }
}

/// A cached source revision attached to the current project's file identity.
#[derive(Clone, Debug)]
pub struct PreparedProjectSource {
    pub id: FileId,
    pub role: SourceRole,
    pub snapshot: Arc<SourceSnapshot>,
}

#[derive(Debug)]
pub struct PreparedSources {
    pub sources: Vec<PreparedProjectSource>,
    pub stats: PreparationStats,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SourceIdentity {
    path: PathBuf,
    declaration_identity: Option<String>,
    role: SourceRole,
}

impl SourceIdentity {
    fn of(source: &SourceFile) -> Self {
        Self {
            path: source.path.clone(),
            declaration_identity: match &source.origin {
                SourceOrigin::Filesystem => None,
                SourceOrigin::DeclarationLibrary { identity } => Some(identity.clone()),
            },
            role: source.role,
        }
    }
}

/// Retains only immutable parse/bind inputs and source recipes between project checks.
///
/// Call preparation after registering the selected standard declaration
/// sources. The host must refresh project membership and module resolution on
/// each check: an unchanged Lua buffer can acquire a new module, global name,
/// declaration conflict or standard-library contract without changing bytes.
#[derive(Default)]
pub struct ProjectWorkspace {
    sources: HashMap<SourceIdentity, Arc<SourceSnapshot>>,
    next_revision: u64,
    /// Parse/bind work performed by dependency discovery before preparation.
    pending_preparation: HashMap<u64, bool>,
    last_preparation: PreparationStats,
    /// Each retained revision's recipe identity, kept so its source recipes stay valid.
    identities: HashMap<u64, crate::native_functions::project_provider::SourceRecipeIdentity>,
    /// The source recipes of the last session, by revision, and the global names their flow
    /// recipes were seeded with. A session over the same names reuses them; they hold no type.
    recipes: HashMap<u64, crate::native_functions::SourceRecipes>,
    recipe_names: Vec<Vec<u8>>,
}

impl ProjectWorkspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn retained_source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn last_preparation_stats(&self) -> PreparationStats {
        self.last_preparation
    }

    /// The recipe identity of each revision, kept across sessions, and the source recipes the last
    /// session built for them.
    pub(crate) fn source_inputs(&mut self, revisions: &[u64]) -> (Vec<crate::native_functions::project_provider::SourceRecipeIdentity>, crate::native_functions::project_provider::PreparedSources) {
        let identities = revisions.iter()
            .map(|revision| *self.identities.entry(*revision).or_insert_with(crate::native_functions::project_provider::SourceRecipeIdentity::new))
            .collect();
        let prepared = crate::native_functions::project_provider::PreparedSources {
            recipes: revisions.iter().map(|revision| self.recipes.get(revision).cloned()).collect(),
            names: self.recipe_names.clone(),
        };
        (identities, prepared)
    }

    /// Keeps the source recipes a session built for `revisions` (in order) for the next session.
    pub(crate) fn retain_sources(&mut self, revisions: &[u64], prepared: crate::native_functions::project_provider::PreparedSources) {
        if prepared.names != self.recipe_names { self.recipes.clear(); }
        for (revision, recipes) in revisions.iter().zip(prepared.recipes) {
            if let Some(recipes) = recipes { self.recipes.insert(*revision, recipes); }
        }
        self.recipe_names = prepared.names;
    }

    /// Check a freshly loaded project while retaining unchanged source inputs.
    /// All native semantic state belongs to this check's immutable epoch.
    pub fn check_project(
        &mut self,
        project: Project,
        options: crate::project_check::ProjectCheckOptions,
    ) -> crate::project_check::ProjectCheckReport {
        crate::project_check::check_project_with_workspace(project, options, self)
    }

    /// Releases retained source inputs. Existing prepared snapshots remain
    /// valid through their own shared ownership.
    pub fn clear(&mut self) {
        self.sources.clear();
        self.pending_preparation.clear();
        self.last_preparation = PreparationStats::default();
    }

    /// Supplies the source-host's static candidates from the same cached
    /// syntax/bind inputs used by checking. Resolution still runs on every load;
    /// candidate discovery never grants the runtime callee require provenance.
    pub fn static_require_requests(&mut self, source: &SourceFile) -> FileRequests {
        if source.role == SourceRole::Declaration {
            return FileRequests::default();
        }
        let (snapshot, created) = self.snapshot_for(source);
        if created {
            self.pending_preparation
                .insert(snapshot.revision, snapshot.bound.is_some());
        }
        let Some(bound) = snapshot.bound() else {
            return FileRequests::default();
        };
        let requires = crate::module_requests::require_calls(snapshot.syntax(), bound)
            .into_iter()
            .filter_map(|call| {
                let crate::module_requests::RequireTarget::Static(module) = call.target else {
                    return None;
                };
                Some(RequireRequest::new(
                    module,
                    call.argument_span.start..call.argument_span.end,
                ))
            })
            .collect();
        let provides = snapshot.syntax().meta().flatten().map(str::to_owned);
        FileRequests { requires, provides }
    }

    fn snapshot_for(&mut self, source: &SourceFile) -> (Arc<SourceSnapshot>, bool) {
        let identity = SourceIdentity::of(source);
        if let Some(known) = self.sources.get(&identity)
            && known.source.as_ref() == source.source.as_str()
        {
            return (Arc::clone(known), false);
        }
        // As TypeScript binds the tree its parser recovers, a source with syntax errors is bound
        // too, so the language service answers in it while it is being typed.
        let syntax = luanalysis_syntax::parse(&source.source);
        let bound = Some(Arc::new(bind::bind(&syntax)));
        self.next_revision = self
            .next_revision
            .checked_add(1)
            .expect("workspace source revisions exhausted");
        let snapshot = Arc::new(SourceSnapshot {
            revision: self.next_revision,
            source: Arc::from(source.source.as_str()),
            syntax: Arc::new(syntax),
            bound,
            checking_cost: std::sync::OnceLock::new(),
        });
        self.sources.insert(identity, Arc::clone(&snapshot));
        (snapshot, true)
    }

    /// Exact bytes, origin, path and role determine parse/bind reuse. A fresh
    /// project ID or changed discovery order never reuses an old FileId.
    pub fn prepare_registered_sources(&mut self, project: &Project) -> PreparedSources {
        let pending = std::mem::take(&mut self.pending_preparation);
        let mut stats = PreparationStats {
            parsed: pending.len(),
            bound: pending.values().filter(|&&bound| bound).count(),
            ..PreparationStats::default()
        };
        let mut current = HashSet::default();
        let mut sources = Vec::with_capacity(project.files.len());
        for source in &project.files {
            stats.files_seen += 1;
            current.insert(SourceIdentity::of(source));
            let (snapshot, created) = self.snapshot_for(source);
            if created {
                stats.parsed += 1;
                stats.bound += usize::from(snapshot.bound.is_some());
            } else if !pending.contains_key(&snapshot.revision) {
                stats.parse_reused += 1;
                stats.bind_reused += usize::from(snapshot.bound.is_some());
            }
            sources.push(PreparedProjectSource {
                id: source.id,
                role: source.role,
                snapshot,
            });
        }
        let before = self.sources.len();
        self.sources
            .retain(|identity, _| current.contains(identity));
        let revisions: HashSet<u64> = sources.iter().map(|source: &PreparedProjectSource| source.snapshot.revision).collect();
        self.identities.retain(|revision, _| revisions.contains(revision));
        self.recipes.retain(|revision, _| revisions.contains(revision));
        stats.evicted = before - self.sources.len();
        self.last_preparation = stats;
        PreparedSources { sources, stats }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luanalysis_project::{
        DeclarationSource, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver,
    };
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "luanalysis incremental inputs {} {}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, source: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, source).unwrap();
            path
        }

        fn load(&self, paths: &[PathBuf], declarations: &[PathBuf]) -> Project {
            let resolver = Resolver::new(ProjectConfig::new([self.0.clone()])).unwrap();
            let mut plan = ProjectLoadPlan::new(paths.iter().cloned());
            plan.libraries = declarations
                .iter()
                .cloned()
                .map(|path| ProjectInput::new(path, SourceRole::Declaration))
                .collect();
            Project::load_plan(&resolver, plan, |_| Vec::new())
        }

        fn declaration(&self, identity: &str, source: &str) -> Project {
            let mut project = self.load(&[], &[]);
            project
                .add_declaration_source(DeclarationSource {
                    identity: identity.into(),
                    path: self.0.join("standard.def.lua"),
                    source: source.into(),
                })
                .unwrap();
            project
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn matches_cold(project: &Project, prepared: &PreparedSources) {
        assert_eq!(prepared.sources.len(), project.files.len());
        for (source, cached) in project.files.iter().zip(&prepared.sources) {
            let parsed = luanalysis_syntax::parse(&source.source);
            assert_eq!(cached.id, source.id);
            assert_eq!(cached.role, source.role);
            assert_eq!(cached.snapshot.source(), source.source.as_str());
            assert_eq!(cached.snapshot.syntax(), &parsed);
            assert_eq!(cached.snapshot.bound(), Some(&bind::bind(&parsed)));
        }
        assert_eq!(prepared.stats.semantic_reused, 0);
    }

    #[test]
    fn unchanged_buffers_reuse_parse_and_binding_with_current_project_ids() {
        let fixture = Fixture::new();
        let path = fixture.file("main.lua", "local value=42\nlocal copy=value\n");
        let mut workspace = ProjectWorkspace::new();
        let first_project = fixture.load(std::slice::from_ref(&path), &[]);
        let first = workspace.prepare_registered_sources(&first_project);
        assert_eq!((first.stats.parsed, first.stats.bound), (1, 1));
        let next_project = fixture.load(&[path], &[]);
        let next = workspace.prepare_registered_sources(&next_project);
        assert_ne!(first.sources[0].id, next.sources[0].id);
        assert!(Arc::ptr_eq(
            &first.sources[0].snapshot,
            &next.sources[0].snapshot
        ));
        assert_eq!((next.stats.parsed, next.stats.bound), (0, 0));
        assert_eq!((next.stats.parse_reused, next.stats.bind_reused), (1, 1));
        matches_cold(&next_project, &next);
    }

    #[test]
    fn exact_same_length_content_changes_create_a_new_revision() {
        let fixture = Fixture::new();
        let path = fixture.file("main.lua", "local value=42\n");
        let mut workspace = ProjectWorkspace::new();
        let first =
            workspace.prepare_registered_sources(&fixture.load(std::slice::from_ref(&path), &[]));
        fixture.file("main.lua", "local value=43\n");
        let project = fixture.load(&[path], &[]);
        let next = workspace.prepare_registered_sources(&project);
        assert!(!Arc::ptr_eq(
            &first.sources[0].snapshot,
            &next.sources[0].snapshot
        ));
        assert_ne!(
            first.sources[0].snapshot.revision(),
            next.sources[0].snapshot.revision()
        );
        assert_eq!(
            (next.stats.parsed, next.stats.bound, next.stats.parse_reused),
            (1, 1, 0)
        );
        assert_eq!(first.sources[0].snapshot.source(), "local value=42\n");
        matches_cold(&project, &next);
    }

    #[test]
    fn changed_membership_evicts_removed_inputs_and_preserves_the_remaining_snapshot() {
        let fixture = Fixture::new();
        let first_path = fixture.file("first.lua", "local a=1\n");
        let second_path = fixture.file("second.lua", "local b=2\n");
        let mut workspace = ProjectWorkspace::new();
        let first = workspace.prepare_registered_sources(
            &fixture.load(&[first_path.clone(), second_path.clone()], &[]),
        );
        let project = fixture.load(&[second_path], &[]);
        let next = workspace.prepare_registered_sources(&project);
        assert_eq!(
            (next.stats.evicted, workspace.retained_source_count()),
            (1, 1)
        );
        assert!(Arc::ptr_eq(
            &first.sources[1].snapshot,
            &next.sources[0].snapshot
        ));
        matches_cold(&project, &next);
        let restored_project = fixture.load(&[first_path], &[]);
        let restored = workspace.prepare_registered_sources(&restored_project);
        assert_eq!((restored.stats.parsed, restored.stats.parse_reused), (1, 0));
        matches_cold(&restored_project, &restored);
    }

    #[test]
    fn role_and_declaration_origin_are_part_of_source_identity() {
        let fixture = Fixture::new();
        let path = fixture.file("main.lua", "---@alias Value number\n");
        let mut workspace = ProjectWorkspace::new();
        let implementation =
            workspace.prepare_registered_sources(&fixture.load(std::slice::from_ref(&path), &[]));
        let project = fixture.load(&[], &[path]);
        let declaration = workspace.prepare_registered_sources(&project);
        assert_eq!(
            (declaration.stats.parsed, declaration.stats.parse_reused),
            (1, 0)
        );
        assert!(!Arc::ptr_eq(
            &implementation.sources[0].snapshot,
            &declaration.sources[0].snapshot
        ));
        matches_cold(&project, &declaration);
        let standard = workspace
            .prepare_registered_sources(&fixture.declaration("lua54", "---@alias Value number\n"));
        let alternate_project = fixture.declaration("lua51", "---@alias Value number\n");
        let alternate = workspace.prepare_registered_sources(&alternate_project);
        assert_eq!(
            (alternate.stats.parsed, alternate.stats.parse_reused),
            (1, 0)
        );
        assert!(!Arc::ptr_eq(
            &standard.sources[0].snapshot,
            &alternate.sources[0].snapshot
        ));
        matches_cold(&alternate_project, &alternate);
    }

    #[test]
    fn syntax_failures_are_bound_from_their_recovered_tree_and_reused() {
        let fixture = Fixture::new();
        let path = fixture.file("main.lua", "local =\n");
        let mut workspace = ProjectWorkspace::new();
        let first =
            workspace.prepare_registered_sources(&fixture.load(std::slice::from_ref(&path), &[]));
        assert!(first.sources[0].snapshot.syntax().has_lua_syntax_errors());
        assert_eq!((first.stats.parsed, first.stats.bound), (1, 1));
        let project = fixture.load(std::slice::from_ref(&path), &[]);
        let repeated = workspace.prepare_registered_sources(&project);
        assert_eq!(
            (repeated.stats.parse_reused, repeated.stats.bind_reused),
            (1, 1)
        );
        matches_cold(&project, &repeated);
        fixture.file("main.lua", "local fixed=1\n");
        let project = fixture.load(&[path], &[]);
        let corrected = workspace.prepare_registered_sources(&project);
        assert_eq!((corrected.stats.parsed, corrected.stats.bound), (1, 1));
        matches_cold(&project, &corrected);
    }

    #[test]
    fn namespace_changes_reuse_only_unedited_source_inputs() {
        let fixture = Fixture::new();
        let main = fixture.file("main.lua", "---@type Later\nlocal value=1\n");
        let declaration = fixture.file("later.def.lua", "---@alias Later number\n");
        let mut workspace = ProjectWorkspace::new();
        let first = workspace.prepare_registered_sources(&fixture.load(
            std::slice::from_ref(&main),
            std::slice::from_ref(&declaration),
        ));
        fixture.file("later.def.lua", "---@alias Later string\n");
        let project = fixture.load(&[main], &[declaration]);
        let changed = workspace.prepare_registered_sources(&project);
        assert_eq!((changed.stats.parsed, changed.stats.parse_reused), (1, 1));
        assert_eq!(changed.stats.semantic_reused, 0);
        assert!(Arc::ptr_eq(
            &first.sources[1].snapshot,
            &changed.sources[1].snapshot
        ));
        matches_cold(&project, &changed);
    }

    #[test]
    fn dependency_discovery_reuses_prepared_inputs_and_rechecks_missing_paths() {
        let fixture = Fixture::new();
        let main = fixture.file("main.lua", "local value=require('dependency')\n");
        let dependency = fixture.file("dependency.lua", "return 1\n");
        let resolver = Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap();
        let mut workspace = ProjectWorkspace::new();
        let project = Project::load(&resolver, [main.clone()], |source| {
            workspace.static_require_requests(source)
        });
        let first = workspace.prepare_registered_sources(&project);
        assert_eq!(
            (
                first.stats.parsed,
                first.stats.bound,
                first.stats.parse_reused
            ),
            (2, 2, 0)
        );
        matches_cold(&project, &first);
        let project = Project::load(&resolver, [main.clone()], |source| {
            workspace.static_require_requests(source)
        });
        let repeated = workspace.prepare_registered_sources(&project);
        assert_eq!(
            (
                repeated.stats.parsed,
                repeated.stats.bound,
                repeated.stats.parse_reused
            ),
            (0, 0, 2)
        );
        matches_cold(&project, &repeated);

        fs::remove_file(&dependency).unwrap();
        let project = Project::load(&resolver, [main.clone()], |source| {
            workspace.static_require_requests(source)
        });
        let missing = workspace.prepare_registered_sources(&project);
        assert!(matches!(
            project.dependencies[0].outcome,
            luanalysis_project::DependencyOutcome::Unresolved(_)
        ));
        assert_eq!((missing.stats.parse_reused, missing.stats.evicted), (1, 1));
        matches_cold(&project, &missing);

        fixture.file("dependency.lua", "return 'changed'\n");
        let project = Project::load(&resolver, [main], |source| {
            workspace.static_require_requests(source)
        });
        let restored = workspace.prepare_registered_sources(&project);
        assert!(matches!(
            project.dependencies[0].outcome,
            luanalysis_project::DependencyOutcome::Resolved(_)
        ));
        assert_eq!(
            (
                restored.stats.parsed,
                restored.stats.bound,
                restored.stats.parse_reused
            ),
            (1, 1, 1)
        );
        matches_cold(&project, &restored);
    }

    #[test]
    fn unchanged_sources_reuse_their_recipes_until_global_names_change() {
        let fixture = Fixture::new();
        let stable = fixture.file("stable.lua", "local function stable(value) return value end\nreturn stable\n");
        let edited = fixture.file("edited.lua", "local count=1\n");
        let mut workspace = ProjectWorkspace::new();
        let options = crate::project_check::ProjectCheckOptions { jobs: 1, ..Default::default() };
        let check = |workspace: &mut ProjectWorkspace| {
            let report = workspace.check_project(fixture.load(&[stable.clone(), edited.clone()], &[]), options.clone());
            let revision = workspace.sources.values().find(|snapshot| snapshot.source().starts_with("local function stable")).unwrap().revision;
            (std::sync::Arc::clone(workspace.recipes[&revision].flow()), report.has_errors())
        };
        let (first, _) = check(&mut workspace);
        fixture.file("edited.lua", "local count=2\n");
        let (reused, _) = check(&mut workspace);
        assert!(std::sync::Arc::ptr_eq(&first, &reused), "an edit without new global names reuses other sources' recipes");
        fixture.file("edited.lua", "added=2\n");
        let (rebuilt, failed) = check(&mut workspace);
        assert!(!std::sync::Arc::ptr_eq(&first, &rebuilt), "a new global name rebuilds every recipe");
        assert!(!failed);
    }
}
