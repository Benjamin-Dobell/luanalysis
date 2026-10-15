//! Original normal-completion graphs for native reached-state call transfers.
//! These are source recipes and genuine signature facets, never inferred heaps.
use super::NativeRegistry;
use luanalysis_engine::{NativeAliasExecution, NativeFlowKey, NativeFunctionKey, NativeTypeProvider, TypeQueries};

impl NativeRegistry {
    pub(crate) fn source_function_flow_execution(
        &self,
        function: NativeFunctionKey,
        creation: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<NativeAliasExecution> {
        if self.source_role() == luanalysis_project::SourceRole::Declaration
            || self.file.has_lua_syntax_errors()
            || !self.contains_function(function)
            || !self.flow.contains_graph(creation.graph)
        { return None; }
        let original = self.flow.graph_for_function(function, None)?;
        let source = &self.flow.program.graphs[original.region as usize];
        // Function identities use the native source namespace; creation uses
        // the independent original flow namespace and its actual caller facet.
        if self.flow.flow_key(source.creation?.input, creation.graph.context) != creation { return None; }
        let captured = queries.current_captured_function()?;
        let value = queries.captured_function_value(&captured);
        if queries.native_function_key(&value) != Some(function) { return None; }
        self.retain_capture_ancestry(&captured, queries);
        let contextual = queries.current_contextual_function();
        let graph = self.flow.graph_with_contexts(original.region, contextual.as_ref(), Some(&captured));
        let mut terminals = Vec::with_capacity(source.returns.len() + 1);
        for &returned in &source.returns {
            let statement = self.flow.program.node(returned);
            if !matches!(statement.kind, crate::flow::NodeKind::Return { .. }) { return None; }
            let before = *statement.antecedents.first()?;
            terminals.push(self.flow.flow_key(before, graph.context));
        }
        terminals.push(self.flow.flow_key(source.exit, graph.context));
        terminals.sort_by_key(|terminal| terminal.node);
        terminals.dedup();
        Some(NativeAliasExecution {
            entry: self.flow.flow_key(source.entry, graph.context),
            terminals,
        })
    }
}
