//! Diagnostic operands retain their native session capabilities until report
//! publication. Recording an error must not infer another function's body just
//! to print its type while the original semantic query is still active.
use crate::{CheckReport, Checker, Diagnostic, Type, TypeQueries};
use luanalysis_syntax::Span;

#[derive(Clone, Debug)]
pub(crate) struct PendingDiagnostic {
    pub(crate) index: usize,
    template: String,
    types: Vec<Type>,
}

impl CheckReport {
    pub(crate) fn push_type_diagnostic(
        &mut self,
        code: &'static str,
        span: Span,
        template: impl Into<String>,
        types: &[&Type],
    ) {
        let template = template.into();
        assert_eq!(
            template.matches("{}").count(),
            types.len(),
            "typed diagnostic operands match their template"
        );
        let index = self.diagnostics.len();
        self.diagnostics.push(Diagnostic {
            code,
            span,
            message: template.clone(),
        });
        self.pending_diagnostics.push(PendingDiagnostic {
            index,
            template,
            types: types.iter().map(|ty| (**ty).clone()).collect(),
        });
    }

    pub(crate) fn render_diagnostics(&mut self, engine: &mut dyn TypeQueries) {
        for pending in std::mem::take(&mut self.pending_diagnostics) {
            let mut parts = pending.template.split("{}");
            let mut message = parts.next().unwrap_or_default().to_owned();
            for (ty, suffix) in pending.types.iter().zip(parts) {
                // The native printer retains named declarations, generic names,
                // mapped signatures and unions; no owned inspection erases them.
                message.push_str(&crate::describe_diagnostic_type(ty, engine));
                message.push_str(suffix);
            }
            self.diagnostics[pending.index].message = message;
        }
    }
}

pub(crate) fn named_error_description(
    error: &luanalysis_engine::NamedTypeError,
) -> (String, Vec<Type>) {
    use luanalysis_engine::NamedTypeError::*;
    match error {
        InvalidReceiver { actual, expected, .. } => ("Receiver type '{}' does not satisfy '{}'.".into(), vec![actual.clone(), expected.clone()]),
        UnsupportedReceiverType { actual } => ("Type '{}' cannot supply a named receiver.".into(), vec![actual.clone()]),
        DefinitionOwner { actual, .. } => ("Type '{}' does not match the original class definition owner.".into(), vec![actual.clone()]),
        DefinitionInitializer { actual, .. } => ("Class definition requires a table, found '{}'.".into(), vec![actual.clone()]),
        DefinitionField { error, .. } | Table { error, .. } => crate::native_tables::table_error_description(error),
        InvalidBase { base, .. } => ("Type '{}' must resolve to a class or shape record schema before it can be a base type.".into(), vec![base.clone()]),
        IncompatibleOverride { actual, expected, .. } => ("Declared member type '{}' is not assignable to the inherited member type '{}'.".into(), vec![actual.clone(), expected.clone()]),
        IncompatibleBase { actual, expected, .. } => ("Inherited member or call signatures '{}' and '{}' conflict.".into(), vec![actual.clone(), expected.clone()]),
        TypeArgumentConstraint { actual, expected, .. } => ("Type '{}' does not satisfy constraint '{}'.".into(), vec![actual.clone(), expected.clone()]),
        UnknownOwner { name, .. } => (format!("Cannot find an object owner for '{name}'."), vec![]),
        AmbiguousOwner { name, .. } => (format!("Duplicate type owner '{name}'."), vec![]),
        TypeArgumentArity { actual, minimum, maximum, .. } => (format!("Expected {minimum}..{maximum} type arguments, got {actual}."), vec![]),
        PendingContext { .. } => ("Named member ownership remains provisional.".into(), vec![]),
        InvalidDefinition(_) => ("Class definition has no valid original declaration owner.".into(), vec![]),
        InvalidDeclaration(_) => ("Invalid original named declaration.".into(), vec![]),
        InvalidMember(_) => ("Invalid original named member.".into(), vec![]),
        InvalidMemberSignatures { .. } => ("Named member has an invalid original callable signature.".into(), vec![]),
        InvalidTypeParameter(_) => ("Invalid original generic parameter.".into(), vec![]),
        CircularAlias(_) => ("Type alias circularly references itself.".into(), vec![]),
        CircularBase(_) => ("Type recursively references itself as a base type.".into(), vec![]),
        TooDeep(_) => ("Type instantiation is excessively deep.".into(), vec![]),
        UnsupportedKind(_, kind) => (format!("{kind:?} checking requires native nominal/module semantics."), vec![]),
        UnsupportedBases(_) => ("Named base checking is not implemented for this declaration.".into(), vec![]),
        UnsupportedVisibility(_) => ("Private/protected member semantics are not implemented for this declaration.".into(), vec![]),
        UnsupportedOwnerInstantiation(_) => ("Augmentation of a specialized alias requires native owner-mapper transport.".into(), vec![]),
    }
}

impl Checker<'_> {
    pub(crate) fn error_types(
        &mut self,
        code: &'static str,
        span: Span,
        template: impl Into<String>,
        types: &[&Type],
    ) {
        self.report
            .push_type_diagnostic(code, span, template, types);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luanalysis_engine::{Engine, LuaKey};

    #[test]
    fn appended_opaque_operands_keep_distinct_types_and_original_diagnostic_locations() {
        Engine::new().with_session(|queries| {
            let number = queries.regular_type(&Type::NumberLiteral(1.0));
            let string = queries.regular_type(&Type::StringLiteral(b"word".to_vec()));
            assert!(matches!(number, Type::SessionType(_)));
            assert!(matches!(string, Type::SessionType(_)));
            let span = Span { start: 7, end: 12 };
            let mut first = CheckReport::default();
            first.push_type_diagnostic("TS2345", span, "Argument type '{}'.", &[&number]);
            let mut second = CheckReport::default();
            second.push_type_diagnostic("TS2345", span, "Argument type '{}'.", &[&string]);
            // Before final publication these errors have intentionally identical
            // templates. Their native operands must not be merged or lost.
            assert_eq!(first.diagnostics[0], second.diagnostics[0]);
            first.append(second);
            first.render_diagnostics(queries);
            assert_eq!(first.diagnostics.len(), 2);
            assert!(first.diagnostics[0].message.contains("1"));
            assert!(first.diagnostics[1].message.contains("word"));
            assert!(
                first
                    .diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.span == span && diagnostic.code == "TS2345")
            );
            assert!(first.pending_diagnostics.is_empty());
        });
    }

    #[test]
    fn literal_braces_are_rendered_as_key_data_and_large_integer_keys_stay_exact() {
        Engine::new().with_session(|queries| {
            let mut report = CheckReport::default();
            for key in [
                LuaKey::String(b"{}".to_vec()),
                LuaKey::Integer(9_007_199_254_740_993),
            ] {
                let error = luanalysis_engine::TableError::MissingKey {
                    key: luanalysis_engine::TableKey::Literal(key),
                };
                let (template, types) = crate::native_tables::table_error_description(&error);
                report.push_type_diagnostic(
                    "TS2339",
                    Span { start: 0, end: 1 },
                    template,
                    &types.iter().collect::<Vec<_>>(),
                );
            }
            report.render_diagnostics(queries);
            assert!(report.diagnostics[0].message.contains("\"{}\""));
            assert!(report.diagnostics[1].message.contains("9007199254740993"));
        });
    }
}
