//! Original execution/entry facts for native value-origin proofs. No inferred
//! types or call/write preservation results are stored by the source adapter.
use super::NativeRegistry;
use crate::flow;
use luanalysis_engine::{
    NativeAliasExecution, NativeAliasInitial, NativeFlowKey, NativeFunctionKey, NativeTypeProvider,
};

impl NativeRegistry {
    pub(super) fn source_alias_initial(&self, reference: &flow::Reference) -> NativeAliasInitial {
        let Some(globals) = &self.project_globals else {
            return NativeAliasInitial::Opaque;
        };
        if reference.root == flow::ReferenceRoot::EnvironmentObject && reference.path.is_empty() {
            return NativeAliasInitial::Environment(globals.environment);
        }
        if reference.root != flow::ReferenceRoot::EnvironmentObject {
            return NativeAliasInitial::Opaque;
        }
        let [flow::Key::String(name)] = reference.path.as_slice() else {
            return NativeAliasInitial::Opaque;
        };
        std::str::from_utf8(name)
            .ok()
            .and_then(|name| globals.binding(name))
            .map_or(NativeAliasInitial::Opaque, NativeAliasInitial::GlobalValue)
    }

    pub(super) fn source_alias_execution(
        &self,
        function: NativeFunctionKey,
        creation: NativeFlowKey,
    ) -> Option<NativeAliasExecution> {
        if self.source_role() == luanalysis_project::SourceRole::Declaration
            || self.file.has_lua_syntax_errors()
            || !self.contains_function(function)
            || creation.graph.snapshot != self.flow.program.snapshot
            || !self.flow.contains_graph(creation.graph)
            || creation.graph.region != 0
            || creation.graph.context != 0
        {
            return None;
        }
        let graph = self.flow.graph_for_function(function, None)?;
        let source = &self.flow.program.graphs[graph.region as usize];
        if self.flow.flow_key(source.creation?.input, 0) != creation {
            return None;
        }
        let mut terminals: Vec<_> = source
            .returns
            .iter()
            .map(|&at| self.flow.flow_key(at, 0))
            .collect();
        terminals.push(self.flow.flow_key(source.exit, 0));
        Some(NativeAliasExecution {
            entry: self.flow.flow_key(source.entry, 0),
            terminals,
        })
    }
}
