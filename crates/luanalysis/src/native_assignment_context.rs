//! Contextual guidance from immutable declaration paths. This never evaluates
//! an assignment receiver/key early or changes the frozen address transaction.
use crate::Checker;
use luanalysis_engine::{LuaKey, TableKey, Type};
use luanalysis_syntax::{Expr, ExprKind};

impl Checker<'_> {
    pub(crate) fn assignment_target_context(&mut self, target: &Expr) -> Option<Type> {
        if let Some(native) = self.native
            && let Some(written) = native.original_assignment_annotation(target.span, self.engine)
        { return Some(written); }
        let context = self.assignment_declaration_path(target)?;
        let mut root = target;
        while let ExprKind::Parenthesized(inner) = &root.kind {
            root = inner;
        }
        // Only replacing the binding creates a distinct allocation. Member
        // stores still use written field contracts on its original table.
        if matches!(root.kind, ExprKind::Name(_))
            && self.engine.runtime_table_schema(&context).is_some()
        {
            None
        } else {
            Some(context)
        }
    }

    fn assignment_declaration_path(&mut self, expression: &Expr) -> Option<Type> {
        match &expression.kind {
            ExprKind::Name(name) => {
                if let Some(binding) = self.bound.binding_at(name.span.start) {
                    self.resolve_local(binding)
                        .filter(|binding| !binding.evolves_from_nil)
                        .map(|binding| binding.constraint.clone())
                } else {
                    if name.text == "_ENV" { return Some(Type::Unknown); }
                    self.global_read(name)
                }
            },
            ExprKind::Field { base, name } => {
                let receiver = self.assignment_declaration_path(base)?;
                self.engine.table_value_context(
                    &receiver,
                    &TableKey::Literal(LuaKey::String(name.text.as_bytes().to_vec())),
                )
            }
            ExprKind::Index { base, index } => {
                let key = crate::native_tables::literal_key(index)?;
                let receiver = self.assignment_declaration_path(base)?;
                self.engine
                    .table_value_context(&receiver, &TableKey::Literal(key))
            }
            ExprKind::Parenthesized(inner) => self.assignment_declaration_path(inner),
            _ => None,
        }
    }
}
