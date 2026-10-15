//! Frozen original paths rooted in a proved runtime environment object.
//! Paths describe addresses, never attest the current value of a builtin member.

use super::NativeRegistry;
use crate::{bind::BindingId, flow};
use luanalysis_engine::{LuaKey, NativeFlowKey, TypeQueries};
use luanalysis_syntax::{Expr, ExprKind};
use std::collections::HashSet;

impl NativeRegistry {
    pub(crate) fn frozen_environment_path(
        &self,
        expression: &Expr,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Vec<flow::Key>> {
        self.environment_path(expression, input, queries, &mut HashSet::new(), 0)
    }

    fn environment_path(
        &self,
        expression: &Expr,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
        aliases: &mut HashSet<BindingId>,
        depth: usize,
    ) -> Option<Vec<flow::Key>> {
        if depth >= 128 || !self.flow.contains_graph(input.graph) { return None; }
        let mut expression = expression;
        while let ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }

        // An original allocation reached only through immutable local cells
        // cannot be the pre-existing global environment object. This is a Lua
        // identity fact; asking its written class assertion to prove it would
        // recursively validate that class while resolving its own methods.
        if self.immutable_fresh_table(expression, &mut HashSet::new()) { return None; }

        // This primitive query bypasses the assertion on the original receiver.
        // An equal-looking table schema cannot manufacture a runtime identity.
        let mut checker = self.checker_at_flow(input, queries);
        if checker.environment_origin_of_expression(expression).is_some() && !checker.incomplete_flow {
            return Some(Vec::new());
        }
        drop(checker);

        match &expression.kind {
            ExprKind::Field { base, name } => {
                let mut path = self.environment_path(base, input, queries, aliases, depth + 1)?;
                path.push(flow::Key::String(name.text.as_bytes().to_vec()));
                Some(path)
            }
            ExprKind::Index { base, index } => {
                let mut path = self.environment_path(base, input, queries, aliases, depth + 1)?;
                path.push(self.environment_path_key(index)?);
                Some(path)
            }
            ExprKind::Name(name) => {
                let Some(binding) = self.bound.binding_at(name.span.start) else {
                    let mut checker = self.checker_at_flow(input, queries);
                    let environment = checker.environment_value(name.span.start);
                    if checker.incomplete_flow || checker.engine.global_environment_origin(&environment).is_none() {
                        return None;
                    }
                    return Some(vec![flow::Key::String(name.text.as_bytes().to_vec())]);
                };
                if !aliases.insert(binding) { return None; }
                let result = self.immutable_environment_alias(binding, input, queries, aliases, depth + 1);
                aliases.remove(&binding);
                result
            }
            _ => None,
        }
    }

    fn immutable_environment_alias(
        &self,
        binding: BindingId,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
        aliases: &mut HashSet<BindingId>,
        depth: usize,
    ) -> Option<Vec<flow::Key>> {
        let initializer = self.immutable_initializer(binding)?;
        let original = &self.flow.program.expression(initializer.expression).expression;
        // A contextual callback's signature is not its enclosing chunk's
        // context. Reuse a context only for its genuine owning graph.
        let context = if initializer.input.graph == input.graph.region { input.graph.context }
            else if initializer.input.graph == 0 { 0 }
            else { return None; };
        let frozen = self.flow.flow_key(initializer.input, context);
        self.environment_path(original, frozen, queries, aliases, depth)
    }

    fn immutable_initializer(&self, binding: BindingId) -> Option<&flow::Evaluation> {
        // Count original stores across every actual graph. A later store, even
        // inside a closure, invalidates this immutable initializer proof.
        let mut writes = self.flow.program.graphs.iter().flat_map(|graph| &graph.nodes).filter_map(|node| {
            let flow::NodeKind::Write { address, value, declaration, .. } = &node.kind else { return None; };
            (address.reference.root == flow::ReferenceRoot::Binding(binding) && address.reference.path.is_empty())
                .then_some((value, *declaration))
        });
        let (flow::ValueDemand::Evaluation(initializer), true) = writes.next()? else { return None; };
        if writes.next().is_some() { return None; }
        Some(self.flow.program.evaluation(*initializer))
    }

    fn immutable_fresh_table(&self, expression: &Expr, seen: &mut HashSet<BindingId>) -> bool {
        let mut expression = expression;
        loop {
            while let ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }
            match &expression.kind {
                ExprKind::Table(_) => return true,
                ExprKind::Name(name) => {
                    let Some(binding) = self.bound.binding_at(name.span.start) else { return false };
                    if !seen.insert(binding) { return false; }
                    let Some(initializer) = self.immutable_initializer(binding) else { return false };
                    expression = &self.flow.program.expression(initializer.expression).expression;
                }
                _ => return false,
            }
        }
    }

    fn environment_path_key(&self, expression: &Expr) -> Option<flow::Key> {
        Some(match crate::native_tables::literal_key(expression) {
            Some(LuaKey::String(value)) => flow::Key::String(value),
            Some(LuaKey::Boolean(value)) => flow::Key::Boolean(value),
            Some(LuaKey::Integer(value)) => flow::Key::Integer(value),
            Some(LuaKey::Number(value)) if value >= i64::MIN as f64 && value < -(i64::MIN as f64) && value.fract() == 0.0 => flow::Key::Integer(value as i64),
            Some(LuaKey::Number(value)) => flow::Key::Float(value.to_bits()),
            None => flow::Key::Dynamic(self.flow.program.expressions.iter()
                .find(|source| source.expression.span == expression.span)?.id),
        })
    }
}
