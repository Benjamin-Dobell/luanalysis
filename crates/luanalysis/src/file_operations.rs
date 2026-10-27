//! Semantic edits for LSP workspace file operations. A request names paths, but only
//! a checked standard-loader dependency grants permission to change a Lua string.
use crate::project_check::ProjectCheckReport;
use luanalysis_project::{DependencyOutcome, Resolver, SourceOrigin};
use luanalysis_syntax::{ExprKind, Span};
use std::{collections::{HashMap, HashSet}, fs, path::{Component, Path, PathBuf}};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileRename {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileReferenceEdit {
    /// The original, pre-operation document. The client applies edits before moving it.
    pub path: PathBuf,
    pub span: Span,
    pub replacement: String,
}

/// Canonicalize existing ancestors too: the destination need not exist yet, and
/// the source may already have disappeared by didRenameFiles.
fn identity(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) { return path; }
    match (path.parent(), path.components().next_back()) {
        (Some(parent), Some(Component::Normal(name))) => identity(parent).join(name),
        (Some(parent), Some(Component::ParentDir)) => {
            let mut path = identity(parent);
            path.pop();
            path
        },
        (Some(parent), Some(Component::CurDir)) => identity(parent),
        _ => path.to_path_buf(),
    }
}

pub(crate) fn normalize_renames(renames: &[FileRename]) -> Result<Vec<FileRename>, String> {
    normalize(renames, false)
}

/// didRenameFiles follows the filesystem operation. Preserve the old final
/// spelling even on case-insensitive filesystems, where canonicalizing it now
/// could return the destination spelling instead of the old document key.
pub(crate) fn normalize_completed_renames(renames: &[FileRename]) -> Result<Vec<FileRename>, String> {
    normalize(renames, true)
}

fn requested_identity(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => identity(parent).join(name),
        _ => identity(path),
    }
}

fn normalize(renames: &[FileRename], completed: bool) -> Result<Vec<FileRename>, String> {
    let mut result: Vec<FileRename> = Vec::new();
    for rename in renames {
        if !rename.old_path.is_absolute() || !rename.new_path.is_absolute() {
            return Err("file-operation paths must be absolute".into());
        }
        let old_path = if completed { requested_identity(&rename.old_path) } else { identity(&rename.old_path) };
        // Preserve the requested final spelling (including a case-only rename).
        let new_path = requested_identity(&rename.new_path);
        if new_path != old_path && new_path.starts_with(&old_path) {
            return Err("a directory cannot be moved into itself".into());
        }
        if let Some(previous) = result.iter().find(|previous| previous.old_path == old_path) {
            if previous.new_path != new_path { return Err("one source has two rename destinations".into()); }
            continue;
        }
        if result.iter().any(|previous| previous.new_path == new_path && previous.old_path != old_path) {
            return Err("two sources have the same rename destination".into());
        }
        result.push(FileRename { old_path, new_path });
    }
    Ok(result)
}

/// A batch is simultaneous; moving A to B does not then move it again for B to C.
/// An explicit child operation takes precedence over its directory operation.
pub(crate) fn remap_path(path: &Path, renames: &[FileRename]) -> PathBuf {
    renames.iter().filter_map(|rename| path.strip_prefix(&rename.old_path).ok().map(|tail| (rename, tail)))
        .max_by_key(|(rename, _)| rename.old_path.components().count())
        .map_or_else(|| path.to_path_buf(), |(rename, tail)| {
            // Path::join("") appends a trailing separator, which turns a file
            // destination into an unresolvable package.path spelling.
            if tail.as_os_str().is_empty() { rename.new_path.clone() }
            else { rename.new_path.join(tail) }
        })
}

/// All-or-nothing: an affected import that cannot still name the original module
/// refuses the operation's semantic edit instead of silently changing its meaning.
/// The caller supplies a check of the CURRENT overlay revision, never an old snapshot.
pub(crate) fn rename_edits(report: &ProjectCheckReport, resolver: &Resolver, renames: &[FileRename]) -> Result<Vec<FileReferenceEdit>, String> {
    let renames = normalize_renames(renames)?;
    let suffix_files = resolver.config().suffix_files.iter().map(|file| remap_path(file, &renames)).collect();
    let view = AfterMove { report, resolver, renames: &renames, suffix_files };
    view.validate()?;
    let mut edits = Vec::new();
    let mut parsed = HashMap::new();
    for dependency in &report.project.dependencies {
        let DependencyOutcome::Resolved(target) = dependency.outcome else { continue; };
        let Some(source) = report.project.file(dependency.from) else { continue; };
        if source.origin != SourceOrigin::Filesystem { continue; }
        // Native checking proves intrinsic identity even through local aliases,
        // and rejects replaced globals, shadowed locals and custom loaders.
        let Some(checked) = report.file(dependency.from) else { continue; };
        let span = &dependency.request.span;
        if !checked.report.imports.iter().any(|import| import.start == span.start && import.end == span.end) { continue; }
        let Some(target) = report.project.file(target) else { continue; };
        let moved_target = remap_path(&target.path, &renames);
        let moved_source = remap_path(&source.path, &renames);
        let module = &dependency.request.module;
        // A module a root provides stays one a root provides, as Luanalysis v1 renames it; one
        // found by its path's suffix may still be found so, by as many parts as it can.
        let by_suffix = resolver.resolve_in_roots(module).map(|module| module.path).as_ref() != Ok(&target.path);
        // @meta module names are explicit contracts, independent of file paths.
        if view.resolve(&moved_source, module, by_suffix).as_ref() == Some(&moved_target) { continue; }
        let candidates = view.module_names(&moved_target, module, &target.path, by_suffix);
        let Some(replacement_module) = candidates.into_iter().find(|name| view.resolve(&moved_source, name, by_suffix).as_ref() == Some(&moved_target)) else {
            return Err(format!("{} cannot safely refer to moved module {} using the configured module roots and package.path", source.path.display(), moved_target.display()));
        };
        let file = parsed.entry(source.path.clone()).or_insert_with(|| luanalysis_syntax::parse(&source.source));
        if !file.diagnostics.is_empty() {
            return Err(format!("cannot update module references in invalid buffer {}", source.path.display()));
        }
        let mut literal = None;
        crate::module_requests::visit_expressions(&file.statements, &mut |expression| {
            if expression.span.start >= span.start && expression.span.end <= span.end
                && matches!(&expression.kind, ExprKind::String(value) if value.as_slice() == module.as_bytes())
            {
                literal = Some(expression.span);
            }
        });
        let literal = literal.ok_or_else(|| format!("module reference no longer matches {}", source.path.display()))?;
        let original = source.source.get(literal.start..literal.end).ok_or("module reference has an invalid span")?;
        edits.push(FileReferenceEdit { path: source.path.clone(), span: literal, replacement: string_literal(original, &replacement_module) });
    }
    edits.sort_by(|left, right| (&left.path, left.span.start, left.span.end).cmp(&(&right.path, right.span.start, right.span.end)));
    edits.dedup();
    if edits.windows(2).any(|pair| pair[0].path == pair[1].path && pair[0].span.end > pair[1].span.start) {
        return Err("overlapping module reference edits".into());
    }
    Ok(edits)
}

struct AfterMove<'a> {
    report: &'a ProjectCheckReport,
    resolver: &'a Resolver,
    renames: &'a [FileRename],
    /// The files a module no root provides is found among, where they will be.
    suffix_files: Vec<PathBuf>,
}

impl AfterMove<'_> {
    fn validate(&self) -> Result<(), String> {
        // A closed host namespace is owned by its manifest. Guessing a new Lua
        // string cannot repair paths stored in that external contract.
        if let Some(map) = &self.resolver.config().module_map {
            for (source, imports) in map {
                if remap_path(source, self.renames) != *source || imports.values().any(|target| remap_path(target, self.renames) != *target) {
                    return Err("file operations affecting --module-map paths require updating the host module-map manifest".into());
                }
            }
        }
        for rename in self.renames {
            if rename.old_path != rename.new_path && fs::symlink_metadata(&rename.new_path).is_ok()
                && remap_path(&identity(&rename.new_path), self.renames) == identity(&rename.new_path)
            {
                return Err(format!("rename destination already exists: {}", rename.new_path.display()));
            }
        }
        let mut paths = HashSet::new();
        for file in &self.report.project.files {
            if file.origin == SourceOrigin::Filesystem && !paths.insert(remap_path(&file.path, self.renames)) {
                return Err("file rename batch would merge distinct project files".into());
            }
        }
        Ok(())
    }

    /// Resolve in the virtual post-operation filesystem, honoring the same root
    /// and template order as Resolver::resolve_from, and its path suffixes when
    /// `by_suffix`. No files are moved or read from their destination while
    /// preparing the edit.
    fn resolve(&self, source: &Path, module: &str, by_suffix: bool) -> Option<PathBuf> {
        let config = self.resolver.config();
        if let Some(map) = &config.module_map {
            return map.get(source)?.get(module).cloned();
        }
        if let Some(provider) = self.report.project.provider(module) {
            return self.report.project.file(provider).map(|file| remap_path(&file.path, self.renames));
        }
        if module.is_empty() || module.contains('\0') { return None; }
        let name = module.replace('.', std::path::MAIN_SEPARATOR_STR);
        for root in &config.roots {
            for template in &config.templates {
                let candidate = identity(root).join(template.replace('?', &name));
                match self.file_at(&candidate) {
                    Ok(Some(path)) => return Some(path),
                    Ok(None) => {},
                    Err(()) => return None,
                }
            }
        }
        if !by_suffix { return None; }
        self.resolver.suffix_module(Some(source), module, &self.suffix_files)
    }

    fn file_at(&self, candidate: &Path) -> Result<Option<PathBuf>, ()> {
        // Preserve the future spelling before canonicalizing: on a
        // case-insensitive filesystem the old file can already be found under
        // the destination spelling, but it still needs a module-name edit.
        let candidate = if self.renames.iter().any(|rename| candidate.starts_with(&rename.new_path)) {
            candidate.to_path_buf()
        } else { identity(candidate) };
        // A destination can be supplied by a directory or an individual file.
        // Reverse only the most specific destination, then verify forward too.
        let moved = self.renames.iter().filter_map(|rename| candidate.strip_prefix(&rename.new_path).ok().map(|tail| (rename, tail)))
            .max_by_key(|(rename, _)| rename.new_path.components().count());
        if let Some((rename, tail)) = moved {
            let original = identity(&rename.old_path.join(tail));
            if remap_path(&original, self.renames) == candidate
                && (is_file(&original)? || self.report.project.file_by_path(&original).is_some())
            { return Ok(Some(candidate)); }
            return Ok(None);
        }
        if remap_path(&candidate, self.renames) != candidate { return Ok(None); }
        Ok(is_file(&candidate)?.then_some(candidate))
    }

    fn module_names(&self, target: &Path, previous: &str, original: &Path, by_suffix: bool) -> Vec<String> {
        let mut names = Vec::new();
        let Some(target) = target.to_str() else { return names; };
        let separator = if previous.contains('/') || previous.contains('\\') { "/" } else { "." };
        if by_suffix && !self.suffix_files.is_empty() {
            names = suffix_names(target, previous, separator, &self.resolver.config().templates);
        }
        let previous_path = previous.replace('.', std::path::MAIN_SEPARATOR_STR);
        let mut templates: Vec<_> = self.resolver.config().roots.iter().flat_map(|root| {
            self.resolver.config().templates.iter().map(move |template| (root, template))
        }).collect();
        // Retain the original package.path convention, such as pkg/init.lua ->
        // require("pkg"), before trying other valid names for the same file.
        templates.sort_by_key(|(root, template)| identity(&root.join(template.replace('?', &previous_path))) != original);
        for (root, template) in templates {
                // Mark only template placeholders: canonical Windows prefixes
                // and Unix root names can contain a literal question mark.
                let marker = char::from(0);
                let pattern = identity(&root.join(template.replace('?', &marker.to_string())));
                let Some(pattern) = pattern.to_str() else { continue; };
                let Some(name) = invert_template(pattern, target, marker) else { continue; };
                let name = name.replace(std::path::MAIN_SEPARATOR, separator);
                if !name.is_empty() && !names.contains(&name) { names.push(name); }
        }
        names
    }
}

/// The names that may find `target` by its path's suffix: for each template of one `?` after a
/// directory or nothing, the path's last parts before the template's suffix, as many as
/// `previous` has first, then fewest first.
fn suffix_names(target: &str, previous: &str, separator: &str, templates: &[String]) -> Vec<String> {
    let parts = previous.split(['.', '/', '\\']).count();
    let mut names = Vec::new();
    for template in templates {
        let Some((prefix, suffix)) = template.split_once('?') else { continue };
        if suffix.contains('?') || !(prefix.is_empty() || prefix.ends_with(['/', '\\'])) { continue; }
        let suffix = suffix.replace(['/', '\\'], std::path::MAIN_SEPARATOR_STR);
        let Some(stem) = target.strip_suffix(suffix.as_str()) else { continue };
        let components: Vec<&str> = stem.split(std::path::MAIN_SEPARATOR).collect();
        let prefix: Vec<&str> = prefix.split(['/', '\\']).filter(|part| !part.is_empty() && *part != ".").collect();
        let mut counts: Vec<usize> = (1..components.len()).collect();
        counts.sort_by_key(|count| (*count != parts, *count));
        for count in counts {
            let (before, name) = components.split_at(components.len() - count);
            if !before.ends_with(&prefix) { continue; }
            let name = name.join(separator);
            if !names.contains(&name) { names.push(name); }
        }
    }
    names
}

fn is_file(path: &Path) -> Result<bool, ()> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => Ok(false),
        // Resolver stops on I/O failures too; never skip an inaccessible earlier
        // candidate to claim that a later root will win after the operation.
        Err(_) => Err(()),
    }
}

/// Inverse of replacing every '?' with the SAME module path, including custom
/// prefixes/suffixes and templates containing several occurrences.
fn invert_template<'a>(template: &str, target: &'a str, marker: char) -> Option<&'a str> {
    let count = template.chars().filter(|character| *character == marker).count();
    if count == 0 { return None; }
    let constant = template.len() - count * marker.len_utf8();
    let variable = target.len().checked_sub(constant)?;
    if variable % count != 0 { return None; }
    let length = variable / count;
    let prefix = template.find(marker)?;
    let name = target.get(prefix..prefix + length)?;
    (template.replace(marker, name) == target).then_some(name)
}

fn string_literal(original: &str, module: &str) -> String {
    if original.starts_with('[') && !module.starts_with(['\r', '\n']) && !module.contains(['\r', '\n', '\0']) {
        if let Some(end) = original[1..].find('[') {
            let mut equals = end;
            loop {
                let closing = format!("]{}]", "=".repeat(equals));
                if !module.contains(&closing) { return format!("[{}[{module}{closing}", "=".repeat(equals)); }
                equals += 1;
            }
        }
    }
    let quote = if original.starts_with('"') { '"' } else { '\'' };
    let mut result = String::from(quote);
    for character in module.chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            character if character == quote => { result.push('\\'); result.push(character); },
            character if character.is_ascii_control() => result.push_str(&format!("\\{:03}", character as u8)),
            character => result.push(character),
        }
    }
    result.push(quote);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{incremental::ProjectWorkspace, project_check::ProjectCheckOptions, standard_environment::StandardEnvironmentProfile};
    use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(files: &[(&str, &str)]) -> Self {
            let root = std::env::temp_dir().join(format!("luanalysis-file-operations-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir_all(&root).unwrap();
            let result = Self(fs::canonicalize(root).unwrap());
            for (path, text) in files { result.write(path, text); }
            result
        }
        fn write(&self, path: &str, source: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        fn resolver(&self, roots: &[&str]) -> Resolver {
            self.resolver_with_suffixes(roots, &[])
        }
        /// Finds a module no root provides among `files` by its path's suffix.
        fn resolver_with_suffixes(&self, roots: &[&str], files: &[&str]) -> Resolver {
            let mut config = ProjectConfig::new(roots.iter().map(|root| self.0.join(root)));
            config.set_package_path("?.lua;?/init.lua;?.ttslua;?/init.ttslua");
            config.suffix_files = files.iter().map(|file| self.0.join(file)).collect();
            Resolver::new(config).unwrap()
        }
        fn check(&self, resolver: &Resolver, entry: &str, overlay: Option<&str>) -> ProjectCheckReport {
            let mut workspace = ProjectWorkspace::new();
            let overlays = overlay.into_iter().map(|text| (self.0.join(entry), text.to_owned())).collect();
            let project = Project::load_plan_with_overlays(resolver, ProjectLoadPlan::new([self.0.join(entry)]), &overlays, |source| workspace.static_require_requests(source));
            workspace.check_project(project, ProjectCheckOptions { jobs: 1, environment: StandardEnvironmentProfile::Lua54, ..ProjectCheckOptions::default() })
        }
        fn rename(&self, old: &str, new: &str) -> FileRename { FileRename { old_path: self.0.join(old), new_path: self.0.join(new) } }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }


    // Consume the original imported inputs and byte-for-byte expected files.
    // Do not duplicate their content here: import verification owns that oracle.
    macro_rules! legacy_file {
        ($path:literal) => {
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../luanalysis-fixtures/tests/legacy/", $path))
        };
    }

    fn replay_legacy_file_operation(before: &[(&str, &str)], after: &[(&str, &str)], old: &str, new: &str, after_module: &str) {
        let fixture = Fixture::new(before);
        let resolver = fixture.resolver(&[""]);
        let check_all = |files: &[(&str, &str)]| {
            let mut workspace = ProjectWorkspace::new();
            let plan = ProjectLoadPlan::new(files.iter().map(|(path, _)| fixture.0.join(path)));
            let project = Project::load_plan(&resolver, plan, |source| workspace.static_require_requests(source));
            workspace.check_project(project, ProjectCheckOptions {
                jobs: 1, environment: StandardEnvironmentProfile::Lua54, ..ProjectCheckOptions::default()
            })
        };
        let assert_dependency = |report: &ProjectCheckReport, target: &str, module: &str| {
            assert!(!report.has_errors(), "legacy file-operation check failed: {report:#?}");
            assert_eq!(report.project.dependencies.len(), 1, "the original fixtures have one require");
            let dependency = &report.project.dependencies[0];
            assert_eq!(dependency.request.module, module);
            let source = report.project.file(dependency.from).unwrap();
            assert_eq!(source.path, fs::canonicalize(fixture.0.join("B.lua")).unwrap());
            let DependencyOutcome::Resolved(target_id) = dependency.outcome else {
                panic!("original require did not resolve: {dependency:?}");
            };
            assert_eq!(report.project.file(target_id).unwrap().path, fs::canonicalize(fixture.0.join(target)).unwrap());
            // A syntactically collected dependency alone is insufficient: the
            // native checker must prove that this is the standard Lua loader.
            let imports = &report.file(dependency.from).unwrap().report.imports;
            assert!(imports.iter().any(|span| span.start == dependency.request.span.start && span.end == dependency.request.span.end));
        };
        let report = check_all(before);
        assert_dependency(&report, old, "A");
        let rename = fixture.rename(old, new);
        let edits = rename_edits(&report, &resolver, std::slice::from_ref(&rename)).unwrap();
        assert_eq!(edits.len(), 1, "the original operation changes only B.lua's require");
        assert_eq!(edits[0].path, fixture.0.join("B.lua"));
        let mut changed = std::collections::BTreeMap::new();
        // Spans index the original buffers. Apply each document's edits in
        // reverse byte order before performing the actual filesystem operation.
        for edit in edits.iter().rev() {
            let text = changed.entry(edit.path.clone()).or_insert_with(|| fs::read_to_string(&edit.path).unwrap());
            text.replace_range(edit.span.start..edit.span.end, &edit.replacement);
        }
        for (path, text) in changed { fs::write(path, text).unwrap(); }
        fs::create_dir_all(rename.new_path.parent().unwrap()).unwrap();
        fs::rename(&rename.old_path, &rename.new_path).unwrap();
        assert!(!rename.old_path.exists(), "the old file must disappear after the operation");
        for (path, expected) in after {
            assert_eq!(fs::read(fixture.0.join(path)).unwrap().as_slice(), expected.as_bytes(), "original after-file differs: {path}");
        }
        let mut actual_paths = Vec::new();
        let mut pending = vec![fixture.0.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() { pending.push(entry.path()); }
                else { actual_paths.push(entry.path().strip_prefix(&fixture.0).unwrap().to_path_buf()); }
            }
        }
        let mut expected_paths: Vec<_> = after.iter().map(|(path, _)| PathBuf::from(*path)).collect();
        actual_paths.sort();
        expected_paths.sort();
        assert_eq!(actual_paths, expected_paths, "the complete original after-file set must match");
        let moved_report = check_all(after);
        assert_dependency(&moved_report, new, after_module);
        assert_eq!(fs::canonicalize(&rename.new_path).unwrap(), resolver.resolve(after_module).unwrap().path);
    }

    #[test]
    fn original_legacy_move_file_fixture_is_applied_and_resolved() {
        replay_legacy_file_operation(&[
            ("A.lua", legacy_file!("resources/refactoring/moveFile/before/A.lua")),
            ("B.lua", legacy_file!("resources/refactoring/moveFile/before/B.lua")),
            ("to/C.lua", legacy_file!("resources/refactoring/moveFile/before/to/C.lua")),
        ], &[
            ("B.lua", legacy_file!("resources/refactoring/moveFile/after/B.lua")),
            ("to/A.lua", legacy_file!("resources/refactoring/moveFile/after/to/A.lua")),
            ("to/C.lua", legacy_file!("resources/refactoring/moveFile/after/to/C.lua")),
        ], "A.lua", "to/A.lua", "to.A");
    }

    #[test]
    fn original_legacy_rename_file_fixture_is_applied_and_resolved() {
        replay_legacy_file_operation(&[
            ("A.lua", legacy_file!("embedded/RenameTest/test_rename_file_before/A.lua")),
            ("B.lua", legacy_file!("embedded/RenameTest/test_rename_file_before/B.lua")),
        ], &[
            ("B.lua", legacy_file!("embedded/RenameTest/test_rename_file_after/B.lua")),
            ("C.lua", legacy_file!("embedded/RenameTest/test_rename_file_after/C.lua")),
        ], "A.lua", "C.lua", "C");
    }

    #[test]
    fn overlay_references_follow_original_standard_loader_and_preserve_literal_style() {
        let fixture = Fixture::new(&[("old.lua", "return 1\n"), ("main.lua", "return require('old')\n")]);
        let resolver = fixture.resolver(&[""]);
        let overlay = "local emoji='😀'; local load=require\nlocal a=load((-- keep\n'old'))\nlocal b=require [=[old]=]\ndo local require=function(_) return 1 end; require('old') end\nreturn a+b\n";
        let report = fixture.check(&resolver, "main.lua", Some(overlay));
        let edits = rename_edits(&report, &resolver, &[fixture.rename("old.lua", "to/new.lua")]).unwrap();
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert_eq!(edits[0].replacement, "'to.new'");
        assert_eq!(edits[1].replacement, "[=[to.new]=]");
        let mut applied = overlay.to_owned();
        for edit in edits.iter().rev() { applied.replace_range(edit.span.start..edit.span.end, &edit.replacement); }
        assert!(applied.contains("-- keep\n'to.new'"), "{applied}");
        assert!(applied.contains("require('old') end"), "{applied}");
        assert!(luanalysis_syntax::parse(&applied).diagnostics.is_empty());
    }

    #[test]
    fn ordered_module_roots_are_checked_after_the_move() {
        let fixture = Fixture::new(&[("one/old.lua", "return 1\n"), ("two/old.lua", "return 2\n"), ("main.lua", "return require('old')\n")]);
        let resolver = fixture.resolver(&["one", "two"]);
        let report = fixture.check(&resolver, "main.lua", None);
        let edits = rename_edits(&report, &resolver, &[fixture.rename("one/old.lua", "two/new.lua")]).unwrap();
        assert_eq!(edits[0].replacement, "'new'");
        // The other old.lua now wins under the unchanged name, so it must change.
        fixture.write("one/new.lua", "return 3\n");
        assert!(rename_edits(&report, &resolver, &[fixture.rename("one/old.lua", "two/new.lua")]).is_err());
        // Moving the shadowed file never edits references to the first root.
        assert!(rename_edits(&report, &resolver, &[fixture.rename("two/old.lua", "two/other.lua")]).unwrap().is_empty());
    }

    #[test]
    fn a_module_found_by_suffix_keeps_its_name_or_parts_and_one_a_root_provides_stays_so() {
        let fixture = Fixture::new(&[
            ("src/main.lua", "local a = require('ge.utils')\nlocal b = require('lib.ge.table')\nreturn a, b\n"),
            ("lib/ge/utils.lua", "return 1\n"), ("lib/ge/table.lua", "return 2\n"),
        ]);
        let files = ["src/main.lua", "lib/ge/utils.lua", "lib/ge/table.lua"];
        let resolver = fixture.resolver_with_suffixes(&[""], &files);
        let report = fixture.check(&resolver, "src/main.lua", None);
        assert!(!report.has_errors(), "{report:#?}");
        // A move that keeps the suffix keeps the name; a rename keeps as many parts.
        assert!(rename_edits(&report, &resolver, &[fixture.rename("lib/ge/utils.lua", "vendor/ge/utils.lua")]).unwrap().is_empty());
        let edits = rename_edits(&report, &resolver, &[fixture.rename("lib/ge/utils.lua", "lib/ge/helpers.lua")]).unwrap();
        assert_eq!(edits.iter().map(|edit| edit.replacement.as_str()).collect::<Vec<_>>(), ["'ge.helpers'"]);
        // A module the root provides is still named from the root after a move.
        let edits = rename_edits(&report, &resolver, &[fixture.rename("lib/ge/table.lua", "vendor/ge/table.lua")]).unwrap();
        assert_eq!(edits.iter().map(|edit| edit.replacement.as_str()).collect::<Vec<_>>(), ["'vendor.ge.table'"]);
    }

    #[test]
    fn a_directory_batch_is_simultaneous_and_keeps_slash_names_and_init_templates() {
        let fixture = Fixture::new(&[("pkg/a.ttslua", "return 1\n"), ("pkg/b/init.lua", "return 2\n"), ("main.lua", "local a=require('pkg/a'); return a+require('pkg.b')\n")]);
        let resolver = fixture.resolver(&[""]);
        let report = fixture.check(&resolver, "main.lua", None);
        let edits = rename_edits(&report, &resolver, &[fixture.rename("pkg", "next")]).unwrap();
        assert_eq!(edits.iter().map(|edit| edit.replacement.as_str()).collect::<Vec<_>>(), ["'next/a'", "'next.b'"]);
        // Both are actual spellings under the configured templates, including init.lua.
        let renames = [fixture.rename("pkg", "next"), fixture.rename("next", "last")];
        assert_eq!(remap_path(&fixture.0.join("pkg/a.ttslua"), &renames), fixture.0.join("next/a.ttslua"));
        let renames = [fixture.rename("pkg", "next"), fixture.rename("pkg/a.ttslua", "special.ttslua")];
        assert_eq!(remap_path(&fixture.0.join("pkg/a.ttslua"), &renames), fixture.0.join("special.ttslua"));
    }

    #[test]
    fn rename_refuses_collisions_and_out_of_root_moves_without_partial_edits() {
        let fixture = Fixture::new(&[("src/old.lua", "return 1\n"), ("src/used.lua", "return 2\n"), ("main.lua", "return require('old')\n")]);
        let resolver = fixture.resolver(&["src"]);
        let report = fixture.check(&resolver, "main.lua", None);
        assert!(rename_edits(&report, &resolver, &[fixture.rename("src/old.lua", "src/used.lua")]).is_err());
        assert!(rename_edits(&report, &resolver, &[fixture.rename("src/old.lua", "outside.lua")]).is_err());
        assert!(normalize_renames(&[fixture.rename("src", "src/inner")]).is_err());
    }

    #[test]
    fn unchanged_module_name_is_kept_when_it_still_resolves_to_the_moved_file() {
        let fixture = Fixture::new(&[("one/old.lua", "return 1\n"), ("main.lua", "return require('old')\n")]);
        let resolver = fixture.resolver(&["one", "two"]);
        let report = fixture.check(&resolver, "main.lua", None);
        assert!(rename_edits(&report, &resolver, &[fixture.rename("one/old.lua", "two/old.lua")]).unwrap().is_empty());
    }

    #[test]
    fn an_exact_file_rename_keeps_the_destination_without_a_trailing_separator() {
        let fixture = Fixture::new(&[("old.lua", "return 1\n")]);
        let rename = fixture.rename("old.lua", "nested/new.lua");
        let mapped = remap_path(&rename.old_path, std::slice::from_ref(&rename));
        // Path equality alone normalizes trailing separators; module-name
        // template inversion operates on the actual path string.
        assert_eq!(mapped.as_os_str(), rename.new_path.as_os_str());
        assert!(!mapped.to_string_lossy().ends_with(std::path::MAIN_SEPARATOR));
    }

    #[test]
    fn template_inversion_and_replacement_literals_preserve_lua_meaning() {
        assert_eq!(invert_template("/p/mod-?.lua", "/p/mod-sub/x.lua", '?'), Some("sub/x"));
        assert_eq!(invert_template("/p/?/?.lua", "/p/a/a.lua", '?'), Some("a"));
        assert_eq!(invert_template("/p/?/?.lua", "/p/a/b.lua", '?'), None);
        let marker = char::from(0);
        assert_eq!(invert_template(&format!("/literal?/mod-{marker}.lua"), "/literal?/mod-sub.lua", marker), Some("sub"));
        assert_eq!(invert_template(&format!(r"\\?\C:\project\mod-{marker}.lua"), r"\\?\C:\project\mod-sub.lua", marker), Some("sub"));
        for original in ["'old'", "\"old\"", "[[old]]", "[=[old]=]"] {
            for module in ["new.λ", "quote'\"\\name", "close]]and]=]", "line\nnext", "\nfirst"] {
                let literal = string_literal(original, module);
                let file = luanalysis_syntax::parse(&format!("return {literal}"));
                assert!(file.diagnostics.is_empty(), "{literal}");
                let mut decoded = None;
                crate::module_requests::visit_expressions(&file.statements, &mut |expression| {
                    if let ExprKind::String(value) = &expression.kind { decoded = Some(value.clone()); }
                });
                assert_eq!(decoded.as_deref(), Some(module.as_bytes()), "{literal}");
            }
        }
    }
}
