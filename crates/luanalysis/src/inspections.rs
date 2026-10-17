//! Inspections: what an editor shows beyond errors (`ProjectCheckOptions::inspections`). Like
//! tsgo's suggestion diagnostics they are reported only when asked for, and none is an error
//! (`Diagnostic::category`).
use crate::bind::{DeclarationKind, OccurrenceKind};
use crate::native_functions::NativeRegistry;
use crate::{CheckReport, Checker, Diagnostic};
use luanalysis_engine::Type;
use luanalysis_syntax::{Expr, ExprKind, Span, Stmt};

impl Checker<'_> {
    fn inspecting(&self) -> bool {
        !self.incomplete_flow && self.native.is_some_and(NativeRegistry::inspections)
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
        if [left_type, right_type].into_iter().any(|ty| matches!(ty, Type::Never) || self.engine.is_assignable(&Type::Nil, ty)) { return; }
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
    /// Each local, local function, loop variable and parameter whose value is never read
    /// (TS6133). A name starting with `_` declares that it is unused.
    pub(crate) fn report_unused_declarations(&self) {
        if !self.inspections() || self.source_role() != luanalysis_project::SourceRole::Implementation { return; }
        let bound = self.bound();
        let mut read = vec![false; bound.declarations.len()];
        for occurrence in &bound.occurrences {
            if occurrence.kind == OccurrenceKind::Read && let Some(binding) = occurrence.binding {
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
