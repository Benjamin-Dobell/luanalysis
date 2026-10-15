//! Bundled original declaration membership. Selection grants capabilities by
//! exact registered FileIds, never by display paths or user library spelling.
use luanalysis_project::{DeclarationSource, DeclarationSourceError, FileId, Project};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StandardEnvironmentProfile {
    /// Historical type compatibility and an original standard file-loader declaration.
    #[default]
    LegacyMinimal,
    /// No declaration profile or implicit builtin fallback.
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
}
impl StandardLibrarySource {
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
        }
    }

    fn source(self, profile: StandardEnvironmentProfile) -> Option<&'static str> {
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
    ) -> Result<Self, DeclarationSourceError> {
        let mut files = Vec::new();
        let Some(directory) = profile.directory() else {
            return Ok(Self { profile, files });
        };
        for kind in [
            StandardLibrarySource::LegacyRequire,
            StandardLibrarySource::Base,
            StandardLibrarySource::Bit32,
            StandardLibrarySource::Builtin,
            StandardLibrarySource::Coroutine,
            StandardLibrarySource::Debug,
            StandardLibrarySource::Io,
            StandardLibrarySource::Math,
            StandardLibrarySource::Os,
            StandardLibrarySource::Package,
            StandardLibrarySource::Table,
            StandardLibrarySource::String,
            StandardLibrarySource::Utf8,
        ] {
            let Some(source) = kind.source(profile) else {
                continue;
            };
            let file = project.add_declaration_source(DeclarationSource {
                identity: format!(
                    "luanalysis.std.{directory}.{}@{}",
                    kind.name(),
                    env!("CARGO_PKG_VERSION")
                ),
                path: PathBuf::from(format!("stdlib/{directory}/{}.def.lua", kind.name())),
                source: source.to_owned(),
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
