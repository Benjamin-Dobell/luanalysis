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
    ordinary_receiver: bool,
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
            10 if self.ordinary_receiver => NativeAliasValue::Unknown,
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
        Engine::new().with_native_provider(|| LoopStore { key, pending_receiver: false, ordinary_receiver: false }, |source, queries| {
            assert_eq!(queries.flow_standard_operation(source, value(0), at(2)), NativeStandardOperation::Ordinary);
            // The host-policy loop walk supplies no strict runtime certificate.
            assert_eq!(queries.flow_alias_intrinsic(source, value(0), at(2)), NativeAliasIntrinsic::Unknown);
        });
    }
}

#[test]
fn a_different_fixed_environment_key_preserves_the_original_loop_operation() {
    Engine::new().with_native_provider(|| LoopStore { key: Some(b"other"), pending_receiver: false, ordinary_receiver: false }, |source, queries| {
        assert_eq!(queries.flow_standard_operation(source, value(0), at(2)), NativeStandardOperation::Standard(site()));
    });
}

#[test]
fn a_genuinely_reentered_loop_receiver_stays_pending() {
    Engine::new().with_native_provider(|| LoopStore { key: Some(b"assert"), pending_receiver: true, ordinary_receiver: false }, |source, queries| {
        assert_eq!(queries.flow_standard_operation(source, value(0), at(2)), NativeStandardOperation::Pending);
    });
}

#[test]
fn an_ordinary_loop_table_store_does_not_demand_a_global_purity_proof() {
    for key in [Some(b"assert".as_slice()), None] {
        Engine::new().with_native_provider(|| LoopStore { key, pending_receiver: false, ordinary_receiver: true }, |source, queries| {
            assert_eq!(queries.flow_standard_operation(source, value(0), at(2)), NativeStandardOperation::Standard(site()));
            assert_eq!(queries.flow_alias_intrinsic(source, value(0), at(2)), NativeAliasIntrinsic::Unknown);
        });
    }
}

fn captured_graph(region: u32) -> NativeGraphKey { NativeGraphKey { region, ..graph() } }
fn captured_at(region: u32, node: u32) -> NativeFlowKey { NativeFlowKey { graph: captured_graph(region), node } }
fn captured_ref(region: u32, reference: u32) -> NativeReferenceKey { NativeReferenceKey { graph: captured_graph(region), reference } }

/// A captured builtin copy is used before nested loops. Resolving another
/// loop callee would revisit this call if the immutable capture walked the
/// local body instead of using its original closure-creation value.
struct CapturedLoop { mutation: u8 }
impl NativeTypeProvider for CapturedLoop {
    fn assumed_standard_environment(&self) -> Option<NativeEnvironmentKey> { Some(environment()) }
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool { key == environment() }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool { key == binding(0) }
    fn global_binding_declarations(&self, _: NativeGlobalBindingKey) -> Vec<NativeValueDeclarationKey> { vec![declaration()] }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool { key == declaration() }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader { role: GlobalSourceRole::Declaration, kind: GlobalValueKind::Function }
    }
    fn value_declaration_signatures(&self, _: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> { Some(vec![signature()]) }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool { key == signature() }
    fn signature_intrinsic(&self, key: NativeSignatureKey) -> Option<NativeIntrinsicSite> { (key == signature()).then_some(site()) }
    fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("immutable capture is metadata-only") }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { panic!("immutable capture is metadata-only") }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { panic!("immutable capture is metadata-only") }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("immutable capture is metadata-only") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("immutable capture is metadata-only") }
}
impl NativeFlowProvider for CapturedLoop {
    fn contains_graph(&self, key: NativeGraphKey) -> bool { key == captured_graph(0) || key == captured_graph(1) }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool { self.contains_graph(key.graph) && key.reference < 3 }
    fn reference_is_cell(&self, key: NativeReferenceKey) -> bool { key.reference != 0 }
    fn reference_alias_initial(&self, key: NativeReferenceKey) -> NativeAliasInitial {
        if key.reference == 0 { NativeAliasInitial::GlobalValue(binding(0)) } else { NativeAliasInitial::Opaque }
    }
    fn reference_declaration(&self, key: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        let owner = if key.reference == 1 { captured_graph(0) } else { key.graph };
        let mut writes = if key.reference == 1 { vec![NativeVariableWrite { owner, statement_end: 10 }] } else { Vec::new() };
        if key.reference == 1 && self.mutation != 0 {
            writes.push(NativeVariableWrite { owner: captured_graph(if self.mutation == 1 { 0 } else { 1 }), statement_end: 30 });
        }
        Some(NativeVariableDeclaration { owner, reference: NativeReferenceKey { graph: owner, reference: key.reference }, writes })
    }
    fn graph_creation(&self, key: NativeGraphKey) -> Option<NativeFunctionCreation> {
        (key == captured_graph(1)).then_some(NativeFunctionCreation { input: captured_at(0, 1), position: 20 })
    }
    fn reference_in_graph(&self, key: NativeReferenceKey, target: NativeGraphKey) -> Option<NativeReferenceKey> {
        let original = NativeReferenceKey { graph: target, ..key };
        self.contains_reference(original).then_some(original)
    }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        if !self.contains_graph(key.graph) { return None; }
        Some(match (key.graph.region, key.node) {
            (_, 0) => NativeFlowNode::Start,
            (0, 1) => NativeFlowNode::Write { before: captured_at(0, 0), target: captured_ref(0, 1), value: value(10), input: captured_at(0, 0) },
            (0, 2) if self.mutation == 1 => NativeFlowNode::Write { before: captured_at(0, 1), target: captured_ref(0, 1), value: value(11), input: captured_at(0, 1) },
            (1, 1) => NativeFlowNode::Loop { edges: vec![captured_at(1, 0), captured_at(1, if self.mutation == 2 { 5 } else { 4 })] },
            (1, 2) => NativeFlowNode::Call { before: captured_at(1, 1), value: value(20), input: captured_at(1, 1) },
            (1, 3) => NativeFlowNode::Loop { edges: vec![captured_at(1, 2), captured_at(1, 4)] },
            (1, 4) => NativeFlowNode::Call { before: captured_at(1, 3), value: value(21), input: captured_at(1, 3) },
            (1, 5) if self.mutation == 2 => NativeFlowNode::Write { before: captured_at(1, 4), target: captured_ref(1, 1), value: value(11), input: captured_at(1, 4) },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn value_alias_source(&self, key: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        match key.index {
            0 => NativeAliasValue::Reference { reference: captured_ref(1, 1), input },
            1 => NativeAliasValue::Reference { reference: captured_ref(1, 2), input },
            10 => NativeAliasValue::Reference { reference: captured_ref(0, 0), input: captured_at(0, 0) },
            _ => NativeAliasValue::Unknown,
        }
    }
    fn alias_call_source(&self, key: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasCall> {
        let callee = match key.index { 20 => value(0), 21 => value(1), _ => return None };
        Some(NativeAliasCall { callee: NativeAliasReturnValue { value: callee, input }, arguments: Vec::new(), execution_input: input })
    }
    fn write_affects(&self, watched: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if watched == target { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
    }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type { panic!("immutable capture must not demand flowed types") }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { panic!("immutable capture must not demand call effects") }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("invalid original test graph: {error:?}") }
}

#[test]
fn an_immutable_captured_callee_does_not_walk_its_own_nested_loop_calls() {
    Engine::new().with_native_provider(|| CapturedLoop { mutation: 0 }, |source, queries| {
        assert_eq!(queries.flow_standard_operation(source, value(0), captured_at(1, 1)), NativeStandardOperation::Standard(site()));
    });
}

#[test]
fn a_late_or_foreign_capture_store_cannot_use_the_creation_value() {
    for mutation in [1, 2] {
        Engine::new().with_native_provider(|| CapturedLoop { mutation }, |source, queries| {
            assert_eq!(queries.flow_standard_operation(source, value(0), captured_at(1, 1)), NativeStandardOperation::Ordinary);
        });
    }
}
struct MemberStore { copied: bool, pending: bool, dynamic: bool }
impl NativeTypeProvider for MemberStore {
    fn assumed_standard_environment(&self) -> Option<NativeEnvironmentKey> { Some(environment()) }
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool { key == environment() }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool { key.environment == environment() && key.index < 2 }
    fn global_environment_binding(&self, key: NativeEnvironmentKey, name: &[u8]) -> Option<NativeGlobalBindingKey> {
        if key != environment() { return None }
        match name { b"assert" => Some(binding(0)), b"_G" => Some(binding(1)), _ => None }
    }
    fn global_binding_declarations(&self, key: NativeGlobalBindingKey) -> Vec<NativeValueDeclarationKey> { if key == binding(0) { vec![declaration()] } else { Vec::new() } }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool { key == declaration() }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader { ValueDeclarationHeader { role: GlobalSourceRole::Declaration, kind: GlobalValueKind::Function } }
    fn value_declaration_signatures(&self, _: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> { Some(vec![signature()]) }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool { key == signature() }
    fn signature_intrinsic(&self, key: NativeSignatureKey) -> Option<NativeIntrinsicSite> { (key == signature()).then_some(site()) }
    fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("member operation must not demand a body") }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { panic!("member operation must not demand a body") }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { panic!("member operation must not demand a body") }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("member operation must not demand a body") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("member operation must not demand a body") }
}
impl NativeFlowProvider for MemberStore {
    fn contains_graph(&self, key: NativeGraphKey) -> bool { key == graph() }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool { key.graph == graph() && key.reference < 5 }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        if key.graph != graph() { return None }
        Some(match key.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Write { before: at(0), target: reference(1), value: value(10), input: at(0) },
            2 => NativeFlowNode::Write { before: at(1), target: reference(2), value: value(11), input: at(1) },
            3 => NativeFlowNode::Write { before: at(2), target: reference(3), value: value(12), input: at(2) },
            4 => NativeFlowNode::Write { before: at(3), target: reference(4), value: value(13), input: at(3) },
            5 => NativeFlowNode::Write { before: at(4), target: reference(4), value: value(12), input: at(4) },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn reference_is_cell(&self, key: NativeReferenceKey) -> bool { matches!(key.reference, 1 | 2) }
    fn reference_alias_initial(&self, key: NativeReferenceKey) -> NativeAliasInitial {
        if key == reference(0) { NativeAliasInitial::GlobalValue(binding(0)) } else { NativeAliasInitial::Opaque }
    }
    fn value_alias_source(&self, key: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        match key.index {
            0 => NativeAliasValue::Reference { reference: reference(3), input },
            10 => NativeAliasValue::Allocation,
            11 if self.pending => NativeAliasValue::Reference { reference: reference(2), input: at(2) },
            11 if self.copied => NativeAliasValue::Reference { reference: reference(1), input },
            11 => NativeAliasValue::Allocation,
            12 => NativeAliasValue::Reference { reference: reference(0), input: at(0) },
            13 => NativeAliasValue::Unknown,
            21 => NativeAliasValue::Reference { reference: reference(1), input },
            22 => NativeAliasValue::Reference { reference: reference(2), input },
            _ => panic!("unexpected original member value"),
        }
    }
    fn write_affects(&self, watched: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if watched == target { NativeWriteRelation::Exact }
        else if watched == reference(3) && target == reference(1) { NativeWriteRelation::Ancestor }
        else { NativeWriteRelation::Unaffected }
    }
    fn write_affects_with_types(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeWriteRelation { panic!("ordinary member writes must not start a physical heap proof") }
    fn reference_proper_prefixes(&self, key: NativeReferenceKey) -> Option<Vec<NativeReferenceKey>> {
        (key == reference(3)).then(|| vec![reference(1)])
    }
    fn reference_member_path(&self, root: NativeReferenceKey, member: NativeReferenceKey) -> Option<Vec<LuaKey>> {
        (root == reference(1) && member == reference(3)).then(|| vec![LuaKey::String(b"guard".to_vec())])
    }
    fn write_alias_receiver(&self, key: NativeFlowKey) -> Option<NativeAliasReturnValue> {
        let receiver = match key.node { 3 => 1, 4 | 5 => 2, _ => return None };
        Some(NativeAliasReturnValue { value: value(20 + receiver), input: at(key.node - 1) })
    }
    fn write_member_key(&self, _: NativeReferenceKey, _: NativeReferenceKey, key: NativeFlowKey) -> Option<LuaKey> {
        (key.node == 3 || matches!(key.node, 4 | 5) && !self.dynamic).then(|| LuaKey::String(b"guard".to_vec()))
    }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type { panic!("ordinary member writes must not demand flowed types") }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { panic!("ordinary member writes must not demand call effects") }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("invalid original member graph: {error:?}") }
}

#[test]
fn unrelated_dynamic_and_fixed_registry_stores_preserve_member_operations_without_raw_proofs() {
    for dynamic in [false, true] {
        Engine::new().with_native_provider(|| MemberStore { copied: false, pending: false, dynamic }, |source, queries| {
            assert_eq!(queries.flow_standard_operation(source, value(0), at(4)), NativeStandardOperation::Standard(site()));
        });
    }
}

#[test]
fn copied_receiver_stores_replace_the_member_operation_and_real_pending_stays_pending() {
    for dynamic in [false, true] {
        Engine::new().with_native_provider(|| MemberStore { copied: true, pending: false, dynamic }, |source, queries| {
            assert_eq!(queries.flow_standard_operation(source, value(0), at(4)), NativeStandardOperation::Ordinary);
            if !dynamic {
                assert_eq!(queries.flow_standard_operation(source, value(0), at(5)), NativeStandardOperation::Standard(site()));
            }
        });
    }
    Engine::new().with_native_provider(|| MemberStore { copied: false, pending: true, dynamic: false }, |source, queries| {
        for _ in 0..2 {
            assert_eq!(queries.flow_standard_operation(source, value(0), at(4)), NativeStandardOperation::Pending);
        }
    });
}
