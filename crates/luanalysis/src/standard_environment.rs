//! Bundled original declaration membership. Selection grants capabilities by
//! exact registered FileIds, never by display paths or user library spelling.
use luanalysis_project::{DeclarationSource, DeclarationSourceError, FileId, Project};
use luanalysis_syntax::{ExprKind, StmtKind};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StandardEnvironmentProfile {
    /// Historical type compatibility and an original standard file-loader declaration.
    #[default]
    LegacyMinimal,
    /// No declarations at all, not even `type`, `assert` and `setmetatable`, for testing the
    /// checker in isolation. A host that declares its own library selects a Lua version with an
    /// empty `StandardLibraryAllowList`, as `--no-stdlib` does.
    None,
    /// Lua 5.0 declaration resources and original standard capabilities.
    Lua50,
    /// Lua 5.1 declaration resources and original standard capabilities.
    Lua51,
    /// Lua 5.2 declaration resources, including bit32, and original capabilities.
    Lua52,
    /// Lua 5.3 declaration resources and original standard capabilities.
    Lua53,
    /// Lua 5.4 declaration resources and original standard capabilities.
    Lua54,
}

impl StandardEnvironmentProfile {
    /// Whether this selection supplies an original standard environment.
    pub const fn is_selected_standard(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Whether the selection supplies a full versioned Lua standard library.
    pub const fn is_full_lua(self) -> bool {
        matches!(
            self,
            Self::Lua50 | Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54
        )
    }

    fn version(self) -> Option<&'static str> {
        match self {
            Self::Lua50 => Some("5.0"),
            Self::Lua51 => Some("5.1"),
            Self::Lua52 => Some("5.2"),
            Self::Lua53 => Some("5.3"),
            Self::Lua54 => Some("5.4"),
            Self::LegacyMinimal | Self::None => None,
        }
    }

    fn directory(self) -> Option<&'static str> {
        match self {
            Self::LegacyMinimal => Some("LegacyMinimal"),
            Self::None => None,
            Self::Lua50 => Some("Lua50"),
            Self::Lua51 => Some("Lua51"),
            Self::Lua52 => Some("Lua52"),
            Self::Lua53 => Some("Lua53"),
            Self::Lua54 => Some("Lua54"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StandardLibrarySource {
    LegacyRequire,
    Base,
    Bit32,
    Builtin,
    Coroutine,
    Debug,
    Io,
    Math,
    Os,
    Package,
    Table,
    String,
    Utf8,
    /// TypeScript's utility types, `Partial`, `Pick`, `Exclude` and their kin.
    Utility,
}
impl StandardLibrarySource {
    const ALL: [Self; 14] = [
        Self::LegacyRequire,
        Self::Base,
        Self::Bit32,
        Self::Builtin,
        Self::Coroutine,
        Self::Debug,
        Self::Io,
        Self::Math,
        Self::Os,
        Self::Package,
        Self::Table,
        Self::String,
        Self::Utf8,
        Self::Utility,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::LegacyRequire => "require",
            Self::Base => "global",
            Self::Bit32 => "bit32",
            Self::Builtin => "builtin",
            Self::Coroutine => "coroutine",
            Self::Debug => "debug",
            Self::Io => "io",
            Self::Math => "math",
            Self::Os => "os",
            Self::Package => "package",
            Self::Table => "table",
            Self::String => "string",
            Self::Utf8 => "utf8",
            Self::Utility => "utility",
        }
    }

    /// The library an allow-list names this file's members by. The builtin types and the legacy
    /// loader are not libraries; they are always included.
    fn library(self) -> Option<&'static str> {
        match self {
            Self::LegacyRequire | Self::Builtin | Self::Utility => None,
            Self::Base => Some("basic"),
            kind => Some(kind.name()),
        }
    }

    fn source(self, profile: StandardEnvironmentProfile) -> Option<&'static str> {
        // One file for every version of Lua, as the builtin types are; not v1's minimal one.
        if self == Self::Utility {
            return (!matches!(profile, StandardEnvironmentProfile::None | StandardEnvironmentProfile::LegacyMinimal))
                .then_some(include_str!("../stdlib/utility.def.lua"));
        }
        macro_rules! sources {
            ($directory:literal; $($kind:ident => $file:literal),* $(,)?) => {
                match self {
                    $(Self::$kind => Some(include_str!(concat!("../stdlib/", $directory, "/", $file, ".def.lua"))),)*
                    _ => None,
                }
            };
        }
        match profile {
            StandardEnvironmentProfile::None => None,
            StandardEnvironmentProfile::LegacyMinimal => {
                sources!("LegacyMinimal"; LegacyRequire => "require")
            }
            StandardEnvironmentProfile::Lua50 => sources!("Lua50";
                Base => "global", Builtin => "builtin", Coroutine => "coroutine", Debug => "debug",
                Io => "io", Math => "math", Os => "os", Table => "table", String => "string",
            ),
            StandardEnvironmentProfile::Lua51 => sources!("Lua51";
                Base => "global", Builtin => "builtin", Coroutine => "coroutine", Debug => "debug",
                Io => "io", Math => "math", Os => "os", Package => "package", Table => "table", String => "string",
            ),
            StandardEnvironmentProfile::Lua52 => sources!("Lua52";
                Base => "global", Bit32 => "bit32", Builtin => "builtin", Coroutine => "coroutine", Debug => "debug",
                Io => "io", Math => "math", Os => "os", Package => "package", Table => "table", String => "string",
            ),
            StandardEnvironmentProfile::Lua53 => sources!("Lua53";
                Base => "global", Bit32 => "bit32", Builtin => "builtin", Coroutine => "coroutine", Debug => "debug",
                Io => "io", Math => "math", Os => "os", Package => "package", Table => "table", String => "string", Utf8 => "utf8",
            ),
            StandardEnvironmentProfile::Lua54 => sources!("Lua54";
                Base => "global", Builtin => "builtin", Coroutine => "coroutine", Debug => "debug",
                Io => "io", Math => "math", Os => "os", Package => "package", Table => "table", String => "string", Utf8 => "utf8",
            ),
        }
    }
}

/// The parts of the standard library a host provides, when it provides only some of it, as a
/// sandbox does, or none of it, when the host declares its own. Each entry names a whole library
/// (`basic`, `coroutine`, `debug`, `io`, `math`, `os`, `package`, `string`, `table`, `utf8` or
/// `bit32`) or one member of one (`print`, `os.time`, `string.dump`). `basic` is the base
/// library's globals, such as `print` and `pairs`; from Lua 5.1, `require` belongs to `package`.
/// Anything not named is left out, except what Lua cannot be without: `type`, `assert`,
/// `setmetatable` and the builtin types (`string`, `number`, `table` and so on). An empty list
/// leaves out everything else. Leaving out the `string` library leaves out the methods of strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StandardLibraryAllowList {
    entries: Vec<String>,
}

impl StandardLibraryAllowList {
    pub fn new<S: Into<String>>(entries: impl IntoIterator<Item = S>) -> Self {
        let mut entries: Vec<String> = entries.into_iter().map(Into::into)
            .map(|entry| entry.trim().to_owned()).filter(|entry| !entry.is_empty()).collect();
        entries.sort();
        entries.dedup();
        Self { entries }
    }

    /// A comma-separated list, as `--stdlib` takes it.
    pub fn parse(list: &str) -> Self {
        Self::new(list.split(','))
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    fn allows(&self, library: &str, member: &str) -> bool {
        FUNDAMENTALS.contains(&member) || self.entries.iter().any(|entry| entry == library || entry == member)
    }

    /// Checks that every entry names a library or member of `profile`'s standard library.
    pub fn validate(&self, profile: StandardEnvironmentProfile) -> Result<(), String> {
        if !profile.is_full_lua() {
            return Err("a standard library allow-list requires a Lua version (--lua-version)".into());
        }
        let mut known = Vec::new();
        for kind in StandardLibrarySource::ALL {
            let (Some(library), Some(source)) = (kind.library(), kind.source(profile)) else { continue };
            known.push(library.to_owned());
            known.extend(standard_members(kind, profile, source).into_iter().map(|member| member.name));
        }
        let unknown: Vec<&str> = self.entries.iter().filter(|entry| !known.contains(entry)).map(String::as_str).collect();
        if unknown.is_empty() {
            return Ok(());
        }
        Err(format!(
            "the Lua {} standard library has no {} {}",
            profile.version().unwrap_or_default(),
            if unknown.len() == 1 { "library or member" } else { "libraries or members" },
            unknown.join(", ")
        ))
    }

    /// A short stable digest, which distinguishes the identities of filtered declarations.
    fn digest(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in self.entries.join(",").bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
        hash
    }
}

/// The standard functions every host provides: without them, it would not be Lua.
const FUNDAMENTALS: [&str; 3] = ["type", "assert", "setmetatable"];

/// One top-level declaration of a standard library file: a global (`print`), a library's own
/// table (`os`) or one of its members (`os.time`). Locals, such as the methods of file handles,
/// and other statements are not members; they are always kept.
struct StandardMember {
    name: String,
    library: &'static str,
    /// The library's own table, which stays while any of its members does.
    table: bool,
    statement: luanalysis_syntax::Span,
    /// The documentation comments directly above the statement.
    documentation: Option<luanalysis_syntax::Span>,
    class: bool,
}

fn standard_members(kind: StandardLibrarySource, profile: StandardEnvironmentProfile, source: &str) -> Vec<StandardMember> {
    let Some(library) = kind.library() else { return Vec::new() };
    let file = luanalysis_syntax::parse(source);
    let mut members = Vec::new();
    for statement in &file.statements {
        let target = match &statement.kind {
            StmtKind::FunctionDeclaration { target, .. } => target.as_ref(),
            StmtKind::Assign { targets, .. } if targets.len() == 1 => &targets[0],
            _ => continue,
        };
        let name = match &target.kind {
            ExprKind::Name(name) => name.text.clone(),
            ExprKind::Field { base, name } => match &base.kind {
                ExprKind::Name(base) if base.text == library || (kind == StandardLibrarySource::Base && base.text == "_G") => {
                    format!("{}.{}", base.text, name.text)
                }
                _ => continue,
            },
            _ => continue,
        };
        // Lua 5.1 moved `require` and `module` from the base library to `package`.
        let library = match name.as_str() {
            "require" | "module" if profile != StandardEnvironmentProfile::Lua50 => "package",
            _ => library,
        };
        let documentation = documentation_above(source, &file.comments, statement.span.start);
        let class = documentation.is_some_and(|span| source[span.start..span.end].contains("@class"));
        members.push(StandardMember { table: name == library, name, library, statement: statement.span, documentation, class });
    }
    members
}

/// The span of the comments directly above `start`, each on the line before the next.
fn documentation_above(source: &str, comments: &[luanalysis_syntax::Comment], start: usize) -> Option<luanalysis_syntax::Span> {
    let mut begin = start;
    for comment in comments.iter().rev().filter(|comment| comment.span.end <= start) {
        let gap = &source[comment.span.end..begin];
        if !gap.chars().all(char::is_whitespace) || gap.matches('\n').count() > 1 {
            break;
        }
        begin = comment.span.start;
    }
    (begin < start).then(|| luanalysis_syntax::Span::new(begin, start))
}

/// `source` with only the members `allow` names, or None when it names none of this file's.
/// Removed declarations become blank lines, so the members kept keep their line numbers.
fn allowed_source(kind: StandardLibrarySource, profile: StandardEnvironmentProfile, source: &'static str, allow: &StandardLibraryAllowList) -> Option<String> {
    if kind.library().is_none() {
        return Some(source.to_owned());
    }
    let members = standard_members(kind, profile, source);
    let kept = |member: &StandardMember| allow.allows(member.library, &member.name);
    let any_kept = members.iter().any(|member| !member.table && kept(member));
    // The string library's table is the class of strings, which every profile declares.
    if !any_kept && kind != StandardLibrarySource::String {
        return None;
    }
    let mut text = source.as_bytes().to_vec();
    let blank = |text: &mut Vec<u8>, span: luanalysis_syntax::Span| {
        for byte in &mut text[span.start..span.end] {
            if *byte != b'\n' { *byte = b' '; }
        }
    };
    let mut localised = Vec::new();
    for member in &members {
        if kept(member) || (member.table && any_kept) {
            continue;
        }
        if member.table && member.class {
            // The table's class stays declared, on a local the removed members no longer reach.
            localised.push(member.statement.start);
            continue;
        }
        if let Some(documentation) = member.documentation {
            blank(&mut text, documentation);
        }
        blank(&mut text, member.statement);
    }
    let mut text = String::from_utf8(text).expect("blanking keeps UTF-8");
    for start in localised.into_iter().rev() {
        text.insert_str(start, "local ");
    }
    Some(text)
}

pub(crate) struct StandardEnvironmentSelection {
    pub profile: StandardEnvironmentProfile,
    files: Vec<(FileId, StandardLibrarySource)>,
}
impl StandardEnvironmentSelection {
    pub(crate) fn empty(profile: StandardEnvironmentProfile) -> Self {
        Self {
            profile,
            files: Vec::new(),
        }
    }
    pub(crate) fn register(
        project: &mut Project,
        profile: StandardEnvironmentProfile,
        allow: Option<&StandardLibraryAllowList>,
    ) -> Result<Self, DeclarationSourceError> {
        let mut files = Vec::new();
        let Some(directory) = profile.directory() else {
            return Ok(Self { profile, files });
        };
        let allow = allow.filter(|_| profile.is_full_lua());
        for kind in StandardLibrarySource::ALL {
            let Some(source) = kind.source(profile) else {
                continue;
            };
            let (source, filtered) = match allow {
                Some(allow) => match allowed_source(kind, profile, source, allow) {
                    Some(filtered) => (filtered, format!("+{:016x}", allow.digest())),
                    None => continue,
                },
                None => (source.to_owned(), String::new()),
            };
            let file = project.add_declaration_source(DeclarationSource {
                identity: format!(
                    "luanalysis.std.{directory}.{}@{}{filtered}",
                    kind.name(),
                    env!("CARGO_PKG_VERSION")
                ),
                path: PathBuf::from(format!("stdlib/{directory}/{}.def.lua", kind.name())),
                source,
            })?;
            files.push((file, kind));
        }
        Ok(Self { profile, files })
    }
    pub(crate) fn kind(&self, file: FileId) -> Option<StandardLibrarySource> {
        self.files
            .iter()
            .find(|(id, _)| *id == file)
            .map(|(_, kind)| *kind)
    }
    pub(crate) fn files(&self) -> Vec<FileId> {
        self.files.iter().map(|(file, _)| *file).collect()
    }
}
