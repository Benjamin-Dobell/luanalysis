//! Rename rejections retain their source locations until the protocol chooses an encoding.
use crate::symbols::{RenameError, SymbolRenameFailure};
use luanalysis_project::{SourceFile, SourceOrigin, SourceRole};
use luanalysis_syntax::Span;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenameReason {
    NoSymbol,
    ImplicitReceiver,
    InvalidName,
    LibraryOwned,
    NameConflict,
    Capture,
    MemberConflict,
    AmbiguousMember,
    TypeConflict,
    BuiltInType,
    Unavailable,
}

impl RenameReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoSymbol => "noSymbol",
            Self::ImplicitReceiver => "implicitReceiver",
            Self::InvalidName => "invalidName",
            Self::LibraryOwned => "libraryOwned",
            Self::NameConflict => "nameConflict",
            Self::Capture => "capture",
            Self::MemberConflict => "memberConflict",
            Self::AmbiguousMember => "ambiguousMember",
            Self::TypeConflict => "typeConflict",
            Self::BuiltInType => "builtInType",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenameRelatedInformation {
    pub path: PathBuf,
    pub span: Span,
    /// The original source, including an editor overlay when the failure came from one.
    pub text: String,
    pub origin: SourceOrigin,
    pub role: SourceRole,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenameFailure {
    pub message: String,
    pub reason: RenameReason,
    pub related: Vec<RenameRelatedInformation>,
}

impl RenameFailure {
    pub(crate) fn new(reason: RenameReason, message: impl Into<String>) -> Self {
        Self { message: message.into(), reason, related: Vec::new() }
    }

    pub(crate) fn with_related(mut self, source: &SourceFile, span: Span) -> Self {
        self.related.push(RenameRelatedInformation {
            path: source.path.clone(), span, text: source.source.to_string(),
            origin: source.origin.clone(), role: source.role,
        });
        self
    }
}

impl std::fmt::Display for RenameFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<SymbolRenameFailure> for RenameFailure {
    fn from(failure: SymbolRenameFailure) -> Self {
        let reason = match &failure.error {
            RenameError::NoSymbol => RenameReason::NoSymbol,
            RenameError::ImplicitReceiver => RenameReason::ImplicitReceiver,
            RenameError::InvalidName(_) => RenameReason::InvalidName,
            RenameError::Library(_) => RenameReason::LibraryOwned,
            RenameError::Exists(_) => RenameReason::NameConflict,
            RenameError::Capture { .. } => RenameReason::Capture,
        };
        Self { message: failure.error.to_string(), reason, related: failure.related }
    }
}
