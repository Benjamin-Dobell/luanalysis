//! Original Lua iterator headers and visible declaration projections.
//! The native shared walker owns frame feedback, joins and row filtering.

use crate::{
    Binding, bind::BindingId, flow, native_flow::FlowAdapter, native_functions::NativeRegistry,
};
use luanalysis_engine::{NativeFlowKey, TypeQueries, ValuePack};

impl NativeRegistry {
    pub(super) fn source_iterator_header(
        &self,
        iterator: flow::IteratorId,
        at: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        let sources = self.iterator_sources();
        let recipe = sources.program.iterator(iterator);
        let input = sources.flow_key(recipe.input, at.graph.context);
        let expressions: Vec<_> = recipe
            .values
            .iter()
            .map(|value| {
                let evaluation = sources.program.evaluation(*value);
                sources
                    .program
                    .expression(evaluation.expression)
                    .expression
                    .clone()
            })
            .collect();
        let mut checker = self.checker_at_flow(input, queries);
        let raw = checker.expression_list_pack(&expressions, None);
        let pack = match checker.engine.adjust_pack(&raw, 4) {
            Ok(pack) => pack,
            Err(error) => {
                checker.error(
                    "LUA2019",
                    recipe.span,
                    format!("native Lua iterator-header adjustment failed: {error:?}"),
                );
                ValuePack::never()
            }
        };
        if !checker.incomplete_flow {
            self.merge_flow_report(input, checker.check_mode, checker.report);
        }
        pack
    }

    pub(super) fn source_iterator_result(
        &self,
        iterator: flow::IteratorId,
        index: usize,
        at: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> luanalysis_engine::NativeFlowResult {
        let sources = self.iterator_sources();
        let frame = sources
            .iterator_reference(at.graph, iterator)
            .expect("original hidden iterator frame");
        queries.iterator_result_with_status(
            &FlowAdapter {
                sources,
                values: self,
            },
            at,
            frame,
            index,
        )
    }

    pub(super) fn iterator_body_binding(
        &self,
        binding: BindingId,
        at: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<(Binding, bool)> {
        let sources = self.iterator_sources();
        let write = sources.declaration_write(binding)?;
        let flow::NodeKind::Write {
            address,
            value: flow::ValueDemand::IteratorResult { iterator, index },
            ..
        } = &write.kind
        else {
            return None;
        };
        let input = sources.flow_key(address.epoch, at.graph.context);
        let result = self.source_iterator_result(*iterator, *index, input, queries);
        let constraint = queries.widen(&result.ty);
        Some((
            Binding {
                value: constraint.clone(),
                initial: constraint.clone(),
                constraint,
                evolves_from_nil: false,
            },
            result.incomplete,
        ))
    }
    pub(super) fn iterator_binding_in_current_context(
        &self,
        binding: BindingId,
        queries: &mut dyn TypeQueries,
    ) -> Option<(Binding, bool)> {
        let sources = self.iterator_sources();
        let write = sources.declaration_write(binding)?;
        if !matches!(
            write.kind,
            flow::NodeKind::Write {
                value: flow::ValueDemand::IteratorResult { .. },
                ..
            }
        ) {
            return None;
        }
        let contextual = queries.current_contextual_function();
        let captured = queries.current_captured_function();
        let graph =
            sources.graph_with_contexts(write.id.graph, contextual.as_ref(), captured.as_ref());
        self.iterator_body_binding(
            binding,
            NativeFlowKey {
                graph,
                node: write.id.index,
            },
            queries,
        )
    }
}
