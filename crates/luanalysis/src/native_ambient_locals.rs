//! Original declaration-library local owners and terminal export statements.
//! Empty tables identify declaration owners; no runtime value is constructed.
use super::NativeRegistry;
use crate::native_named::{NativeNamedKey, RuntimeAttachmentKind};
use luanalysis_engine::{NativeTypeDeclarationKey, TypeQueries};
use luanalysis_project::SourceRole;
use luanalysis_syntax::{
    Expr, ExprKind, NamedRecordKind, NamedTypeDeclarationKind, Span, StmtKind,
};

impl NativeRegistry {
    fn ambient_local_owner(&self, span: Span) -> Option<NativeNamedKey> {
        if self.source_role() != SourceRole::Declaration {
            return None;
        }
        let attachment = self.named_metadata().attachment(span)?;
        let RuntimeAttachmentKind::Local { bindings } = &attachment.kind else {
            return None;
        };
        if bindings.len() != 1 || attachment.has_explicit_type_annotation {
            return None;
        }
        let [key] = attachment.declarations.as_slice() else {
            return None;
        };
        let source = self.named_metadata().declaration(*key)?;
        if !matches!(
            source.syntax.kind,
            NamedTypeDeclarationKind::Record {
                flavor: NamedRecordKind::Class,
                ..
            }
        ) || self
            .named_metadata()
            .candidates(&source.syntax.name.text)
            .len()
            != 1
        {
            return None;
        }
        let statement = self
            .statements()
            .iter()
            .find(|statement| statement.span == span)?;
        let StmtKind::Local { values, .. } = &statement.kind else {
            return None;
        };
        let [value] = values.as_slice() else {
            return None;
        };
        let mut value = value;
        while let ExprKind::Parenthesized(inner) = &value.kind {
            value = inner;
        }
        matches!(&value.kind, ExprKind::Table(fields) if fields.is_empty()).then_some(*key)
    }

    pub(super) fn is_ambient_local_attachment(&self, span: Span) -> bool {
        self.ambient_local_owner(span).is_some()
    }

    fn check_ambient_export_value(&self, value: &Expr, queries: &mut dyn TypeQueries) -> bool {
        let value = match &value.kind {
            ExprKind::Parenthesized(inner) => {
                return self.check_ambient_export_value(inner, queries);
            }
            ExprKind::Name(name) => name,
            _ => return false,
        };
        if let Some(binding) = self.bound().binding_at(value.span.start) {
            let owner = self
                .named_metadata()
                .declarations()
                .iter()
                .find_map(|source| {
                    let span = source.syntax.attachment?;
                    let owner = self.ambient_local_owner(span)?;
                    let RuntimeAttachmentKind::Local { bindings } =
                        &self.named_metadata().attachment(span)?.kind
                    else {
                        return None;
                    };
                    bindings.contains(&binding).then_some(owner)
                });
            let Some(owner) = owner else { return false };
            let _ = queries.named_declared_type(NativeTypeDeclarationKey {
                snapshot: owner.snapshot,
                index: owner.index,
            });
            return true;
        }
        let Some(binding) = self.global_binding(&value.text) else {
            return false;
        };
        let _ = queries.global_binding_type(binding);
        true
    }

    /// Returns true only for a recognized original declaration recipe. All
    /// other executable statements retain the declaration-library diagnostic.
    pub(super) fn check_ambient_local_statement(
        &self,
        span: Span,
        queries: &mut dyn TypeQueries,
    ) -> bool {
        if self.source_role() != SourceRole::Declaration {
            return false;
        }
        if let Some(owner) = self.ambient_local_owner(span) {
            let _ = queries.named_declared_type(NativeTypeDeclarationKey {
                snapshot: owner.snapshot,
                index: owner.index,
            });
            return true;
        }
        let Some(statement) = self.statements().iter().find(|statement| {
            statement.span == span || matches!(&statement.kind, StmtKind::FunctionDeclaration { target, .. } if target.span == span)
        }) else { return false };
        match &statement.kind {
            StmtKind::FunctionDeclaration { function, .. } => {
                let key = self.key_for(function);
                let Some(method) = self.methods.get(key) else {
                    return false;
                };
                let Some(owner) = method.class_owner else {
                    return false;
                };
                let Some(attachment) = self
                    .named_metadata()
                    .declaration(owner)
                    .and_then(|source| source.syntax.attachment)
                else {
                    return false;
                };
                if self.ambient_local_owner(attachment) != Some(owner) {
                    return false;
                }
                let _ = self.ambient_function_contract(key, queries);
                true
            }
            StmtKind::Return(values) => {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|value| self.check_ambient_export_value(value, queries))
            }
            _ => false,
        }
    }
}
