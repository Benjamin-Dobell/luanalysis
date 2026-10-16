//! The original written view used by call reachability effects. Runtime
//! intrinsic identity is a separate native value-origin query.
use super::{AnnotationPack, DeclarationSource, NativeRegistry, Type, TypeQueries};
use luanalysis_engine::{TableKey, NativeTypeProvider};
use luanalysis_syntax::{Expr, ExprKind, ExpressionAssertionKind};

impl NativeRegistry {
    pub(super) fn original_effect_call_contract(
        &self,
        expression: &Expr,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        match &expression.kind {
            ExprKind::Call { callee, .. } => self.original_effect_expression_contract(callee, queries),
            ExprKind::MethodCall { receiver, name, .. } => {
                let receiver = self.original_effect_expression_contract(receiver, queries)?;
                queries.property_type(&receiver, &name.text)
            }
            _ => None,
        }
    }

    fn original_effect_expression_contract(
        &self,
        expression: &Expr,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        if let Some(assertion) = self.expression_assertions(expression.span).into_iter()
            .find(|assertion| assertion.kind == ExpressionAssertionKind::Type)
        {
            let mut checker = self.checker(queries);
            checker.type_context = self.expressions.get(self.expressions.key_for_span(expression.span)).owner;
            let pack = AnnotationPack::annotation(&assertion.annotation).lower(&mut |ty| checker.annotation(ty));
            return Some(checker.engine.pack_slot(&pack, 0));
        }
        match &expression.kind {
            ExprKind::Parenthesized(inner) => self.original_effect_expression_contract(inner, queries),
            ExprKind::Name(name) => {
                if let Some(binding) = self.bound.binding_at(name.span.start) {
                    if let Some(DeclarationSource::Function(index)) = self.declarations.get(&binding) {
                        let function = self.key(*index);
                        // An explicit result contract, including public heads,
                        // is sufficient; no body or flowed callable is needed.
                        if self.declared_pack_recipe(function).is_some()
                            || !self.function_overload_signatures(function).is_empty()
                        {
                            return Some(Type::NativeFunction(function));
                        }
                    }
                    if let Some(written) = self.original_written_binding_contract(binding, queries) {
                        return Some(written);
                    }
                    // An immutable local alias names its initializer's contract,
                    // syntactically: Lua code localises globals (ge_tts's
                    // lunajson has `local error = error`). Bun accepts only
                    // annotated declarations; this is the one deviation.
                    let initializer = self.flow.immutable_initializer(binding)?;
                    let initializer = &self.flow.program.expression(initializer.expression).expression;
                    return self.original_effect_expression_contract(initializer, queries);
                }
                // This only selects the written contract of the original
                // environment. A lexical/reassigned _ENV needs its real view.
                if self.bound.environment_binding_at(name.span.start).is_some()
                    || !self.flow.environment_cell_is_immutable()
                { return None; }
                let binding = self.global_binding(&name.text)?;
                let globals = self.project_globals.as_ref()?;
                // Ambient declarations are authoritative written contracts;
                // implementation-only globals may require initializer inference.
                if !globals.declarations(binding).iter().any(|&owner| globals.is_declaration_owner(owner)) {
                    return None;
                }
                queries.global_binding_type(binding).ok()
            }
            ExprKind::Field { base, name } => {
                let receiver = self.original_effect_expression_contract(base, queries)?;
                queries.property_type(&receiver, &name.text)
            }
            ExprKind::Index { base, index } => {
                let key = crate::native_tables::literal_key(index)?;
                let receiver = self.original_effect_expression_contract(base, queries)?;
                queries.table_lookup(&receiver, &TableKey::Literal(key)).ok()
            }
            _ => None,
        }
    }
}
