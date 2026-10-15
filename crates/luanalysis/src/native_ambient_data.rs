//! Finite declaration syntax used by the original Luanalysis libraries.
//!
//! Literal initializers declare mutable primitive domains; an empty table
//! declares the broad Lua table domain. This is deliberately not an expression
//! checker: calls, names, operators and nonempty constructors require a written
//! contract and cannot execute or infer a placeholder declaration body.
use luanalysis_engine::Type;
use luanalysis_syntax::{Expr, ExprKind, UnaryOperator};

pub(super) fn literal_contract(expression: &Expr) -> Option<Type> {
    match &expression.kind {
        ExprKind::Nil => Some(Type::Nil),
        ExprKind::Boolean(_) => Some(Type::Boolean),
        ExprKind::Number(_) => Some(Type::Number),
        ExprKind::String(_) => Some(Type::String),
        ExprKind::Table(fields) if fields.is_empty() => Some(Type::RuntimeTable),
        ExprKind::Parenthesized(inner) => literal_contract(inner),
        ExprKind::Unary {
            op: UnaryOperator::Negate,
            value,
            ..
        } if matches!(value.kind, ExprKind::Number(_)) => Some(Type::Number),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::native_functions::NativeRegistry;
    use luanalysis_engine::{Engine, GlobalValueKind, NativeTypeProvider};
    use luanalysis_project::SourceRole;

    #[test]
    fn data_member_keys_retain_the_original_value_and_initializer_without_a_function_identity() {
        let source = "---@class Settings\nsettings = {}\nsettings.path = ''\n---@return number\nfunction settings.read() end\n";
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        Engine::new().with_native_provider(
            move || {
                let mut registry = NativeRegistry::new(file, bound);
                registry.set_source_role(SourceRole::Declaration);
                registry
            },
            |registry, queries| {
                let data = registry
                    .globals
                    .declarations
                    .iter()
                    .find(|declaration| declaration.name.text == "path")
                    .unwrap();
                let function = registry
                    .globals
                    .declarations
                    .iter()
                    .find(|declaration| declaration.name.text == "read")
                    .unwrap();
                let member = data.namespace.as_ref().unwrap().member;
                assert!(data.function.is_none());
                assert_eq!(
                    registry.global_header(data.key).kind,
                    GlobalValueKind::Member
                );
                assert_ne!(data.key, function.key);
                assert_ne!(member, function.namespace.as_ref().unwrap().member);
                assert!(registry.contains_type_member(member));
                let expression = &registry
                    .expressions
                    .get(data.initializer.unwrap())
                    .expression;
                assert_eq!(&source[expression.span.start..expression.span.end], "''");
                let ty = queries.value_declaration_type(data.key).unwrap();
                assert_eq!(queries.format(&ty), "string");
            },
        );
    }
}
