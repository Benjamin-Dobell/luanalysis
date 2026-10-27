//! Filesystem/project hosting for original Lua source. This std-only crate
//! discovers files and resolves static require requests supplied by a parser
//! and binder. It does not parse source, execute Lua, or check types.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    fs, io,
    ops::Range,
    path::{Component, Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

mod ignore;
pub use ignore::IgnoreRules;

static NEXT_PROJECT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct ProjectConfig {
    /// Ordered source roots. Relative roots are anchored once by Resolver::new.
    pub roots: Vec<PathBuf>,
    /// Ordered package.path templates within each root. Every ? is substituted.
    pub templates: Vec<String>,
    pub discovery_extensions: Vec<String>,
    pub excluded: Vec<PathBuf>,
    /// What discovery skips in each root directory.
    pub ignore: IgnoreRules,
    /// The files a module that no root provides is found among by its path's suffix, as LuaLS
    /// finds it with `runtime.pathStrict` false: the shortest whose path ends with a template's
    /// path for the module. Canonical paths, as discovery gives them; none finds no module so.
    pub suffix_files: Vec<PathBuf>,
    /// A host's own module namespace, closed: each importing source's modules by their exact
    /// names. With it, a module is what the map says or unresolved; neither package.path nor a
    /// `---@meta` declaration provides one.
    pub module_map: Option<ModuleMap>,
}

/// Each importing source's modules: the file each exact, case-sensitive module name resolves to.
pub type ModuleMap = HashMap<PathBuf, HashMap<String, PathBuf>>;

impl ProjectConfig {
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            roots: roots.into_iter().collect(),
            templates: vec!["?.lua".into(), "?/init.lua".into()],
            module_map: None,
            discovery_extensions: vec!["lua".into()],
            excluded: Vec::new(),
            ignore: IgnoreRules::default(),
            suffix_files: Vec::new(),
        }
    }
    /// Empty path entries are ignored as in package.searchpath. This API does
    /// not read LUA_PATH or infer the importing file's directory.
    pub fn set_package_path(&mut self, package_path: &str) {
        self.templates = package_path
            .split(';')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathFailure {
    pub path: PathBuf,
    pub kind: io::ErrorKind,
    pub message: String,
}
impl PathFailure {
    fn new(path: impl Into<PathBuf>, error: io::Error) -> Self {
        Self {
            path: path.into(),
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}
impl std::fmt::Display for PathFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}
impl std::error::Error for PathFailure {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    InvalidModule(String),
    Missing {
        module: String,
        attempted: Vec<PathBuf>,
    },
    /// A host's module map names no such module for the importing source.
    Unmapped(String),
    Io(PathFailure),
}
impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidModule(module) => write!(f, "invalid module name {module:?}"),
            Self::Missing { module, attempted } => write!(
                f,
                "module {module:?} was not found in {} candidate paths",
                attempted.len()
            ),
            Self::Unmapped(module) => write!(f, "module {module:?} is not in the module map's imports of this file"),
            Self::Io(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ResolveError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModule {
    pub module: String,
    pub path: PathBuf,
    pub attempted: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Resolver {
    config: ProjectConfig,
    /// `config.suffix_files` by file name.
    suffix_index: HashMap<OsString, Vec<PathBuf>>,
}
impl Resolver {
    pub fn new(mut config: ProjectConfig) -> io::Result<Self> {
        let cwd = std::env::current_dir()?;
        for root in &mut config.roots {
            if root.is_relative() {
                *root = cwd.join(&*root);
            }
        }
        for excluded in &mut config.excluded {
            if excluded.is_relative() {
                *excluded = cwd.join(&*excluded);
            }
        }
        // A map's sources and targets are files, named as the project names them.
        if let Some(map) = config.module_map.take() {
            let mut canonical = ModuleMap::new();
            for (source, imports) in map {
                let source = mapped_file(&source)?;
                let mut targets = HashMap::new();
                for (module, target) in imports {
                    if module.is_empty() || module.contains('\0') {
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, "module map names must be nonempty and contain no NUL"));
                    }
                    targets.insert(module, mapped_file(&target)?);
                }
                if canonical.insert(source.clone(), targets).is_some() {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("duplicate module map source {}", source.display())));
                }
            }
            config.module_map = Some(canonical);
        }
        let mut suffix_index: HashMap<OsString, Vec<PathBuf>> = HashMap::new();
        for file in &config.suffix_files {
            if let Some(name) = file.file_name() {
                suffix_index.entry(name.to_owned()).or_default().push(file.clone());
            }
        }
        Ok(Self { config, suffix_index })
    }
    pub fn config(&self) -> &ProjectConfig {
        &self.config
    }
    pub fn resolve(&self, module: &str) -> Result<ResolvedModule, ResolveError> {
        self.resolve_from(None, module)
    }
    /// The module `module` names for the source at `source`: in a module map, the one it maps
    /// that name to for that source; otherwise the first package.path finds.
    pub fn resolve_from(&self, source: Option<&Path>, module: &str) -> Result<ResolvedModule, ResolveError> {
        if module.is_empty() || module.contains('\0') {
            return Err(ResolveError::InvalidModule(module.into()));
        }
        if let Some(map) = &self.config.module_map {
            return match source.and_then(|source| map.get(source)).and_then(|imports| imports.get(module)) {
                Some(path) => Ok(ResolvedModule { module: module.into(), path: path.clone(), attempted: vec![path.clone()] }),
                None => Err(ResolveError::Unmapped(module.into())),
            };
        }
        let attempted = match self.resolve_in_roots(module) {
            Err(ResolveError::Missing { attempted, .. }) => attempted,
            result => return result,
        };
        if let Some(path) = self.suffix_match(source, module, |name| self.suffix_index.get(name).into_iter().flatten().collect()) {
            return Ok(ResolvedModule { module: module.into(), path, attempted });
        }
        Err(ResolveError::Missing {
            module: module.into(),
            attempted,
        })
    }
    /// The first file a template finds for `module` in a root, in order: `package.searchpath`'s.
    pub fn resolve_in_roots(&self, module: &str) -> Result<ResolvedModule, ResolveError> {
        if module.is_empty() || module.contains('\0') {
            return Err(ResolveError::InvalidModule(module.into()));
        }
        let name = module.replace('.', std::path::MAIN_SEPARATOR_STR);
        let mut attempted = Vec::new();
        let mut seen = HashSet::new();
        for root in &self.config.roots {
            for template in &self.config.templates {
                let candidate = root.join(template.replace('?', &name));
                if !seen.insert(candidate.clone()) {
                    continue;
                }
                attempted.push(candidate.clone());
                match fs::metadata(&candidate) {
                    Ok(metadata) if metadata.is_file() => {
                        let path = fs::canonicalize(&candidate).map_err(|error| {
                            ResolveError::Io(PathFailure::new(candidate, error))
                        })?;
                        return Ok(ResolvedModule {
                            module: module.into(),
                            path,
                            attempted,
                        });
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) => {}
                    Err(error) => return Err(ResolveError::Io(PathFailure::new(candidate, error))),
                }
            }
        }
        Err(ResolveError::Missing {
            module: module.into(),
            attempted,
        })
    }
    /// The file among `files` that `module` is by its path's suffix, as `resolve_from` finds a
    /// module that no root provides among `suffix_files`.
    pub fn suffix_module(&self, source: Option<&Path>, module: &str, files: &[PathBuf]) -> Option<PathBuf> {
        self.suffix_match(source, module, |name| files.iter().filter(|file| file.file_name() == Some(name)).collect())
    }
    /// The shortest of the files `named` gives for a file name whose path ends with a template's
    /// path for `module`, earlier templates first among paths of a length. A source is not its
    /// own module; absolute templates and those leaving their root have no suffix.
    fn suffix_match<'a>(&self, source: Option<&Path>, module: &str, named: impl Fn(&std::ffi::OsStr) -> Vec<&'a PathBuf>) -> Option<PathBuf> {
        if self.config.suffix_files.is_empty() || self.config.module_map.is_some() {
            return None;
        }
        let name = module.replace('.', std::path::MAIN_SEPARATOR_STR);
        let mut best: Option<(usize, usize, &PathBuf)> = None;
        for (order, template) in self.config.templates.iter().enumerate() {
            let path = PathBuf::from(template.replace('?', &name));
            let mut suffix = PathBuf::new();
            for component in path.components() {
                match component {
                    Component::Normal(part) => suffix.push(part),
                    Component::CurDir => {}
                    _ => {
                        suffix.clear();
                        break;
                    }
                }
            }
            let Some(file_name) = suffix.file_name() else { continue };
            for file in named(file_name) {
                if Some(file.as_path()) == source || !file.ends_with(&suffix) {
                    continue;
                }
                let key = (file.as_os_str().len(), order, file);
                if best.is_none_or(|best| key < best) {
                    best = Some(key);
                }
            }
        }
        best.map(|(_, _, file)| file.clone())
    }
    pub fn discover(&self) -> Discovery {
        let mut result = Discovery::default();
        let mut directories = HashSet::new();
        let mut files = HashSet::new();
        let excluded: Vec<_> = self
            .config
            .excluded
            .iter()
            .map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
            .collect();
        let rules = &self.config.ignore;
        let configured: Vec<_> = rules.patterns.iter().filter_map(|line| ignore::pattern(line)).collect();
        // Each path, with the root it was found in and the `.gitignore` patterns there; a root
        // has none.
        let mut pending: Vec<(PathBuf, Option<Walk>)> = self.config.roots.iter().rev().map(|root| (root.clone(), None)).collect();
        while let Some((path, walk)) = pending.pop() {
            let ignored = |directory: bool| {
                let Some(walk) = &walk else { return false };
                if directory && rules.git_ignore && path.file_name() == Some(".git".as_ref()) {
                    return true;
                }
                walk.ignores(&configured, &path, directory)
            };
            let canonical = match fs::canonicalize(&path) {
                Ok(path) => path,
                Err(error) => {
                    if !ignored(false) {
                        result.errors.push(PathFailure::new(path, error));
                    }
                    continue;
                }
            };
            if excluded
                .iter()
                .any(|excluded| canonical.starts_with(excluded))
            {
                continue;
            }
            let metadata = match fs::metadata(&canonical) {
                Ok(metadata) => metadata,
                Err(error) => {
                    result.errors.push(PathFailure::new(canonical, error));
                    continue;
                }
            };
            if ignored(metadata.is_dir()) {
                continue;
            }
            if metadata.is_file() {
                let accepted = canonical
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        self.config
                            .discovery_extensions
                            .iter()
                            .any(|wanted| wanted.trim_start_matches('.') == ext)
                    });
                if accepted && files.insert(canonical.clone()) {
                    result.files.push(canonical);
                }
            } else if metadata.is_dir() && directories.insert(canonical.clone()) {
                let entries = match fs::read_dir(&canonical) {
                    Ok(entries) => entries,
                    Err(error) => {
                        result.errors.push(PathFailure::new(canonical, error));
                        continue;
                    }
                };
                let mut paths = Vec::new();
                for entry in entries {
                    match entry {
                        Ok(entry) => paths.push(entry.path()),
                        Err(error) => result
                            .errors
                            .push(PathFailure::new(canonical.clone(), error)),
                    }
                }
                paths.sort();
                let walk = Walk::within(walk, &canonical, rules.git_ignore);
                pending.extend(paths.into_iter().rev().map(|path| (path, Some(walk.clone()))));
            }
        }
        result.files.sort();
        result.errors.sort_by(|a, b| a.path.cmp(&b.path));
        result
    }
}

/// Where discovery is: the root directory it began in and the `.gitignore` patterns of the
/// directory it is reading.
#[derive(Clone)]
struct Walk {
    root: Rc<PathBuf>,
    scope: Option<Rc<Scope>>,
}

/// A directory's `.gitignore` patterns, then those of the directories it is in.
struct Scope {
    base: PathBuf,
    patterns: Vec<ignore::Pattern>,
    parent: Option<Rc<Scope>>,
}

impl Walk {
    /// The walk in `directory`, found by `walk` or a root: with `git_ignore`, its `.gitignore`
    /// (and in a root, `.git/info/exclude`, which yields to it) applies to what it holds.
    fn within(walk: Option<Walk>, directory: &Path, git_ignore: bool) -> Walk {
        let read = |path: PathBuf, parent: Option<Rc<Scope>>| {
            let patterns = fs::read_to_string(path).map(|text| ignore::patterns(&text)).unwrap_or_default();
            if patterns.is_empty() { parent } else { Some(Rc::new(Scope { base: directory.to_path_buf(), patterns, parent })) }
        };
        let mut walk = walk.unwrap_or_else(|| Walk {
            root: Rc::new(directory.to_path_buf()),
            scope: if git_ignore { read(directory.join(".git").join("info").join("exclude"), None) } else { None },
        });
        if git_ignore {
            walk.scope = read(directory.join(".gitignore"), walk.scope.take());
        }
        walk
    }

    /// Whether `configured` patterns, of the root, or the nearest `.gitignore` that names it
    /// ignore `path`.
    fn ignores(&self, configured: &[ignore::Pattern], path: &Path, directory: bool) -> bool {
        let relative = |base: &Path| ignore::relative(path, base)
            .or_else(|| path.file_name().map(|name| name.to_string_lossy().into_owned()));
        if !configured.is_empty()
            && let Some(relative) = relative(&self.root)
            && ignore::decision(configured, &relative, directory) == Some(true)
        {
            return true;
        }
        let mut scope = self.scope.as_deref();
        while let Some(current) = scope {
            if let Some(relative) = relative(&current.base)
                && let Some(ignored) = ignore::decision(&current.patterns, &relative, directory)
            {
                return ignored;
            }
            scope = current.parent.as_deref();
        }
        false
    }
}
/// A module map's file, as the project names it: its canonical path.
fn mapped_file(path: &Path) -> io::Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    if !fs::metadata(&path)?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("module map path is not a file: {}", path.display())));
    }
    Ok(path)
}

#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub files: Vec<PathBuf>,
    pub errors: Vec<PathFailure>,
}

/// An original source file identity belongs to one immutable loaded project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    pub project: u64,
    pub index: u32,
}

/// Checking provenance is independent of module search paths and file names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceRole {
    Implementation,
    Declaration,
}
impl std::fmt::Display for SourceRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Implementation => "implementation",
            Self::Declaration => "declaration",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectInput {
    pub path: PathBuf,
    pub role: SourceRole,
}
impl ProjectInput {
    pub fn new(path: impl Into<PathBuf>, role: SourceRole) -> Self {
        Self {
            path: path.into(),
            role,
        }
    }
}

/// Explicit libraries are loaded eagerly without becoming runtime entrypoints.
/// Discovery and environment/profile selection belong to the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectLoadPlan {
    pub entries: Vec<PathBuf>,
    pub libraries: Vec<ProjectInput>,
}
impl ProjectLoadPlan {
    pub fn new(entries: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            libraries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub id: FileId,
    pub path: PathBuf,
    pub source: String,
    pub role: SourceRole,
    pub origin: SourceOrigin,
}

/// Source provenance does not change lexical identity or module search policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceOrigin {
    Filesystem,
    DeclarationLibrary { identity: String },
}

/// An original declaration resource carried by the checker distribution.
/// `path` is its stable display origin, not a filesystem module candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationSource {
    pub identity: String,
    pub path: PathBuf,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeclarationSourceError {
    EmptyIdentity,
    ConflictingIdentity {
        identity: String,
        existing: FileId,
        existing_path: PathBuf,
        requested_path: PathBuf,
    },
}
impl std::fmt::Display for DeclarationSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyIdentity => f.write_str("declaration library identity must not be empty"),
            Self::ConflictingIdentity { identity, existing_path, requested_path, .. } => write!(
                f,
                "declaration library identity {identity:?} has conflicting source content or origins: {} and {}",
                existing_path.display(), requested_path.display(),
            ),
        }
    }
}
impl std::error::Error for DeclarationSourceError {}

/// Both requests identify the same canonical file but disagree on how it is
/// checked. The first explicit role is retained; the conflicting request fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRoleConflict {
    pub file: FileId,
    pub path: PathBuf,
    pub requested_path: PathBuf,
    pub existing_role: SourceRole,
    pub requested_role: SourceRole,
}
impl std::fmt::Display for SourceRoleConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "source role conflict for {}: loaded as {}, requested {} via {}",
            self.path.display(),
            self.existing_role,
            self.requested_role,
            self.requested_path.display(),
        )
    }
}
impl std::error::Error for SourceRoleConflict {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequireRequest {
    pub module: String,
    pub span: Range<usize>,
}
impl RequireRequest {
    pub fn new(module: impl Into<String>, span: Range<usize>) -> Self {
        Self {
            module: module.into(),
            span,
        }
    }
}
/// A file's static require requests, and the module it provides to `require` by name
/// whatever its path, as a LuaLS `---@meta name` definition file does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRequests {
    pub requires: Vec<RequireRequest>,
    pub provides: Option<String>,
}
impl From<Vec<RequireRequest>> for FileRequests {
    fn from(requires: Vec<RequireRequest>) -> Self {
        Self { requires, provides: None }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencyOutcome {
    Resolved(FileId),
    Unresolved(ResolveError),
    Unreadable(PathFailure),
    InvalidSpan,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub from: FileId,
    pub request: RequireRequest,
    pub outcome: DependencyOutcome,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyCycle {
    /// Closed chain: first and last are the same file.
    pub files: Vec<FileId>,
}
#[derive(Clone, Debug)]
pub struct Project {
    pub id: u64,
    pub files: Vec<SourceFile>,
    pub entries: Vec<FileId>,
    pub libraries: Vec<FileId>,
    pub dependencies: Vec<Dependency>,
    pub entry_errors: Vec<PathFailure>,
    pub library_errors: Vec<PathFailure>,
    pub role_conflicts: Vec<SourceRoleConflict>,
    /// A static cycle is recorded, not assumed to be a type error.
    pub cycles: Vec<DependencyCycle>,
    by_path: HashMap<PathBuf, FileId>,
    declaration_sources: HashMap<String, FileId>,
    /// Modules provided by name, which `require` resolves before searching paths.
    provided: HashMap<String, FileId>,
}
impl Project {
    /// The callback must return only source-resolved static require requests.
    /// Shadowing/intrinsic recognition and dynamic-require diagnostics belong
    /// to the caller's parser/binder, not a textual scanner in this host.
    pub fn load<R: Into<FileRequests>>(
        resolver: &Resolver,
        entries: impl IntoIterator<Item = PathBuf>,
        requires: impl FnMut(&SourceFile) -> R,
    ) -> Self {
        Self::load_plan(resolver, ProjectLoadPlan::new(entries), requires)
    }

    /// Registers explicit library roles before loading implementation entries
    /// and their dependency closure. The callback is never invoked for a
    /// declaration source, even when a require resolves to that canonical file.
    pub fn load_plan<R: Into<FileRequests>>(
        resolver: &Resolver,
        plan: ProjectLoadPlan,
        requires: impl FnMut(&SourceFile) -> R,
    ) -> Self {
        Self::load_plan_with_overlays(resolver, plan, &HashMap::new(), requires)
    }

    /// As `load_plan`, but a file whose canonical path is in `overlays` is read
    /// from there instead of the filesystem: an editor's unsaved buffers.
    ///
    /// Every entry and library is read before any request is resolved, so a module an entry
    /// provides by name is found by every file that requires it.
    pub fn load_plan_with_overlays<R: Into<FileRequests>>(
        resolver: &Resolver,
        plan: ProjectLoadPlan,
        overlays: &HashMap<PathBuf, String>,
        mut requires: impl FnMut(&SourceFile) -> R,
    ) -> Self {
        let id = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
        assert_ne!(id, u64::MAX, "project identities exhausted");
        let mut project = Self {
            id,
            files: Vec::new(),
            entries: Vec::new(),
            libraries: Vec::new(),
            dependencies: Vec::new(),
            entry_errors: Vec::new(),
            library_errors: Vec::new(),
            role_conflicts: Vec::new(),
            cycles: Vec::new(),
            by_path: HashMap::new(),
            declaration_sources: HashMap::new(),
            provided: HashMap::new(),
        };
        let mut pending = VecDeque::new();
        for input in plan.libraries {
            match project.register(&input.path, Some(input.role), overlays) {
                Ok((file, new)) => {
                    if !project.libraries.contains(&file) {
                        project.libraries.push(file);
                    }
                    if new && input.role == SourceRole::Implementation {
                        pending.push_back(file);
                    }
                }
                Err(RegisterError::Path(error)) => project.library_errors.push(error),
                Err(RegisterError::Role(error)) => project.role_conflicts.push(error),
            }
        }
        for path in plan.entries {
            match project.register(&path, Some(SourceRole::Implementation), overlays) {
                Ok((file, new)) => {
                    if !project.entries.contains(&file) {
                        project.entries.push(file);
                    }
                    if new {
                        pending.push_back(file);
                    }
                }
                Err(RegisterError::Path(error)) => project.entry_errors.push(error),
                Err(RegisterError::Role(error)) => project.role_conflicts.push(error),
            }
        }
        let mut read: VecDeque<(FileId, Vec<RequireRequest>)> = pending
            .into_iter()
            .map(|file| (file, project.requests(file, &mut requires)))
            .collect();
        while let Some((file, requests)) = read.pop_front() {
            for request in requests {
                let source = &project.files[file.index as usize].source;
                let valid = request.span.start <= request.span.end
                    && request.span.end <= source.len()
                    && source.is_char_boundary(request.span.start)
                    && source.is_char_boundary(request.span.end);
                let outcome = if !valid {
                    DependencyOutcome::InvalidSpan
                } else if resolver.config.module_map.is_none()
                    && let Some(&target) = project.provided.get(&request.module)
                {
                    DependencyOutcome::Resolved(target)
                } else {
                    match resolver.resolve_from(Some(&project.files[file.index as usize].path), &request.module) {
                        Err(error) => DependencyOutcome::Unresolved(error),
                        Ok(module) => match project.register(&module.path, None, overlays) {
                            Ok((target, new)) => {
                                if new
                                    && project.files[target.index as usize].role
                                        == SourceRole::Implementation
                                {
                                    let requests = project.requests(target, &mut requires);
                                    read.push_back((target, requests));
                                }
                                DependencyOutcome::Resolved(target)
                            }
                            Err(RegisterError::Path(error)) => DependencyOutcome::Unreadable(error),
                            Err(RegisterError::Role(_)) => {
                                unreachable!("an implicit dependency preserves an explicit role")
                            }
                        },
                    }
                };
                project.dependencies.push(Dependency {
                    from: file,
                    request,
                    outcome,
                });
            }
        }
        project.cycles = project.find_cycles();
        project
    }

    /// Reads a file's requests, recording the module it provides. The first file to provide
    /// a module keeps it.
    fn requests<R: Into<FileRequests>>(
        &mut self,
        file: FileId,
        requires: &mut impl FnMut(&SourceFile) -> R,
    ) -> Vec<RequireRequest> {
        let FileRequests { requires, provides } = requires(&self.files[file.index as usize]).into();
        if let Some(module) = provides {
            self.provided.entry(module).or_insert(file);
        }
        requires
    }

    /// The file that provides `module` by name, if one does.
    pub fn provider(&self, module: &str) -> Option<FileId> {
        self.provided.get(module).copied()
    }

    /// Adds immutable original declaration text without filesystem IO, runtime
    /// entries, dependency extraction or changes to a resolver's search roots.
    /// Identical resources deduplicate; conflicting reuse of an identity fails
    /// without replacing the first source or allocating another file identity.
    pub fn add_declaration_source(
        &mut self,
        source: DeclarationSource,
    ) -> Result<FileId, DeclarationSourceError> {
        if source.identity.is_empty() {
            return Err(DeclarationSourceError::EmptyIdentity);
        }
        if let Some(&id) = self.declaration_sources.get(&source.identity) {
            let existing = self.file(id).expect("registered declaration source");
            if existing.path == source.path && existing.source == source.source {
                return Ok(id);
            }
            return Err(DeclarationSourceError::ConflictingIdentity {
                identity: source.identity,
                existing: id,
                existing_path: existing.path.clone(),
                requested_path: source.path,
            });
        }
        let id = FileId {
            project: self.id,
            index: u32::try_from(self.files.len()).expect("file index fits u32"),
        };
        self.declaration_sources.insert(source.identity.clone(), id);
        self.files.push(SourceFile {
            id,
            path: source.path,
            source: source.source,
            role: SourceRole::Declaration,
            origin: SourceOrigin::DeclarationLibrary { identity: source.identity },
        });
        self.libraries.push(id);
        Ok(id)
    }
    /// Replaces an implementation file's text in place, as an editor's buffer changes, when its
    /// static require requests (`requests`, of the new text) name the same modules in the same
    /// order: the loaded files and their resolutions stay as they are, and only the requests'
    /// spans move. Returns false, changing nothing, when the requests differ; the project must
    /// then be loaded again.
    pub fn replace_source(&mut self, id: FileId, source: String, requests: impl Into<FileRequests>) -> bool {
        let Some(file) = self.file(id) else { return false };
        if file.role != SourceRole::Implementation { return false; }
        let FileRequests { requires: requests, provides } = requests.into();
        let provided = self.provided.iter().find(|&(_, &file)| file == id).map(|(module, _)| module.as_str());
        if provided != provides.as_deref() {
            return false;
        }
        let edges: Vec<usize> = (0..self.dependencies.len()).filter(|&index| self.dependencies[index].from == id).collect();
        if edges.len() != requests.len()
            || edges.iter().zip(&requests).any(|(&index, request)| self.dependencies[index].request.module != request.module)
        {
            return false;
        }
        for (index, request) in edges.into_iter().zip(requests) {
            self.dependencies[index].request = request;
        }
        self.files[id.index as usize].source = source;
        true
    }

    pub fn file(&self, id: FileId) -> Option<&SourceFile> {
        (id.project == self.id)
            .then(|| self.files.get(id.index as usize))
            .flatten()
    }
    pub fn file_by_path(&self, path: &Path) -> Option<&SourceFile> {
        let absolute = std::path::absolute(path).ok()?;
        if let Some(id) = self.by_path.get(&absolute) {
            return self.file(*id);
        }
        let canonical = fs::canonicalize(path).ok()?;
        self.by_path.get(&canonical).and_then(|id| self.file(*id))
    }
    fn register(
        &mut self,
        path: &Path,
        role: Option<SourceRole>,
        overlays: &HashMap<PathBuf, String>,
    ) -> Result<(FileId, bool), RegisterError> {
        let alias = std::path::absolute(path)
            .map_err(|error| RegisterError::Path(PathFailure::new(path, error)))?;
        let canonical = match fs::canonicalize(&alias) {
            Ok(canonical) => canonical,
            // An editor's open document need not exist on disk: its text is its source.
            Err(_) if overlays.contains_key(&alias) => alias.clone(),
            Err(error) => return Err(RegisterError::Path(PathFailure::new(path, error))),
        };
        if let Some(id) = self.by_path.get(&canonical).copied() {
            let existing_role = self.files[id.index as usize].role;
            if let Some(requested_role) = role.filter(|role| *role != existing_role) {
                return Err(RegisterError::Role(SourceRoleConflict {
                    file: id,
                    path: canonical,
                    requested_path: path.to_path_buf(),
                    existing_role,
                    requested_role,
                }));
            }
            self.by_path.insert(alias, id);
            return Ok((id, false));
        }
        let source = match overlays.get(&canonical) {
            Some(source) => source.clone(),
            None => fs::read_to_string(&canonical)
                .map_err(|error| RegisterError::Path(PathFailure::new(canonical.clone(), error)))?,
        };
        let id = FileId {
            project: self.id,
            index: u32::try_from(self.files.len()).expect("file index fits u32"),
        };
        self.files.push(SourceFile {
            id,
            path: canonical.clone(),
            source,
            role: role.unwrap_or(SourceRole::Implementation),
            origin: SourceOrigin::Filesystem,
        });
        self.by_path.insert(canonical, id);
        self.by_path.insert(alias, id);
        Ok((id, true))
    }
    fn find_cycles(&self) -> Vec<DependencyCycle> {
        let mut edges = vec![Vec::new(); self.files.len()];
        for edge in &self.dependencies {
            if let DependencyOutcome::Resolved(to) = edge.outcome {
                if !edges[edge.from.index as usize].contains(&to) {
                    edges[edge.from.index as usize].push(to);
                }
            }
        }
        let mut state = vec![0u8; self.files.len()];
        let mut stack = Vec::new();
        let mut positions = vec![None; self.files.len()];
        let mut cycles = Vec::new();
        // Explicit DFS frames avoid a host stack overflow on long require chains.
        for index in 0..self.files.len() {
            if state[index] != 0 {
                continue;
            }
            let start = FileId {
                project: self.id,
                index: index as u32,
            };
            state[index] = 1;
            positions[index] = Some(stack.len());
            stack.push(start);
            let mut frames = vec![(start, 0usize)];
            while let Some((file, next)) = frames.last_mut() {
                if *next == edges[file.index as usize].len() {
                    let file = *file;
                    frames.pop();
                    stack.pop();
                    positions[file.index as usize] = None;
                    state[file.index as usize] = 2;
                    continue;
                }
                let target = edges[file.index as usize][*next];
                *next += 1;
                match state[target.index as usize] {
                    0 => {
                        state[target.index as usize] = 1;
                        positions[target.index as usize] = Some(stack.len());
                        stack.push(target);
                        frames.push((target, 0));
                    }
                    1 => {
                        let from = positions[target.index as usize].unwrap();
                        let mut files = stack[from..].to_vec();
                        files.push(target);
                        cycles.push(DependencyCycle { files });
                    }
                    _ => {}
                }
            }
        }
        cycles
    }
}

enum RegisterError {
    Path(PathFailure),
    Role(SourceRoleConflict),
}
