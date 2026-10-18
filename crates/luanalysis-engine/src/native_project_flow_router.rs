//! Route project-wide original flow demands through the existing facade bridge.
//! The short-lived adapter borrows the installed provider and checker-owned
//! handles; it owns no source recipes, query state, or type caches.

use super::native_flow::FlowProviderAdapter;
use super::{Checker, NativeTypeProvider, ProviderAdapter};
use bun_sema::check::native_aliases::{
    NativeAliasCall, NativeAliasDispatch, NativeAliasExecution, NativeAliasInitial,
    NativeAliasReturn, NativeAliasReturnValue, NativeAliasValue, NativeAliasEnvironmentMember,
};
use bun_sema::check::native_flow::{
    self as core, NativeCallEffects, NativeFlowError, NativeFlowKey, NativeFlowNode,
    NativeFunctionCreation, NativeGraphKey, NativeIteratorProjection, NativeIteratorStage,
    NativePackProjection, NativePredicate, NativeReferenceKey, NativeValueKey,
    NativeVariableDeclaration, NativeWriteRelation,
};
use bun_sema::check::ResolvedTypeCallError;
use bun_sema::types::{NativeFunctionId, NativeResultPack, NativeTableKey, TypeId};

impl<P: NativeTypeProvider> ProviderAdapter<'_, P> {
    pub(super) fn project_flow_adapter(&self) -> FlowProviderAdapter<'_> {
        FlowProviderAdapter {
            provider: self.provider.project_flow_provider()
                .expect("project flow routing requires an installed original flow provider"),
            handles: self.handles,
        }
    }
}

impl<P: NativeTypeProvider> core::NativeFlowProvider for ProviderAdapter<'_, P> {
    fn alias_environment_candidate(&self, value: NativeValueKey, input: NativeFlowKey) -> bool {
        self.project_flow_adapter().alias_environment_candidate(value, input)
    }
    fn alias_value_type(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> TypeId {
        self.project_flow_adapter().alias_value_type(value, input, checker)
    }
    fn write_alias_receiver(&self, at: NativeFlowKey) -> Option<NativeAliasReturnValue> {
        self.project_flow_adapter().write_alias_receiver(at)
    }
    fn read_alias_receiver(&self, at: NativeFlowKey) -> Option<NativeAliasReturnValue> {
        self.project_flow_adapter().read_alias_receiver(at)
    }
    fn reference_proper_prefixes(&self, reference: NativeReferenceKey) -> Option<Vec<NativeReferenceKey>> {
        self.project_flow_adapter().reference_proper_prefixes(reference)
    }
    fn reference_member_path(&self, reference: NativeReferenceKey, member: NativeReferenceKey, checker: &mut Checker<'_, '_>) -> Option<Vec<NativeTableKey>> {
        self.project_flow_adapter().reference_member_path(reference, member, checker)
    }
    fn require_alias_loader_reference(&self, value: NativeValueKey, input: NativeFlowKey) -> Option<NativeReferenceKey> {
        self.project_flow_adapter().require_alias_loader_reference(value, input)
    }
    fn alias_callee_type(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> TypeId {
        self.project_flow_adapter().alias_callee_type(value, input, checker)
    }
    fn reference_alias_initial(&self, reference: NativeReferenceKey) -> NativeAliasInitial {
        self.project_flow_adapter().reference_alias_initial(reference)
    }

    fn alias_call_source(&self, value: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasCall> {
        self.project_flow_adapter().alias_call_source(value, input)
    }

    fn alias_node_dispatch(&self, at: NativeFlowKey) -> NativeAliasDispatch {
        self.project_flow_adapter().alias_node_dispatch(at)
    }
    fn known_standard_loader_write(&self, at: NativeFlowKey) -> bool {
        self.project_flow_adapter().known_standard_loader_write(at)
    }

    fn function_alias_execution(&self, function: NativeFunctionId, creation: NativeFlowKey) -> Option<NativeAliasExecution> {
        self.project_flow_adapter().function_alias_execution(function, creation)
    }

    fn require_alias_execution(&self, call: NativeValueKey, input: NativeFlowKey) -> Option<Vec<NativeAliasExecution>> {
        self.project_flow_adapter().require_alias_execution(call, input)
    }

    fn reference_is_cell(&self, reference: NativeReferenceKey) -> bool {
        self.project_flow_adapter().reference_is_cell(reference)
    }

    fn reference_written_contract(&self, reference: NativeReferenceKey, checker: &mut Checker<'_, '_>) -> Option<TypeId> {
        self.project_flow_adapter().reference_written_contract(reference, checker)
    }

    fn value_alias_source(&self, value: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        self.project_flow_adapter().value_alias_source(value, input)
    }
    fn value_alias_environment_member(&self, value: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasEnvironmentMember> {
        self.project_flow_adapter().value_alias_environment_member(value, input)
    }

    fn value_alias_member_source(&self, value: NativeValueKey, input: NativeFlowKey, key: NativeTableKey, checker: &Checker<'_, '_>) -> Option<NativeAliasReturnValue> {
        self.project_flow_adapter().value_alias_member_source(value, input, key, checker)
    }
    fn value_alias_member_absent(&self, value: NativeValueKey, input: NativeFlowKey, key: NativeTableKey, checker: &Checker<'_, '_>) -> bool {
        self.project_flow_adapter().value_alias_member_absent(value, input, key, checker)
    }

    fn function_alias_returns(&self, function: NativeFunctionId) -> Option<Vec<NativeAliasReturn>> {
        self.project_flow_adapter().function_alias_returns(function)
    }

    fn function_alias_exit(&self, function: NativeFunctionId) -> Option<NativeFlowKey> {
        self.project_flow_adapter().function_alias_exit(function)
    }

    fn reference_tracks_runtime_value(&self, reference: NativeReferenceKey) -> bool {
        self.project_flow_adapter().reference_tracks_runtime_value(reference)
    }

    fn reference_follows_creation(&self, reference: NativeReferenceKey) -> bool {
        self.project_flow_adapter().reference_follows_creation(reference)
    }

    fn runtime_type_operation(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> bool {
        self.project_flow_adapter().runtime_type_operation(value, input, checker)
    }

    fn value_literal_truthiness(&self, value: NativeValueKey) -> Option<bool> {
        self.project_flow_adapter().value_literal_truthiness(value)
    }

    fn value_lexical_standard(&self, value: NativeValueKey, input: NativeFlowKey) -> core::NativeLexicalStandard {
        self.project_flow_adapter().value_lexical_standard(value, input)
    }

    fn loader_never_mutated(&self, graph: NativeGraphKey) -> bool {
        self.project_flow_adapter().loader_never_mutated(graph)
    }

    fn write_member_key(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, checker: &mut Checker<'_, '_>) -> Option<NativeTableKey> {
        self.project_flow_adapter().write_member_key(reference, target, at, checker)
    }

    fn write_index_value(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey) -> Option<(NativeValueKey, NativeFlowKey)> {
        self.project_flow_adapter().write_index_value(reference, target, at)
    }
    fn write_index_appends(&self, at: NativeFlowKey) -> bool { self.project_flow_adapter().write_index_appends(at) }

    fn runtime_table_store_in_scope(&self, declaration: bun_sema::check::native_named::NativeTypeDeclarationId, at: NativeFlowKey) -> bool {
        self.project_flow_adapter().runtime_table_store_in_scope(declaration, at)
    }

    fn reference_member_key(&self, reference: NativeReferenceKey, member: NativeReferenceKey, checker: &mut Checker<'_, '_>) -> Option<NativeTableKey> {
        self.project_flow_adapter().reference_member_key(reference, member, checker)
    }

    fn assertion_predicate(&self, condition: NativeValueKey, input: NativeFlowKey) -> Option<NativePredicate> {
        self.project_flow_adapter().assertion_predicate(condition, input)
    }
    fn call_type_predicate(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> Option<bun_sema::check::native_signatures::NativeTypePredicate> {
        self.project_flow_adapter().call_type_predicate(value, input, checker)
    }


    fn reference_declaration(&self, reference: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        self.project_flow_adapter().reference_declaration(reference)
    }

    fn graph_creation(&self, graph: NativeGraphKey) -> Option<NativeFunctionCreation> {
        self.project_flow_adapter().graph_creation(graph)
    }

    fn reference_in_graph(&self, reference: NativeReferenceKey, graph: NativeGraphKey) -> Option<NativeReferenceKey> {
        self.project_flow_adapter().reference_in_graph(reference, graph)
    }

    fn iterator_error(&self, at: NativeFlowKey, stage: NativeIteratorStage, callee: TypeId, error: ResolvedTypeCallError, checker: &mut Checker<'_, '_>) {
        self.project_flow_adapter().iterator_error(at, stage, callee, error, checker);
    }

    fn iterator_projection(&self, reference: NativeReferenceKey) -> Option<NativeIteratorProjection> {
        self.project_flow_adapter().iterator_projection(reference)
    }

    fn pack_projection(&self, reference: NativeReferenceKey) -> Option<NativePackProjection> {
        self.project_flow_adapter().pack_projection(reference)
    }

    fn value_pack(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> NativeResultPack {
        self.project_flow_adapter().value_pack(value, input, checker)
    }

    fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        self.project_flow_adapter().contains_graph(graph)
    }

    fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
        self.project_flow_adapter().contains_reference(reference)
    }

    fn node(&self, node: NativeFlowKey) -> Option<NativeFlowNode> {
        self.project_flow_adapter().node(node)
    }

    fn is_shared(&self, node: NativeFlowKey) -> bool {
        self.project_flow_adapter().is_shared(node)
    }

    fn write_affects(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey) -> NativeWriteRelation {
        self.project_flow_adapter().write_affects(reference, target, at)
    }

    fn write_affects_with_types(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, checker: &mut Checker<'_, '_>) -> NativeWriteRelation {
        self.project_flow_adapter().write_affects_with_types(reference, target, at, checker)
    }

    fn write_affects_ignoring_descendants(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, checker: &mut Checker<'_, '_>) -> NativeWriteRelation {
        self.project_flow_adapter().write_affects_ignoring_descendants(reference, target, at, checker)
    }

    fn call_invalidates_reference(&self, reference: NativeReferenceKey, at: NativeFlowKey, checker: &mut Checker<'_, '_>) -> bool {
        self.project_flow_adapter().call_invalidates_reference(reference, at, checker)
    }

    fn value_type(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> TypeId {
        self.project_flow_adapter().value_type(value, input, checker)
    }

    fn call_effects(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> NativeCallEffects {
        self.project_flow_adapter().call_effects(value, input, checker)
    }

    fn flow_error(&self, at: NativeFlowKey, error: NativeFlowError) {
        self.project_flow_adapter().flow_error(at, error);
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NativeFunctionKey, Parameter, Type, TypeHandles, TypeQueries};

    fn original(node: u32) -> NativeFlowKey {
        NativeFlowKey { graph: NativeGraphKey { snapshot: 9821, region: 3, context: 7 }, node }
    }
    struct Recipes;
    impl NativeTypeProvider for Recipes {
        fn project_flow_provider(&self) -> Option<&dyn crate::NativeFlowProvider> { Some(self) }
        fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
        fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("metadata cannot request parameters") }
        fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { panic!("metadata cannot request a return contract") }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { panic!("metadata cannot infer a return type") }
        fn return_cycle(&self, _: NativeFunctionKey) { panic!("metadata cannot infer a return type") }
        fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("metadata cannot infer a return type") }
    }
    impl crate::NativeFlowProvider for Recipes {
        fn read_alias_receiver(&self, at: NativeFlowKey) -> Option<NativeAliasReturnValue> {
            (at == original(9)).then_some(NativeAliasReturnValue {
                value: NativeValueKey { snapshot: 9821, index: 4 }, input: original(2),
            })
        }
        fn contains_graph(&self, graph: NativeGraphKey) -> bool { graph == original(0).graph }
        fn contains_reference(&self, _: NativeReferenceKey) -> bool { false }
        fn node(&self, _: NativeFlowKey) -> Option<NativeFlowNode> { None }
        fn is_shared(&self, _: NativeFlowKey) -> bool { false }
        fn write_affects(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation { NativeWriteRelation::Unaffected }
        fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type { panic!("receiver metadata cannot request a value type") }
        fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { panic!("receiver metadata cannot inspect call effects") }
        fn flow_error(&self, _: NativeFlowKey, _: NativeFlowError) { panic!("original metadata is valid") }
    }

    #[test]
    fn installed_project_core_provider_retains_original_read_receiver_metadata() {
        let provider = Recipes;
        let handles = TypeHandles::new();
        let adapter = ProviderAdapter { provider: &provider, handles: &handles };
        // This is the same core-provider capability selected by call-state
        // replay, including both project and owned-value facade adapters.
        let core = bun_sema::check::NativeFunctionProvider::project_flow_provider(&adapter).unwrap();
        let receiver = core.read_alias_receiver(original(9)).expect("project routing must not fall back to absent metadata");
        assert_eq!(receiver.value, NativeValueKey { snapshot: 9821, index: 4 });
        assert_eq!(receiver.input, original(2));
        assert!(core.read_alias_receiver(original(8)).is_none());
    }
}
