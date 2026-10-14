//! Filesystem/project hosting for original Lua source. This std-only crate
//! discovers files and resolves static require requests supplied by a parser
//! and binder. It does not parse source, execute Lua, or check types.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_PROJECT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct ProjectConfig {
    /// Ordered source roots. Relative roots are anchored once by Resolver::new.
    pub roots: Vec<PathBuf>,
    /// Ordered package.path templates within each root. Every ? is substituted.
    pub templates: Vec<String>,
    pub discovery_extensions: Vec<String>,
    pub excluded: Vec<PathBuf>,
}
impl ProjectConfig {
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            roots: roots.into_iter().collect(),
            templates: vec!["?.lua".into(), "?/init.lua".into()],
            discovery_extensions: vec!["lua".into()],
            excluded: Vec::new(),
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
        Ok(Self { config })
    }
    pub fn config(&self) -> &ProjectConfig {
        &self.config
    }
    pub fn resolve(&self, module: &str) -> Result<ResolvedModule, ResolveError> {
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
        let mut pending: Vec<_> = self.config.roots.iter().rev().cloned().collect();
        while let Some(path) = pending.pop() {
            let canonical = match fs::canonicalize(&path) {
                Ok(path) => path,
                Err(error) => {
                    result.errors.push(PathFailure::new(path, error));
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
                pending.extend(paths.into_iter().rev());
            }
        }
        result.files.sort();
        result.errors.sort_by(|a, b| a.path.cmp(&b.path));
        result
    }
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
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub id: FileId,
    pub path: PathBuf,
    pub source: String,
}
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
    pub dependencies: Vec<Dependency>,
    pub entry_errors: Vec<PathFailure>,
    /// A static cycle is recorded, not assumed to be a type error.
    pub cycles: Vec<DependencyCycle>,
    by_path: HashMap<PathBuf, FileId>,
}
impl Project {
    /// The callback must return only source-resolved static require requests.
    /// Shadowing/intrinsic recognition and dynamic-require diagnostics belong
    /// to the caller's parser/binder, not a textual scanner in this host.
    pub fn load(
        resolver: &Resolver,
        entries: impl IntoIterator<Item = PathBuf>,
        mut requires: impl FnMut(&SourceFile) -> Vec<RequireRequest>,
    ) -> Self {
        let id = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
        assert_ne!(id, u64::MAX, "project identities exhausted");
        let mut project = Self {
            id,
            files: Vec::new(),
            entries: Vec::new(),
            dependencies: Vec::new(),
            entry_errors: Vec::new(),
            cycles: Vec::new(),
            by_path: HashMap::new(),
        };
        let mut pending = VecDeque::new();
        for path in entries {
            match project.register(&path) {
                Ok((file, new)) => {
                    if !project.entries.contains(&file) {
                        project.entries.push(file);
                    }
                    if new {
                        pending.push_back(file);
                    }
                }
                Err(error) => project.entry_errors.push(error),
            }
        }
        while let Some(file) = pending.pop_front() {
            for request in requires(&project.files[file.index as usize]) {
                let source = &project.files[file.index as usize].source;
                let valid = request.span.start <= request.span.end
                    && request.span.end <= source.len()
                    && source.is_char_boundary(request.span.start)
                    && source.is_char_boundary(request.span.end);
                let outcome = if !valid {
                    DependencyOutcome::InvalidSpan
                } else {
                    match resolver.resolve(&request.module) {
                        Err(error) => DependencyOutcome::Unresolved(error),
                        Ok(module) => match project.register(&module.path) {
                            Ok((target, new)) => {
                                if new {
                                    pending.push_back(target);
                                }
                                DependencyOutcome::Resolved(target)
                            }
                            Err(error) => DependencyOutcome::Unreadable(error),
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
    fn register(&mut self, path: &Path) -> Result<(FileId, bool), PathFailure> {
        let alias = std::path::absolute(path).map_err(|error| PathFailure::new(path, error))?;
        let canonical = fs::canonicalize(&alias).map_err(|error| PathFailure::new(path, error))?;
        if let Some(id) = self.by_path.get(&canonical).copied() {
            self.by_path.insert(alias, id);
            return Ok((id, false));
        }
        let source = fs::read_to_string(&canonical)
            .map_err(|error| PathFailure::new(canonical.clone(), error))?;
        let id = FileId {
            project: self.id,
            index: u32::try_from(self.files.len()).expect("file index fits u32"),
        };
        self.files.push(SourceFile {
            id,
            path: canonical.clone(),
            source,
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
