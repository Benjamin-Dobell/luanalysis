use super::*;

const SNAPSHOT: u64 = 12_436;
fn graph() -> NativeGraphKey { NativeGraphKey { snapshot: SNAPSHOT, region: 0, context: 0 } }
fn at(node: u32) -> NativeFlowKey { NativeFlowKey { graph: graph(), node } }
fn reference(index: u32) -> NativeReferenceKey { NativeReferenceKey { graph: graph(), reference: index } }
fn value(index: u32) -> NativeValueKey { NativeValueKey { snapshot: SNAPSHOT, index } }
fn environment() -> NativeEnvironmentKey { NativeEnvironmentKey { snapshot: SNAPSHOT + 1, index: 0 } }
fn binding(index: u32) -> NativeGlobalBindingKey { NativeGlobalBindingKey { environment: environment(), index } }
fn declaration() -> NativeValueDeclarationKey { NativeValueDeclarationKey { snapshot: SNAPSHOT + 2, index: 0 } }
fn signature() -> NativeSignatureKey { NativeSignatureKey { snapshot: SNAPSHOT + 2, index: 0 } }
fn site() -> NativeIntrinsicSite { NativeIntrinsicSite { binding: binding(0), declaration: declaration(), kind: NativeIntrinsicKind::Assert } }

/// Models `local environment=_G; while again do ...;
/// environment[key]=ordinary end`. The heap receiver is frozen before the
/// store and reaches the real entry cell through the loop's metadata walk.
struct LoopStore {
    key: Option<&'static [u8]>,
    pending_receiver: bool,
}

impl NativeTypeProvider for LoopStore {
    fn assumed_standard_environment(&self) -> Option<NativeEnvironmentKey> { Some(environment()) }
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool { key == environment() }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool { key.environment == environment() && key.index < 2 }
    fn global_environment_binding(&self, key: NativeEnvironmentKey, name: &[u8]) -> Option<NativeGlobalBindingKey> {
        if key != environment() { return None; }
        match name { b"assert" => Some(binding(0)), b"_G" => Some(binding(1)), _ => None }
    }
    fn global_environment_self_binding(&self, key: NativeEnvironmentKey) -> Option<NativeGlobalBindingKey> { (key == environment()).then_some(binding(1)) }
    fn global_binding_declarations(&self, key: NativeGlobalBindingKey) -> Vec<NativeValueDeclarationKey> {
        if key == binding(0) { vec![declaration()] } else { Vec::new() }
    }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool { key == declaration() }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader { role: GlobalSourceRole::Declaration, kind: GlobalValueKind::Function }
    }
    fn value_declaration_signatures(&self, _: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> { Some(vec![signature()]) }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool { key == signature() }
    fn signature_intrinsic(&self, key: NativeSignatureKey) -> Option<NativeIntrinsicSite> { (key == signature()).then_some(site()) }
    fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("metadata-only loop write") }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { panic!("metadata-only loop write") }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { panic!("metadata-only loop write") }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("metadata-only loop write") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("metadata-only loop write") }
}

impl NativeFlowProvider for LoopStore {
    fn contains_graph(&self, key: NativeGraphKey) -> bool { key == graph() }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool { key.graph == graph() && key.reference < 4 }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        if key.graph != graph() { return None; }
        Some(match key.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Write { before: at(0), target: reference(1), value: value(10), input: at(0) },
            2 => NativeFlowNode::Loop { edges: vec![at(1), at(4)] },
            3 => NativeFlowNode::Pass { before: at(2) },
            4 => NativeFlowNode::Write { before: at(3), target: reference(2), value: value(11), input: at(3) },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn reference_is_cell(&self, key: NativeReferenceKey) -> bool { key == reference(1) }
    fn reference_alias_initial(&self, key: NativeReferenceKey) -> NativeAliasInitial {
        match key.reference {
            0 => NativeAliasInitial::GlobalValue(binding(0)),
            3 => NativeAliasInitial::GlobalValue(binding(1)),
            _ => NativeAliasInitial::Opaque,
        }
    }
    fn value_alias_source(&self, key: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        match key.index {
            0 => NativeAliasValue::Reference { reference: reference(0), input },
            2 => NativeAliasValue::Reference { reference: reference(1), input },
            10 if self.pending_receiver => NativeAliasValue::Reference { reference: reference(1), input: at(1) },
            10 => NativeAliasValue::Reference { reference: reference(3), input: at(0) },
            11 => NativeAliasValue::Unknown,
            _ => panic!("unexpected original value"),
        }
    }
    fn write_affects(&self, watched: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if watched == target { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
    }
    fn write_affects_with_types(&self, watched: NativeReferenceKey, target: NativeReferenceKey, key: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeWriteRelation {
        assert_ne!(key, at(4), "a loop environment store must use its frozen receiver, without flowing the alias again");
        self.write_affects(watched, target, key)
    }
    fn write_alias_receiver(&self, key: NativeFlowKey) -> Option<NativeAliasReturnValue> {
        (key == at(4)).then_some(NativeAliasReturnValue { value: value(2), input: at(3) })
    }
    fn write_member_key(&self, _: NativeReferenceKey, _: NativeReferenceKey, key: NativeFlowKey) -> Option<LuaKey> {
        (key == at(4)).then(|| self.key.map(|name| LuaKey::String(name.to_vec()))).flatten()
    }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type { panic!("loop write classification must not demand expression types") }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { panic!("loop write classification must not demand call effects") }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("invalid original test graph: {error:?}") }
}

#[test]
fn a_frozen_environment_alias_store_invalidates_the_original_loop_operation_without_flowed_types() {
    for key in [Some(b"assert".as_slice()), None] {
        Engine::new().with_native_provider(|| LoopStore { key, pending_receiver: false }, |source, queries| {
            assert_eq!(queries.flow_standard_operation(source, value(0), at(2)), NativeStandardOperation::Ordinary);
            // The host-policy loop walk supplies no strict runtime certificate.
            assert_eq!(queries.flow_alias_intrinsic(source, value(0), at(2)), NativeAliasIntrinsic::Unknown);
        });
    }
}

#[test]
fn a_different_fixed_environment_key_preserves_the_original_loop_operation() {
    Engine::new().with_native_provider(|| LoopStore { key: Some(b"other"), pending_receiver: false }, |source, queries| {
        assert_eq!(queries.flow_standard_operation(source, value(0), at(2)), NativeStandardOperation::Standard(site()));
    });
}

#[test]
fn a_genuinely_reentered_loop_receiver_stays_pending() {
    Engine::new().with_native_provider(|| LoopStore { key: Some(b"assert"), pending_receiver: true }, |source, queries| {
        assert_eq!(queries.flow_standard_operation(source, value(0), at(2)), NativeStandardOperation::Pending);
    });
}
