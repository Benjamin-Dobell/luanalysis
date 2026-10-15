//! Diagnostic operands retain their native session capabilities until report
//! publication. Recording an error must not infer another function's body just
//! to print its type while the original semantic query is still active.
use crate::{CheckReport, Checker, Diagnostic, Type, TypeQueries};
use luanalysis_engine::ValuePack;
use luanalysis_syntax::Span;

#[derive(Clone, Debug)]
enum DiagnosticOperand {
    Type(Type),
    Pack(ValuePack),
}

#[derive(Clone, Debug)]
pub(crate) struct PendingDiagnostic {
    pub(crate) index: usize,
    template: String,
    operands: Vec<DiagnosticOperand>,
    note: String,
}

impl CheckReport {
    pub(crate) fn push_type_diagnostic(
        &mut self,
        code: &'static str,
        span: Span,
        template: impl Into<String>,
        types: &[&Type],
    ) {
        self.push_diagnostic_operands(
            code,
            span,
            template.into(),
            types
                .iter()
                .map(|ty| DiagnosticOperand::Type((**ty).clone()))
                .collect(),
        );
    }

    pub(crate) fn push_pack_diagnostic(
        &mut self,
        code: &'static str,
        span: Span,
        template: impl Into<String>,
        packs: &[&ValuePack],
    ) {
        self.push_diagnostic_operands(
            code,
            span,
            template.into(),
            packs
                .iter()
                .map(|pack| DiagnosticOperand::Pack((**pack).clone()))
                .collect(),
        );
    }

    fn push_diagnostic_operands(
        &mut self,
        code: &'static str,
        span: Span,
        template: String,
        operands: Vec<DiagnosticOperand>,
    ) {
        assert_eq!(
            template.matches("{}").count(),
            operands.len(),
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
            operands,
            note: String::new(),
        });
    }

    pub(crate) fn append_diagnostic_note(&mut self, index: usize, note: &str) {
        self.diagnostics[index].message.push_str(note);
        if let Some(pending) = self
            .pending_diagnostics
            .iter_mut()
            .find(|pending| pending.index == index)
        {
            // Notes are text, not a template: braces in an explanation must
            // never become extra native type-operand slots.
            pending.note.push_str(note);
        }
    }

    pub(crate) fn render_diagnostics(&mut self, engine: &mut dyn TypeQueries) {
        for pending in std::mem::take(&mut self.pending_diagnostics) {
            let mut parts = pending.template.split("{}");
            let mut message = parts.next().unwrap_or_default().to_owned();
            for (operand, suffix) in pending.operands.iter().zip(parts) {
                // The native printer retains named declarations, generic names,
                // mapped signatures and unions; no owned inspection erases them.
                let displayed = match operand {
                    DiagnosticOperand::Type(ty) => crate::describe_diagnostic_type(ty, engine),
                    DiagnosticOperand::Pack(pack) => {
                        crate::describe_native_pack(pack, engine, &mut Vec::new(), true)
                            .unwrap_or_else(|| "<uninstantiated variadic result pack>".into())
                    }
                };
                message.push_str(&displayed);
                message.push_str(suffix);
            }
            message.push_str(&pending.note);
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
    pub(crate) fn error_packs(
        &mut self,
        code: &'static str,
        span: Span,
        template: impl Into<String>,
        packs: &[&ValuePack],
    ) {
        self.report
            .push_pack_diagnostic(code, span, template, packs);
    }

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
    fn diagnostic_notes_survive_report_append_and_late_native_rendering() {
        Engine::new().with_session(|queries| {
            let span = Span { start: 3, end: 7 };
            let mut report = CheckReport::default();
            report.diagnostics.push(Diagnostic {
                code: "LUA1001",
                span,
                message: "Existing error.".into(),
            });
            let mut staged = CheckReport::default();
            staged.push_type_diagnostic("TS2345", span, "Expected '{}'.", &[&Type::Number]);
            staged.append_diagnostic_note(0, " Note with literal {}.");
            report.append(staged);
            report.render_diagnostics(queries);
            assert_eq!(
                report.diagnostics[1].message,
                "Expected 'number'. Note with literal {}."
            );
            assert_eq!(report.diagnostics[1].code, "TS2345");
            assert_eq!(report.diagnostics[1].span, span);
            report.append_diagnostic_note(1, " Published note.");
            report.render_diagnostics(queries);
            assert_eq!(
                report.diagnostics[1].message,
                "Expected 'number'. Note with literal {}. Published note."
            );
        });
    }

    #[test]
    fn retained_pack_operands_preserve_correlated_rows_optional_slots_and_variadic_tails() {
        use luanalysis_engine::{PackAlternative, PackSlot};
        Engine::new().with_session(|queries| {
            let actual = queries.retain_pack(&ValuePack::Alternatives(vec![
                PackAlternative {
                    fixed: vec![
                        PackSlot::required(Type::Number),
                        PackSlot::required(Type::String),
                    ],
                    tail: None,
                },
                PackAlternative {
                    fixed: vec![
                        PackSlot::required(Type::Nil),
                        PackSlot::required(Type::Boolean),
                    ],
                    tail: Some(Box::new(Type::String)),
                },
            ]));
            let expected = queries.retain_pack(&ValuePack::Alternatives(vec![PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::Number),
                    PackSlot {
                        ty: Type::String,
                        optional: true,
                    },
                ],
                tail: None,
            }]));
            assert!(matches!(actual, ValuePack::Session(_)));
            let mut report = CheckReport::default();
            report.push_pack_diagnostic(
                "TS2322",
                Span { start: 3, end: 8 },
                "Actual: {}. Expected: {}.",
                &[&actual, &expected],
            );
            report.render_diagnostics(queries);
            let message = &report.diagnostics[0].message;
            assert!(message.contains("(number, string)"), "{message}");
            assert!(message.contains("(nil, boolean, string...)"), "{message}");
            assert!(message.contains(" | "), "{message}");
            let expected_text = message.split("Expected: ").nth(1).unwrap();
            assert!(
                expected_text.contains("number")
                    && expected_text.contains("string")
                    && expected_text.contains('?'),
                "{message}"
            );
            assert!(
                !message.contains("undefined") && !message.contains("SessionPack"),
                "{message}"
            );
        });
    }

    #[test]
    fn result_pack_diagnostics_keep_zero_values_distinct_from_one_nil_value() {
        Engine::new().with_session(|queries| {
            let mut report = CheckReport::default();
            report.push_pack_diagnostic(
                "TS2322",
                Span { start: 0, end: 1 },
                "Actual: {}. Expected: {}.",
                &[&ValuePack::empty(), &ValuePack::single(Type::Nil)],
            );
            report.render_diagnostics(queries);
            assert_eq!(
                report.diagnostics[0].message,
                "Actual: void. Expected: nil."
            );
        });
    }

    #[test]
    fn printing_a_named_operand_does_not_demand_its_unrelated_member_bodies() {
        use luanalysis_engine::{
            NativeFunctionKey, NativeTypeDeclarationKey, NativeTypeProvider, ObjectDeclarationForm,
            Parameter, TypeDeclarationHeader, TypeDeclarationKind,
        };
        const DECLARATION: NativeTypeDeclarationKey = NativeTypeDeclarationKey {
            snapshot: 91,
            index: 0,
        };
        struct NameOnly;
        impl NativeTypeProvider for NameOnly {
            fn contains_function(&self, _: NativeFunctionKey) -> bool {
                false
            }
            fn function_parameters(
                &self,
                _: NativeFunctionKey,
                _: &mut dyn TypeQueries,
            ) -> Vec<Parameter> {
                panic!("diagnostic printing must not query a body")
            }
            fn function_declared_return(
                &self,
                _: NativeFunctionKey,
                _: &mut dyn TypeQueries,
            ) -> Option<Type> {
                panic!("diagnostic printing must not query a body")
            }
            fn function_inferred_return(
                &self,
                _: NativeFunctionKey,
                _: &mut dyn TypeQueries,
            ) -> Type {
                panic!("diagnostic printing must not infer a body")
            }
            fn return_cycle(&self, _: NativeFunctionKey) {
                panic!("diagnostic printing must not create a return cycle")
            }
            fn return_type_too_deep(&self, _: NativeFunctionKey) {
                panic!("diagnostic printing must not create a return query")
            }
            fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
                key == DECLARATION
            }
            fn type_declaration_header(
                &self,
                _: NativeTypeDeclarationKey,
            ) -> TypeDeclarationHeader {
                TypeDeclarationHeader {
                    kind: TypeDeclarationKind::Class,
                    name: "NameOnly".into(),
                    object_form: Some(ObjectDeclarationForm::Record),
                    parameters: vec![],
                    outer_parameters: vec![],
                }
            }
            fn type_declaration_members(
                &self,
                _: NativeTypeDeclarationKey,
            ) -> Vec<luanalysis_engine::NativeTypeMemberKey> {
                panic!("printing a named identity must not materialize its member schema")
            }
        }
        Engine::new().with_native_provider(
            || NameOnly,
            |_, queries| {
                let named = queries.named_type_reference(DECLARATION, &[]).unwrap();
                let mut report = CheckReport::default();
                report.push_type_diagnostic(
                    "TS2345",
                    Span { start: 4, end: 5 },
                    "Expected '{}'.",
                    &[&named],
                );
                report.render_diagnostics(queries);
                assert_eq!(report.diagnostics[0].message, "Expected 'NameOnly'.");
            },
        );
    }

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
