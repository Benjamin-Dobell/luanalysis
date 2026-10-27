//! `keyof T`, `T[K]` and `typeof value` in annotations.

use luanalysis_engine::{GlobalQueryError, Type};
use luanalysis_syntax::{Expr, ExprKind, Name, TypeExpr, TypeExprKind, TypeLiteral};

impl super::Checker<'_> {
    pub(super) fn indexed_access_annotation(&mut self, object: &TypeExpr, index: &TypeExpr) -> Type {
        let object_type = self.annotation(object);
        let index_type = self.annotation(index);
        if let Some(value) = self.engine.indexed_access_type(&object_type, &index_type) {
            return value;
        }
        // As TypeScript reports it: a missing name as a missing property, another key as an
        // invalid index.
        if let TypeExprKind::Literal(TypeLiteral::String(name)) = &index.value_syntax().kind {
            self.error_types(
                "TS2339", index.span,
                format!("Property '{}' does not exist on type '{{}}'.", String::from_utf8_lossy(name)),
                &[&object_type],
            );
        } else {
            self.error_types("TS2536", index.span, "Type '{}' cannot be used to index type '{}'.", &[&index_type, &object_type]);
        }
        self.engine.error_type()
    }

    /// The declared type of the value `path` names where it is written: the local in scope
    /// there or else the global, then each member of it. As in TypeScript, this is the
    /// declared type, not one narrowed where the annotation is.
    pub(super) fn type_query_annotation(&mut self, path: &[Name]) -> Type {
        let root = &path[0];
        let Some(mut ty) = self.declared_value_type(root) else {
            self.error("TS2304", root.span, format!("Cannot find name '{}'.", root.text));
            return self.engine.error_type();
        };
        // Editors show each name's type where it is written, as at a read.
        self.record_expression_type(root.span, &ty);
        let mut receiver_span = root.span;
        for member in &path[1..] {
            ty = self.native_named_property_read(&ty, member, receiver_span);
            self.record_expression_type(member.span, &ty);
            receiver_span = receiver_span.join(member.span);
        }
        ty
    }

    /// The declared type of a name or a member path of one (`M.f`), as `typeof` gives it,
    /// without reporting anything. None: another expression, or a name that is not declared.
    pub(super) fn declared_expression_type(&mut self, expression: &Expr) -> Option<Type> {
        match &expression.kind {
            ExprKind::Name(name) => self.declared_value_type(name),
            ExprKind::Field { base, name } => {
                let receiver = self.declared_expression_type(base)?;
                self.engine.property_type(&receiver, &name.text)
            }
            ExprKind::Parenthesized(inner) => self.declared_expression_type(inner),
            _ => None,
        }
    }

    fn declared_value_type(&mut self, name: &Name) -> Option<Type> {
        // The binder resolves the name as editors do. An `---@enum`'s names the variable
        // where its statement writes it, which that occurrence resolves.
        let binding = match self.bound.occurrence_at(name.span.start) {
            Some(occurrence) if occurrence.span == name.span => occurrence.binding,
            _ => self.bound.visible_binding(&name.text, name.span.start),
        };
        if let Some(binding) = binding {
            let declared = self.resolve_local(binding).map(|binding| binding.constraint.clone())?;
            // A local declared without a value or type has a type only where it is read.
            if self.engine.is_auto_variable_constraint(&declared) {
                return Some(Type::Any);
            }
            // A table has every member its stores give it, wherever they are.
            return Some(self.engine.runtime_table_schema(&declared).unwrap_or(declared));
        }
        let native = self.native?;
        if let Some(binding) = native.global_binding(&name.text) {
            let declared = match self.engine.global_binding_type(binding) {
                Ok(declared) => declared,
                // `---@type typeof G` on `G` itself.
                Err(GlobalQueryError::CircularQuery(_) | GlobalQueryError::TooDeep(_)) => {
                    self.error("TS2502", name.span, format!("'{}' is referenced directly or indirectly in its own type annotation.", name.text));
                    return Some(self.engine.error_type());
                }
                // Other failures are reported at the global's declarations.
                Err(_) => return Some(self.engine.error_type()),
            };
            return Some(self.engine.runtime_table_schema(&declared).unwrap_or(declared));
        }
        native.legacy_builtin_value(name)
    }
}
