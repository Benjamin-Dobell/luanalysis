//! The names of a checked project: what a name at a position refers to, where that is defined,
//! every occurrence of it, and the edits that rename it. The language server answers definition,
//! references, highlights and rename from this index; library callers can build it from any
//! `ProjectCheckReport`.
//!
//! A local is lexical, so all its occurrences are in its own file. A global belongs to the
//! environment every file shares, so its occurrences are in all of them: each read or write of
//! the name where no local of that name (or `_ENV`) is in scope, and each `_G.name`,
//! `_G["name"]`, `_ENV.name` or `_ENV["name"]`. A global is defined by its writes, in
//! implementation and declaration files alike.
use crate::bind::{BoundFile, DeclarationKind, OccurrenceKind};
use crate::collections::HashMap;
use crate::project_check::ProjectCheckReport;
use luanalysis_project::{FileId, SourceFile, SourceOrigin, SourceRole};
use luanalysis_syntax::Span;
use std::path::{Path, PathBuf};

const KEYWORDS: [&str; 22] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// What a name refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Symbol {
    /// A local, parameter or loop variable, by its file and the span of its declaration.
    Local { file: FileId, declaration: Span },
    /// A global of the shared environment, by name.
    Global { name: String },
}

/// One occurrence of a symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolOccurrence {
    pub file: FileId,
    pub path: PathBuf,
    /// The name's own span; for `_G["name"]`, the span of the name inside the quotes.
    pub span: Span,
    /// A declaration of a local, or a write of a global.
    pub definition: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameError {
    /// There is no local or global at the position.
    NoSymbol,
    /// `self` is Lua's implicit receiver, which has no name of its own to rename.
    ImplicitReceiver,
    /// The new name is not a Lua name.
    InvalidName(String),
    /// The name is a library's, such as the standard library's or a `--library` declaration
    /// file's, which the project does not own and a rename cannot edit.
    Library(String),
    /// A global of the new name already exists, and renaming would make the two one.
    Exists(String),
    /// After renaming, the name at this position would refer to something else: the renamed
    /// name would be captured by another binding, or would capture another name.
    Capture { name: String, path: PathBuf, line: usize, column: usize },
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSymbol => f.write_str("there is no local or global to rename here"),
            Self::ImplicitReceiver => f.write_str("self is a method's implicit receiver and cannot be renamed"),
            Self::InvalidName(name) => write!(f, "{name:?} is not a Lua name"),
            Self::Library(name) => write!(f, "{name} is declared by a library and cannot be renamed"),
            Self::Exists(name) => write!(f, "a global named {name} already exists"),
            Self::Capture { name, path, line, column } => {
                write!(f, "renaming to {name} would change what the name at {}:{line}:{column} refers to", path.display())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    /// The declaration span of a local of the entry's file.
    Local(usize, usize),
    /// An index into `SymbolIndex::globals`.
    Global(usize),
}

#[derive(Clone, Debug)]
struct Entry {
    span: Span,
    key: Key,
    definition: bool,
    /// The occurrence of an implicit `self`, whose name is not written at its declaration.
    receiver: bool,
}

#[derive(Clone, Debug)]
struct FileSymbols {
    path: PathBuf,
    text: String,
    /// A declaration the checker carries, such as the standard library: not a file to edit.
    bundled: bool,
    /// A library's declarations, bundled or a `--library` file: not the project's to rename.
    library: bool,
    /// Sorted by start.
    entries: Vec<Entry>,
}

/// What a name occurrence refers to: a local by its declaration's span, or a global by name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Resolution {
    Local(Span),
    Global(String),
}

/// The symbols of every file of a checked project.
#[derive(Clone, Debug, Default)]
pub struct SymbolIndex {
    files: HashMap<FileId, FileSymbols>,
    globals: Vec<String>,
    global_ids: HashMap<String, usize>,
    /// Each global's occurrences, as (file, entry index).
    global_occurrences: Vec<Vec<(FileId, usize)>>,
}

impl SymbolIndex {
    pub fn new(report: &ProjectCheckReport) -> Self {
        Self::from_bound_sources(report.files.iter().filter_map(|checked| {
            Some((report.project.file(checked.file)?, checked.report.bound.as_deref()?))
        }))
    }

    /// Indexes a query session's current sources without checking their bodies again.
    pub(crate) fn from_bound_sources<'a>(sources: impl IntoIterator<Item = (&'a SourceFile, &'a BoundFile)>) -> Self {
        let mut index = Self::default();
        for (source, bound) in sources {
            let bundled = !matches!(source.origin, SourceOrigin::Filesystem);
            let library = bundled || source.role == SourceRole::Declaration;
            let entries = index.file_entries(&source.source, bound);
            for (position, entry) in entries.iter().enumerate() {
                if let Key::Global(global) = entry.key {
                    index.global_occurrences[global].push((source.id, position));
                }
            }
            index.files.insert(source.id, FileSymbols { path: source.path.clone(), text: source.source.clone(), bundled, library, entries });
        }
        index
    }

    fn global(&mut self, name: &str) -> usize {
        if let Some(&id) = self.global_ids.get(name) {
            return id;
        }
        let id = self.globals.len();
        self.globals.push(name.to_owned());
        self.global_ids.insert(name.to_owned(), id);
        self.global_occurrences.push(Vec::new());
        id
    }

    fn file_entries(&mut self, text: &str, bound: &BoundFile) -> Vec<Entry> {
        resolutions(text, bound).into_iter().map(|(span, resolution, definition, receiver)| Entry {
            span,
            key: match resolution {
                Resolution::Local(declaration) => Key::Local(declaration.start, declaration.end),
                Resolution::Global(name) => Key::Global(self.global(&name)),
            },
            definition,
            receiver,
        }).collect()
    }

    /// The file of `path`, when the project has one.
    pub fn file(&self, path: &Path) -> Option<FileId> {
        self.files.iter().find(|(_, file)| file.path == path).map(|(id, _)| *id)
    }

    /// The symbol whose name is at `offset` in `file`, and the span of that occurrence. A cursor
    /// just after a name is on it.
    pub fn symbol_at(&self, file: FileId, offset: usize) -> Option<(Symbol, Span)> {
        let (entry, _) = self.entry_at(file, offset)?;
        Some((self.symbol(file, entry), entry.span))
    }

    fn entry_at(&self, file: FileId, offset: usize) -> Option<(&Entry, &FileSymbols)> {
        let symbols = self.files.get(&file)?;
        let after = symbols.entries.partition_point(|entry| entry.span.start <= offset);
        let entry = symbols.entries[..after].last().filter(|entry| offset <= entry.span.end)?;
        Some((entry, symbols))
    }

    fn symbol(&self, file: FileId, entry: &Entry) -> Symbol {
        match entry.key {
            Key::Local(start, end) => Symbol::Local { file, declaration: Span::new(start, end) },
            Key::Global(global) => Symbol::Global { name: self.globals[global].clone() },
        }
    }

    /// Every occurrence of `symbol`, by file and then position.
    pub fn occurrences(&self, symbol: &Symbol) -> Vec<SymbolOccurrence> {
        let mut occurrences: Vec<SymbolOccurrence> = match symbol {
            Symbol::Local { file, declaration } => {
                let Some(symbols) = self.files.get(file) else { return Vec::new() };
                let key = Key::Local(declaration.start, declaration.end);
                symbols.entries.iter().filter(|entry| entry.key == key)
                    .map(|entry| occurrence(*file, symbols, entry)).collect()
            }
            Symbol::Global { name } => {
                let Some(&global) = self.global_ids.get(name) else { return Vec::new() };
                self.global_occurrences[global].iter()
                    .map(|(file, position)| {
                        let symbols = &self.files[file];
                        occurrence(*file, symbols, &symbols.entries[*position])
                    }).collect()
            }
        };
        occurrences.sort_by(|a, b| a.path.cmp(&b.path).then(a.span.start.cmp(&b.span.start)));
        occurrences
    }

    /// Where `symbol` is defined: a local's declaration, or each write of a global. Declarations
    /// the checker carries (the standard library) are not files, so they are left out.
    pub fn definitions(&self, symbol: &Symbol) -> Vec<SymbolOccurrence> {
        self.occurrences(symbol).into_iter()
            .filter(|occurrence| occurrence.definition && !self.files[&occurrence.file].bundled)
            .collect()
    }

    /// The occurrence a rename at `offset` would rename, after checking it can be renamed: not
    /// `self`, and not a library's name, which a rename could not edit everywhere it occurs.
    pub fn prepare_rename(&self, file: FileId, offset: usize) -> Result<(Symbol, Span), RenameError> {
        let (entry, symbols) = self.entry_at(file, offset).ok_or(RenameError::NoSymbol)?;
        if entry.receiver {
            return Err(RenameError::ImplicitReceiver);
        }
        let symbol = self.symbol(file, entry);
        if self.occurrences(&symbol).iter().any(|occurrence| self.files[&occurrence.file].library) {
            return Err(RenameError::Library(symbols.text[entry.span.start..entry.span.end].to_owned()));
        }
        Ok((symbol, entry.span))
    }

    /// The spans to replace with `name` to rename the symbol at `offset`, in every file. A rename
    /// must leave every name referring to what it did: the renamed name to its own declaration
    /// (or global), and every other name to its own.
    pub fn rename(&self, file: FileId, offset: usize, name: &str) -> Result<Vec<SymbolOccurrence>, RenameError> {
        if !is_name(name) {
            return Err(RenameError::InvalidName(name.to_owned()));
        }
        let (symbol, span) = self.prepare_rename(file, offset)?;
        let occurrences = self.occurrences(&symbol);
        if self.files[&file].text[span.start..span.end] == *name {
            return Ok(occurrences);
        }
        if matches!(symbol, Symbol::Global { .. })
            && self.global_ids.get(name).is_some_and(|&global| !self.global_occurrences[global].is_empty())
        {
            return Err(RenameError::Exists(name.to_owned()));
        }
        let mut files: Vec<FileId> = occurrences.iter().map(|occurrence| occurrence.file).collect();
        files.dedup();
        for id in files {
            let edits: Vec<Span> = occurrences.iter().filter(|occurrence| occurrence.file == id).map(|occurrence| occurrence.span).collect();
            self.verify_rename(id, &symbol, &edits, name)?;
        }
        Ok(occurrences)
    }

    /// Whether renaming `symbol`'s occurrences `edits` in `file` to `name` leaves every name of
    /// the file referring to what it did.
    fn verify_rename(&self, file: FileId, symbol: &Symbol, edits: &[Span], name: &str) -> Result<(), RenameError> {
        let symbols = &self.files[&file];
        let mut text = String::with_capacity(symbols.text.len());
        let mut last = 0;
        for edit in edits {
            text.push_str(&symbols.text[last..edit.start]);
            text.push_str(name);
            last = edit.end;
        }
        text.push_str(&symbols.text[last..]);
        // An offset in the renamed text: each edit ending at or before it moves it.
        let shift = |offset: usize| {
            let before = edits.iter().filter(|edit| edit.end <= offset);
            before.fold(offset, |offset, edit| offset + name.len() - (edit.end - edit.start))
        };
        let moved = |span: Span| Span::new(shift(span.start), shift(span.end));
        let parsed = luanalysis_syntax::parse(&text);
        let renamed: HashMap<usize, Resolution> = resolutions(&text, &crate::bind::bind(&parsed)).into_iter()
            .map(|(span, resolution, _, _)| (span.start, resolution))
            .collect();
        for entry in &symbols.entries {
            let edited = edits.iter().any(|edit| *edit == entry.span);
            let expected = match (&entry.key, symbol) {
                (_, Symbol::Global { .. }) if edited => Resolution::Global(name.to_owned()),
                (&Key::Local(start, end), _) => Resolution::Local(moved(Span::new(start, end))),
                (&Key::Global(global), _) => Resolution::Global(self.globals[global].clone()),
            };
            if renamed.get(&shift(entry.span.start)) != Some(&expected) {
                let (line, column) = line_column(&symbols.text, entry.span.start);
                return Err(RenameError::Capture { name: name.to_owned(), path: symbols.path.clone(), line, column });
            }
        }
        Ok(())
    }
}

/// The one-based line and column of `offset` in `text`, in characters.
pub(crate) fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    (before.matches('\n').count() + 1, before[line_start..].chars().count() + 1)
}

/// Every name occurrence of a bound file, sorted by start: its span, what it refers to, whether
/// it declares a local or writes a global, and whether it is an implicit `self`.
fn resolutions(text: &str, bound: &BoundFile) -> Vec<(Span, Resolution, bool, bool)> {
    let mut resolutions = Vec::new();
    for occurrence in &bound.occurrences {
        // A name the parser supplies, such as the value `---@cast x T` reads at its tag, is
        // not written there and cannot be renamed there.
        if text.get(occurrence.span.start..occurrence.span.end) != Some(occurrence.name.as_str()) {
            continue;
        }
        match occurrence.binding {
            Some(binding) => {
                let declaration = bound.declaration(binding);
                resolutions.push((
                    occurrence.span,
                    Resolution::Local(declaration.span),
                    occurrence.kind == OccurrenceKind::Declaration,
                    declaration.kind == DeclarationKind::ImplicitReceiver,
                ));
            }
            None if occurrence.kind != OccurrenceKind::Annotation
                && bound.environment_binding_at(occurrence.span.start).is_none() =>
            {
                let write = occurrence.kind == OccurrenceKind::Write;
                resolutions.push((occurrence.span, Resolution::Global(occurrence.name.clone()), write, false));
                if matches!(occurrence.name.as_str(), "_G" | "_ENV") && !write
                    && let Some((span, name, write)) = environment_field(text, occurrence.span)
                {
                    resolutions.push((span, Resolution::Global(name.to_owned()), write, false));
                }
            }
            None => {}
        }
    }
    resolutions.sort_by_key(|(span, ..)| span.start);
    resolutions
}

fn occurrence(file: FileId, symbols: &FileSymbols, entry: &Entry) -> SymbolOccurrence {
    SymbolOccurrence { file, path: symbols.path.clone(), span: entry.span, definition: entry.definition }
}

/// Whether `name` is a Lua name: not a keyword, a letter or underscore and then letters, digits
/// and underscores.
pub fn is_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !KEYWORDS.contains(&name)
}

/// The field after an environment table read at `table`: the name of `_G.name` or of
/// `_G["name"]`, with whether it is assigned (`_G.name = value`, `function _G.name()`).
fn environment_field(text: &str, table: Span) -> Option<(Span, &str, bool)> {
    let bytes = text.as_bytes();
    let skip = |mut at: usize| {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1 }
        at
    };
    let at = skip(table.end);
    let (span, after) = match bytes.get(at)? {
        b'.' if bytes.get(at + 1) != Some(&b'.') => {
            let start = skip(at + 1);
            let end = start + bytes[start..].iter().take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_').count();
            (Span::new(start, end), end)
        }
        b'[' => {
            let quote_at = skip(at + 1);
            let quote = *bytes.get(quote_at).filter(|quote| matches!(quote, b'"' | b'\''))?;
            let start = quote_at + 1;
            let end = start + bytes[start..].iter().take_while(|byte| **byte != quote && **byte != b'\\' && **byte != b'\n').count();
            if bytes.get(end) != Some(&quote) { return None; }
            let close = skip(end + 1);
            if bytes.get(close) != Some(&b']') { return None; }
            (Span::new(start, end), close + 1)
        }
        _ => return None,
    };
    let name = &text[span.start..span.end];
    if !is_name(name) {
        return None;
    }
    let next = skip(after);
    let assigned = bytes.get(next) == Some(&b'=') && bytes.get(next + 1) != Some(&b'=');
    let declared = text[..table.start].trim_end().strip_suffix("function")
        .is_some_and(|before| !before.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_'));
    Some((span, name, assigned || declared))
}
