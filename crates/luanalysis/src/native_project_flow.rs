//! Project-wide transport of original flow recipes. Cross-file execution keeps
//! each source snapshot and graph intact; native queries own every proof.
use super::ProjectRegistry;
use crate::native_flow::FlowAdapter;
use luanalysis_engine::{
    CallError, LuaKey, NativeAliasCall, NativeAliasDispatch, NativeAliasExecution,
    NativeAliasInitial, NativeAliasReturn, NativeAliasReturnValue, NativeAliasValue, NativeAliasEnvironmentMember,
    NativeCallEffects, NativeFlowError, NativeFlowKey, NativeFlowNode, NativeFlowProvider,
    NativeFunctionCreation, NativeFunctionKey, NativeGraphKey, NativeIteratorProjection,
    NativeIteratorStage, NativePackProjection, NativePredicate, NativeReferenceKey, NativeValueKey,
    NativeVariableDeclaration, NativeWriteRelation, Type, TypeQueries, ValuePack,
};

impl ProjectRegistry {
    fn original_flow_registry(&self, snapshot: u64) -> Option<&super::NativeRegistry> {
        // Graph/value/reference snapshots belong to flow::ProgramFlow, not
        // the independently allocated native declaration namespace.
        Some(&self.files[*self.flow_snapshots.get(&snapshot)?].source)
    }
    fn original_flow(&self, snapshot: u64) -> Option<FlowAdapter<'_>> {
        let source = self.original_flow_registry(snapshot)?;
        Some(FlowAdapter {
            sources: source.iterator_sources(),
            values: source,
        })
    }
    fn original_function_flow(&self, snapshot: u64) -> Option<FlowAdapter<'_>> {
        let source = self.owner_if(snapshot)?;
        Some(FlowAdapter {
            sources: source.iterator_sources(),
            values: source,
        })
    }
}

macro_rules! route {
    ($domain:ident; $(fn $name:ident($($argument:ident: $ty:ty),*) -> $result:ty { $snapshot:expr; $fallback:expr })*) => {
        $(fn $name(&self, $($argument: $ty),*) -> $result {
            let Some(source) = self.$domain($snapshot) else { return $fallback; };
            source.$name($($argument),*)
        })*
    };
}

impl NativeFlowProvider for ProjectRegistry {
    route! {
        original_flow;
        fn alias_value_type(value: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type { value.snapshot; queries.error_type() }
        fn alias_environment_candidate(value: NativeValueKey, input: NativeFlowKey) -> bool { value.snapshot; false }
        fn write_alias_receiver(at: NativeFlowKey) -> Option<NativeAliasReturnValue> { at.graph.snapshot; None }
        fn read_alias_receiver(at: NativeFlowKey) -> Option<NativeAliasReturnValue> { at.graph.snapshot; None }
        fn reference_member_path(reference: NativeReferenceKey, member: NativeReferenceKey) -> Option<Vec<LuaKey>> { reference.graph.snapshot; None }
        fn reference_alias_initial(reference: NativeReferenceKey) -> NativeAliasInitial { reference.graph.snapshot; NativeAliasInitial::Opaque }
        fn alias_call_source(value: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasCall> { value.snapshot; None }
        fn alias_node_dispatch(at: NativeFlowKey) -> NativeAliasDispatch { at.graph.snapshot; NativeAliasDispatch::Unknown }
        fn known_standard_loader_write(at: NativeFlowKey) -> bool { at.graph.snapshot; false }
        fn reference_is_cell(reference: NativeReferenceKey) -> bool { reference.graph.snapshot; false }
        fn reference_written_contract(reference: NativeReferenceKey, queries: &mut dyn TypeQueries) -> Option<Type> { reference.graph.snapshot; None }
        fn value_alias_source(value: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue { value.snapshot; NativeAliasValue::Unknown }
        fn value_alias_environment_member(value: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasEnvironmentMember> { value.snapshot; None }
        fn value_alias_member_source(value: NativeValueKey, input: NativeFlowKey, key: &LuaKey) -> Option<NativeAliasReturnValue> { value.snapshot; None }
        fn value_alias_member_absent(value: NativeValueKey, input: NativeFlowKey, key: &LuaKey) -> bool { value.snapshot; false }
        fn reference_tracks_runtime_value(reference: NativeReferenceKey) -> bool { reference.graph.snapshot; false }
        fn reference_follows_creation(reference: NativeReferenceKey) -> bool { reference.graph.snapshot; false }
        fn runtime_type_operation(value: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> bool { value.snapshot; false }
        fn assertion_predicate(value: NativeValueKey, input: NativeFlowKey) -> Option<NativePredicate> { value.snapshot; None }
        fn reference_declaration(reference: NativeReferenceKey) -> Option<NativeVariableDeclaration> { reference.graph.snapshot; None }
        fn graph_creation(graph: NativeGraphKey) -> Option<NativeFunctionCreation> { graph.snapshot; None }
        fn iterator_error(at: NativeFlowKey, stage: NativeIteratorStage, callee: &Type, error: CallError, queries: &mut dyn TypeQueries) -> () { at.graph.snapshot; () }
        fn iterator_projection(reference: NativeReferenceKey) -> Option<NativeIteratorProjection> { reference.graph.snapshot; None }
        fn pack_projection(reference: NativeReferenceKey) -> Option<NativePackProjection> { reference.graph.snapshot; None }
        fn value_pack(value: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> ValuePack { value.snapshot; ValuePack::single(queries.error_type()) }
        fn contains_graph(graph: NativeGraphKey) -> bool { graph.snapshot; false }
        fn contains_reference(reference: NativeReferenceKey) -> bool { reference.graph.snapshot; false }
        fn node(at: NativeFlowKey) -> Option<NativeFlowNode> { at.graph.snapshot; None }
        fn is_shared(at: NativeFlowKey) -> bool { at.graph.snapshot; false }
        fn write_affects(reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey) -> NativeWriteRelation { at.graph.snapshot; NativeWriteRelation::PossibleAncestor }
        fn write_affects_with_types(reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, queries: &mut dyn TypeQueries) -> NativeWriteRelation { at.graph.snapshot; NativeWriteRelation::PossibleAncestor }
        fn call_invalidates_reference(reference: NativeReferenceKey, at: NativeFlowKey, queries: &mut dyn TypeQueries) -> bool { at.graph.snapshot; true }
        fn write_member_key(reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey) -> Option<LuaKey> { at.graph.snapshot; None }
        fn reference_member_key(reference: NativeReferenceKey, member: NativeReferenceKey) -> Option<LuaKey> { reference.graph.snapshot; None }
        fn value_type(value: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type { value.snapshot; queries.error_type() }
        fn alias_callee_type(value: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type { value.snapshot; queries.error_type() }
        fn call_effects(value: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> NativeCallEffects { value.snapshot; NativeCallEffects::Unsupported("foreign original flow value") }
        fn flow_error(at: NativeFlowKey, error: NativeFlowError) -> () { at.graph.snapshot; () }
    }
    route! {
        original_function_flow;
        fn function_alias_execution(function: NativeFunctionKey, creation: NativeFlowKey) -> Option<NativeAliasExecution> { function.snapshot; None }
        fn function_alias_returns(function: NativeFunctionKey) -> Option<Vec<NativeAliasReturn>> { function.snapshot; None }
        fn function_alias_exit(function: NativeFunctionKey) -> Option<NativeFlowKey> { function.snapshot; None }
    }

    fn require_alias_execution(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
    ) -> Option<Vec<NativeAliasExecution>> {
        let source = self.original_flow_registry(value.snapshot)?;
        let crate::native_flow::ValueRecipe::Evaluation(evaluation) =
            source.iterator_sources().value(value)?
        else {
            return None;
        };
        let evaluation = source.iterator_sources().program.evaluation(*evaluation);
        let expression = &source
            .iterator_sources()
            .program
            .expression(evaluation.expression)
            .expression;
        let (_, import) = source.module_import(expression.span)?;
        let super::ImportOutcome::Module(module) = import.outcome else {
            return None;
        };
        let target = self.owner_if(module.snapshot)?;
        if target.source_role() != luanalysis_project::SourceRole::Implementation
            || target.file.has_lua_syntax_errors()
        {
            return None;
        }
        let graph = target.iterator_sources().graph(0, 0);
        let original = &target.iterator_sources().program.graphs[0];
        let mut terminals: Vec<_> = original
            .returns
            .iter()
            .map(|&at| target.iterator_sources().flow_key(at, graph.context))
            .collect();
        terminals.push(
            target
                .iterator_sources()
                .flow_key(original.exit, graph.context),
        );
        // A source-host candidate is consumed only after the native worker
        // proves the actual standard loader and its unchanged namespace.
        let _ = input;
        Some(vec![NativeAliasExecution {
            entry: target
                .iterator_sources()
                .flow_key(original.entry, graph.context),
            terminals,
        }])
    }

    fn require_alias_loader_reference(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
    ) -> Option<NativeReferenceKey> {
        let source = self.original_flow_registry(value.snapshot)?;
        let crate::native_flow::ValueRecipe::Evaluation(evaluation) =
            source.iterator_sources().value(value)?
        else {
            return None;
        };
        let expression = &source
            .iterator_sources()
            .program
            .expression(
                source
                    .iterator_sources()
                    .program
                    .evaluation(*evaluation)
                    .expression,
            )
            .expression;
        source.module_import(expression.span)?;
        source
            .iterator_sources()
            .contains_graph(input.graph)
            .then(|| source.iterator_sources().loader_reference(input.graph))
    }

    fn reference_in_graph(
        &self,
        reference: NativeReferenceKey,
        graph: NativeGraphKey,
    ) -> Option<NativeReferenceKey> {
        let source = self.original_flow(reference.graph.snapshot)?;
        let target = self.original_flow(graph.snapshot)?;
        let recipe = source.sources.source_reference(reference)?;
        if reference.graph.snapshot != graph.snapshot
            && !matches!(
                recipe.root,
                crate::flow::ReferenceRoot::EnvironmentObject
                    | crate::flow::ReferenceRoot::LoaderState
            )
        {
            // Binding IDs and lexical environment cells belong to their actual
            // file/activation. Only this project's selected physical global
            // environment object and loader-state sentinel are shared domains.
            return None;
        }
        target.sources.reference_key(graph, recipe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_functions::project_provider::RegistryInput;
    use luanalysis_engine::NativeTypeProvider;
    use luanalysis_project::{FileId, SourceRole};

    fn input(index: u32, source: &str) -> RegistryInput {
        let syntax = luanalysis_syntax::parse(source);
        assert!(syntax.diagnostics.is_empty());
        let bound = crate::bind::bind(&syntax);
        RegistryInput {
            id: FileId { project: 778, index },
            role: SourceRole::Implementation,
            standard_library: None,
            syntax,
            bound,
        }
    }

    #[test]
    fn native_function_and_flow_keys_route_through_their_own_original_namespaces() {
        let syntax = luanalysis_syntax::parse("local independent={}\n");
        let bound = crate::bind::bind(&syntax);
        let independent = crate::flow::build(&syntax, &bound);
        let project = ProjectRegistry::new(vec![
            input(0, "local function first() return {} end\n"),
            input(1, "local function second() return {},{} end\n"),
        ]);
        for file in &project.files {
            let source = &file.source;
            let graph = source.iterator_sources().graph(0, 0);
            assert_eq!(graph.snapshot, source.iterator_sources().program.snapshot);
            assert_ne!(graph.snapshot, independent.snapshot);
            assert!(project.contains_graph(graph));
            let entry = source.iterator_sources().flow_key(source.iterator_sources().program.graphs[0].entry, 0);
            assert!(matches!(project.node(entry), Some(NativeFlowNode::Start)));
            let function = source.key_for(&source.functions[0]);
            assert_eq!(function.snapshot, source.snapshot);
            assert!(source.contains_function(function));
            let body = source.iterator_sources().graph_for_function(function, None).unwrap();
            let creation = source.iterator_sources().graph_creation(body).unwrap().input;
            let execution = project.function_alias_execution(function, creation)
                .expect("actual original function and creation graph belong to this source");
            assert_eq!(execution.entry.graph, body);
            assert!(project.function_alias_returns(function).is_some());
            assert!(project.function_alias_exit(function).is_some());
        }
    }

    #[test]
    fn original_read_receivers_keep_their_flow_owner_and_pre_key_evaluation_input() {
        let syntax = luanalysis_syntax::parse("local independent={}\n");
        let bound = crate::bind::bind(&syntax);
        let independent = crate::flow::build(&syntax, &bound);
        let project = ProjectRegistry::new(vec![
            input(0, "local value={}\nlocal function replace() value={} return 'count' end\nlocal field=value.count\nlocal indexed=value[replace()]\nlocal plain=value\n"),
            input(1, "local other={}\nlocal function replace() other={} return 'slot' end\nlocal field=other.slot\nlocal indexed=other[replace()]\nlocal plain=other\n"),
        ]);
        for file in &project.files {
            let sources = file.source.iterator_sources();
            assert_ne!(sources.program.snapshot, independent.snapshot);
            let mut reads = 0;
            for node in &sources.program.graphs[0].nodes {
                let at = sources.flow_key(node.id, 0);
                let actual = project.read_alias_receiver(at);
                let crate::flow::NodeKind::Eval(evaluation) = node.kind else {
                    assert!(actual.is_none(), "only an original field/index read supplies a receiver");
                    continue;
                };
                let evaluation = sources.program.evaluation(evaluation);
                let expression = &sources.program.expression(evaluation.expression).expression;
                if !matches!(expression.kind, luanalysis_syntax::ExprKind::Field { .. } | luanalysis_syntax::ExprKind::Index { .. }) {
                    assert!(actual.is_none(), "ordinary name reads do not manufacture heap receivers");
                    continue;
                }
                reads += 1;
                let receiver = sources.program.evaluation(evaluation.operands[0]);
                let actual = actual.expect("the original field/index read retains its first operand");
                assert_eq!(actual.value, sources.evaluation_value(receiver.id));
                assert_eq!(actual.input, sources.flow_key(receiver.input, at.graph.context));
                assert_eq!(actual.value.snapshot, sources.program.snapshot);
                if matches!(expression.kind, luanalysis_syntax::ExprKind::Index { .. }) {
                    let key = sources.program.evaluation(evaluation.operands[1]);
                    assert!(matches!(sources.program.expression(key.expression).expression.kind, luanalysis_syntax::ExprKind::Call { .. }));
                    assert_ne!(actual.input, sources.flow_key(key.completed, at.graph.context), "the receiver is frozen before the key call can replace its lexical root");
                    assert!(receiver.input.index < key.completed.index);
                }
            }
            assert_eq!(reads, 2);
        }
    }
}
