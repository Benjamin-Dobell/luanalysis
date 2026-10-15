//! Bundled original declaration membership. Selection grants capabilities by
//! exact registered FileIds, never by display paths or user library spelling.
use luanalysis_project::{DeclarationSource, DeclarationSourceError, FileId, Project};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StandardEnvironmentProfile {
    /// Preserve the original low-level API's sole implicit type operation.
    #[default]
    LegacyMinimal,
    /// No declaration profile or implicit builtin fallback.
    None,
    /// Reviewed Lua 5.4 declarations and original standard-value capabilities.
    Lua54,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StandardLibrarySource {
    Base,
    Math,
    Table,
    String,
}
impl StandardLibrarySource {
    fn name(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Math => "math",
            Self::Table => "table",
            Self::String => "string",
        }
    }
    fn source(self) -> &'static str {
        match self {
            Self::Base => include_str!("../stdlib/Lua54/base.def.lua"),
            Self::Math => include_str!("../stdlib/Lua54/math.def.lua"),
            Self::Table => include_str!("../stdlib/Lua54/table.def.lua"),
            Self::String => include_str!("../stdlib/Lua54/string.def.lua"),
        }
    }
}

pub(crate) struct StandardEnvironmentSelection {
    pub profile: StandardEnvironmentProfile,
    files: Vec<(FileId, StandardLibrarySource)>,
}
impl StandardEnvironmentSelection {
    pub(crate) fn empty(profile: StandardEnvironmentProfile) -> Self {
        Self { profile, files: Vec::new() }
    }
    pub(crate) fn register(
        project: &mut Project,
        profile: StandardEnvironmentProfile,
    ) -> Result<Self, DeclarationSourceError> {
        let mut files = Vec::new();
        if profile == StandardEnvironmentProfile::Lua54 {
            for kind in [
                StandardLibrarySource::Base,
                StandardLibrarySource::Math,
                StandardLibrarySource::Table,
                StandardLibrarySource::String,
            ] {
                let identity = format!(
                    "luanalysis.std.Lua54.{}@{}",
                    kind.name(),
                    env!("CARGO_PKG_VERSION")
                );
                let file = project.add_declaration_source(DeclarationSource {
                    identity,
                    path: PathBuf::from(format!("stdlib/Lua54/{}.def.lua", kind.name())),
                    source: kind.source().to_owned(),
                })?;
                files.push((file, kind));
            }
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
