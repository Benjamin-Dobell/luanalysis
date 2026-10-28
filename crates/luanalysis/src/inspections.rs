//! Inspections: what an editor shows beyond errors (`ProjectCheckOptions::inspections`). Like
//! tsgo's suggestion diagnostics they are reported only when asked for, and none is an error
//! (`Diagnostic::category`).
use crate::bind::{DeclarationKind, OccurrenceKind};
use crate::native_functions::NativeRegistry;
use crate::{CheckReport, Checker, Diagnostic};
use luanalysis_engine::{PackInspection, Type, ValuePack};
use luanalysis_syntax::{Expr, ExprKind, Span, Stmt};

impl Checker<'_> {
    fn inspecting(&self) -> bool {
        !self.incomplete_flow && self.native.is_some_and(NativeRegistry::inspections)
    }

    /// Lua adjusts a completed RHS list to the destination count. Inspect only its already
    /// computed physical rows: uncertainty about a tail or optional result is not evidence
    /// that a value is discarded or a destination is always padded.
    pub(crate) fn inspect_assignment_result_count(&mut self, expressions: &[Expr], destinations: &[Span], pack: &ValuePack) {
        if !self.inspecting() || expressions.is_empty() || destinations.is_empty()
            || !crate::native_expressions::publishes(self.check_mode | self.engine.native_type_query_mode())
        { return; }
        let PackInspection::Alternatives(mut rows) = self.engine.inspect_pack_view(pack) else { return };
        // A scalar never in a required prefix prevents the list from completing,
        // even when concatenation retains that impossible row structurally.
        rows.retain(|row| !row.fixed.iter().any(|slot| !slot.optional && matches!(&slot.ty, Type::Never)));
        if rows.is_empty() { return; }
        let minimum = rows.iter().map(|row| row.fixed.iter().filter(|slot| !slot.optional).count()).min().unwrap();
        let maximum = rows.iter().try_fold(0usize, |maximum, row| row.tail.is_none().then_some(maximum.max(row.fixed.len())));
        if minimum > destinations.len() {
            let discarded = minimum - destinations.len();
            let origin = expressions.get(destinations.len()).unwrap_or_else(|| expressions.last().unwrap()).span;
            let message = if discarded == 1 { "This assignment discards at least one result.".to_owned() }
                else { format!("This assignment discards at least {discarded} results.") };
            self.error("LUA4002", origin, message);
        }
        if let Some(maximum) = maximum {
            for &destination in destinations.iter().skip(maximum) {
                self.error("LUA4003", destination, "This destination has no corresponding result; Lua pads it with nil.");
            }
        }
    }

    /// A condition whose type is never nil or false always takes its branch; one that is always
    /// nil or false never does. A literal `true`, `false` or `nil` is written so on purpose.
    pub(crate) fn inspect_condition(&mut self, condition: &Expr, ty: &Type) {
        if !self.inspecting() || matches!(ty, Type::Never) { return; }
        let mut written = condition;
        while let ExprKind::Parenthesized(inner) = &written.kind { written = inner; }
        if matches!(written.kind, ExprKind::Boolean(_) | ExprKind::Nil) { return; }
        let (truthy, falsy) = self.lua_truthiness(ty);
        if matches!(falsy, Type::Never) {
            self.error_types("LUA2872", condition.span, "This condition is always true: its type '{}' is never nil or false.", &[ty]);
        } else if matches!(truthy, Type::Never) {
            self.error_types("LUA2873", condition.span, "This condition is always false: its type '{}' is only nil or false.", &[ty]);
        }
    }

    /// Lua's `==` never equates values of different types, so operands whose types do not
    /// overlap are never equal (TS2367). As in TypeScript, comparing with nil is always allowed.
    pub(crate) fn inspect_comparison(&mut self, left: &Expr, right: &Expr, left_type: &Type, right_type: &Type) {
        if !self.inspecting() { return; }
        // As in TypeScript, a written literal is compared as its literal type.
        let (left_type, right_type) = (&literal_type(left).unwrap_or_else(|| left_type.clone()), &literal_type(right).unwrap_or_else(|| right_type.clone()));
        if [left_type, right_type].into_iter().any(|ty| matches!(ty, Type::Never) || self.engine.contains_nil(ty)) { return; }
        if self.engine.types_comparable(left_type, right_type) { return; }
        self.error_types(
            "TS2367",
            Span { start: left.span.start, end: right.span.end },
            "This comparison appears to be unintentional because the types '{}' and '{}' have no overlap.",
            &[left_type, right_type],
        );
    }

    /// The statements after one that never completes normally (TS7027).
    pub(crate) fn report_unreachable(&mut self, rest: &[Stmt]) {
        if !self.inspecting() { return; }
        // A label is reached by the `goto`s naming it.
        let rest = &rest[..rest.iter().position(|statement| matches!(statement.kind, luanalysis_syntax::StmtKind::Label(_))).unwrap_or(rest.len())];
        let (Some(first), Some(last)) = (rest.first(), rest.last()) else { return };
        self.error("TS7027", Span { start: first.span.start, end: last.span.end }, "Unreachable code detected.");
    }

    /// A parameter without a written or contextual type is `any`. Under `--no-implicit-any` that
    /// is an error (`code`); otherwise it is the suggestion TypeScript makes with `noImplicitAny`
    /// off: TS7044 for a parameter, TS7047 for a rest parameter. An implicit receiver and a
    /// documented function type's parameter (`name` empty) have no suggestion.
    pub(crate) fn implicit_parameter_any(&mut self, code: &'static str, span: Span, name: &str, message: impl Into<String>) {
        let Some(native) = self.native else { return };
        if native.no_implicit_any() {
            self.error(code, span, message);
            return;
        }
        if !self.inspecting() || name.is_empty() || name == "self" { return; }
        match code {
            "TS7006" => self.error("TS7044", span, format!("Parameter '{name}' implicitly has an 'any' type, but a better type may be inferred from usage.")),
            "TS7019" => self.error("TS7047", span, format!("Rest parameter '{name}' implicitly has an 'any[]' type, but a better type may be inferred from usage.")),
            _ => {}
        }
    }
}

impl NativeRegistry {
    /// The binder records the earlier active local before installing each new declaration,
    /// including declarations nested in initializer functions and loop headers.
    pub(crate) fn report_shadowed_declarations(&self) {
        if !self.inspections() || self.source_role() != luanalysis_project::SourceRole::Implementation { return; }
        let bound = self.bound();
        let diagnostics = bound.declarations.iter()
            .filter(|declaration| declaration.name != "_" && declaration.kind != DeclarationKind::ImplicitReceiver)
            .filter(|declaration| bound.shadowed_binding(declaration.id).is_some())
            .map(|declaration| Diagnostic {
                code: "LUA4001",
                span: declaration.span,
                message: format!("Local name '{}' shadows a previously declared local.", declaration.name),
            })
            .collect();
        self.merge_report(CheckReport { diagnostics, ..CheckReport::default() });
    }

    /// A function's `---@generic` parameter named as one of an enclosing function's, or of the
    /// class its method is stored on (`---@generic T` on a method of `Box<T>`), is another
    /// parameter that hides it, as v1 reported (LUA4005). TypeScript permits it, so it is a warning.
    pub(crate) fn report_shadowed_generic_parameters(&self) {
        if !self.inspections() || self.source_role() != luanalysis_project::SourceRole::Implementation { return; }
        let functions: Vec<_> = self.functions_with_method_classes().collect();
        let mut diagnostics = Vec::new();
        for &(function, class) in &functions {
            for parameter in &function.annotations.generics {
                let enclosing = functions.iter().any(|(outer, _)| outer.span.start < function.span.start && function.span.end <= outer.span.end
                    && outer.annotations.generics.iter().any(|other| other.name.text == parameter.name.text));
                let owner = class.and_then(|class| self.named_metadata().declaration(class))
                    .is_some_and(|class| class.syntax.generics.iter().any(|other| other.name.text == parameter.name.text));
                if enclosing || owner {
                    diagnostics.push(Diagnostic {
                        code: "LUA4005",
                        span: parameter.span,
                        message: format!("Generic parameter '{}' shadows a generic parameter of the same name in an enclosing scope.", parameter.name.text),
                    });
                }
            }
        }
        self.merge_report(CheckReport { diagnostics, ..CheckReport::default() });
    }

    /// `---@field readonly T` and `{ readonly: T }` name a field `readonly`, as they always
    /// have, but read like a read-only field missing its name. `["readonly"]` names the field
    /// without the warning.
    pub(crate) fn report_reserved_field_names(&self) {
        use luanalysis_syntax::{NamedTypeDeclarationKind, TypeExprKind, TypeFieldKey};
        if !self.inspections() { return; }
        let reserved = |readonly: Option<Span>, key: &TypeFieldKey| match key {
            TypeFieldKey::Named(name) if readonly.is_none() && name.text == "readonly" => Some(name.span),
            _ => None,
        };
        let syntax = self.syntax();
        let declared = syntax.type_declarations.iter().filter_map(|declaration| match &declaration.kind {
            NamedTypeDeclarationKind::Record { fields, .. } => Some(fields.as_slice()),
            _ => None,
        }).chain(syntax.type_extensions.iter().map(|extension| extension.fields.as_slice()))
            .flatten()
            .filter_map(|field| reserved(field.readonly, &field.key));
        let inline = self.anonymous_heads().iter().filter_map(|head| match &head.syntax.kind {
            TypeExprKind::Record(fields) => Some(fields),
            _ => None,
        }).flatten().filter_map(|field| reserved(field.readonly, &field.key));
        let diagnostics = declared.chain(inline).map(|span| Diagnostic {
            code: "LUA4004",
            span,
            message: "'readonly' is a reserved word and may cause confusion: this declares a writable field named 'readonly'. Write [\"readonly\"] to name it explicitly.".to_owned(),
        }).collect();
        self.merge_report(CheckReport { diagnostics, ..CheckReport::default() });
    }

    /// Each local, local function, loop variable and parameter whose value is never read
    /// (TS6133). A name starting with `_` declares that it is unused.
    pub(crate) fn report_unused_declarations(&self) {
        if !self.inspections() || self.source_role() != luanalysis_project::SourceRole::Implementation { return; }
        let bound = self.bound();
        let mut read = vec![false; bound.declarations.len()];
        for occurrence in &bound.occurrences {
            if matches!(occurrence.kind, OccurrenceKind::Read | OccurrenceKind::TypeQuery) && let Some(binding) = occurrence.binding {
                read[binding.0] = true;
            }
        }
        let diagnostics = bound.declarations.iter()
            .filter(|declaration| !read[declaration.id.0] && !declaration.name.starts_with('_'))
            .filter(|declaration| declaration.kind != DeclarationKind::ImplicitReceiver && declaration.name != "self")
            .map(|declaration| Diagnostic {
                code: "TS6133",
                span: declaration.span,
                message: format!("'{}' is declared but its value is never read.", declaration.name),
            })
            .collect();
        self.merge_report(CheckReport { diagnostics, ..CheckReport::default() });
    }
}

fn literal_type(expression: &Expr) -> Option<Type> {
    let mut written = expression;
    while let ExprKind::Parenthesized(inner) = &written.kind { written = inner; }
    match &written.kind {
        ExprKind::String(bytes) => Some(Type::StringLiteral(bytes.clone())),
        ExprKind::Boolean(value) => Some(Type::BooleanLiteral(*value)),
        _ => match crate::flow::numeric_literal(written)? {
            crate::flow::NumericLiteral::Integer(value) => Some(Type::NumberLiteral(value as f64)),
            crate::flow::NumericLiteral::Float(value) => Some(Type::NumberLiteral(value)),
        },
    }
}
