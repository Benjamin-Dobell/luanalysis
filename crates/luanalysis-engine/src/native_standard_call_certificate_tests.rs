use super::*;
use std::collections::HashMap;

const SNAPSHOT: u64 = 12_432;
const MODULE_SNAPSHOT: u64 = 12_433;

fn graph(region: u32) -> NativeGraphKey {
    NativeGraphKey { snapshot: if region == 4 { MODULE_SNAPSHOT } else { SNAPSHOT }, region, context: 0 }
}

fn at(region: u32, node: u32) -> NativeFlowKey {
    NativeFlowKey { graph: graph(region), node }
}

fn reference(region: u32, reference: u32) -> NativeReferenceKey {
    NativeReferenceKey { graph: graph(region), reference }
}

fn value(region: u32, index: u32) -> NativeValueKey {
    NativeValueKey { snapshot: graph(region).snapshot, index }
}

fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey { snapshot: SNAPSHOT, index }
}

fn environment(index: u32) -> NativeEnvironmentKey {
    NativeEnvironmentKey { snapshot: 12_434, index }
}

fn binding(index: u32) -> NativeGlobalBindingKey {
    NativeGlobalBindingKey { environment: environment(0), index }
}

fn declaration(index: u32) -> NativeValueDeclarationKey {
    NativeValueDeclarationKey { snapshot: 12_435, index }
}

fn signature(index: u32) -> NativeSignatureKey {
    NativeSignatureKey { snapshot: 12_435, index }
}

fn site(index: u32, kind: NativeIntrinsicKind) -> NativeIntrinsicSite {
    NativeIntrinsicSite { binding: binding(index), declaration: declaration(index), kind }
}

/// Only original ownership metadata is available. Every callback that could
/// reconstruct a standard operation from a callable type is forbidden below.
struct StandardRecipes {
    routed: bool,
    original_loader: bool,
    extra_owner: Option<NativeValueDeclarationKey>,
    long_form: bool,
    nodes: HashMap<NativeFlowKey, NativeFlowNode>,
}

impl StandardRecipes {
    fn new(routed: bool) -> Self {
        let mut nodes = HashMap::new();
        let mut insert = |region, node, recipe| { nodes.insert(at(region, node), recipe); };
        insert(0, 0, NativeFlowNode::Start);
        // Copy the actual initial standard value into a mutable lexical cell.
        insert(0, 1, NativeFlowNode::Write { before: at(0, 0), target: reference(0, 1), value: value(0, 0), input: at(0, 0) });
        insert(0, 2, NativeFlowNode::Call { before: at(0, 1), value: value(0, 101), input: at(0, 1) });
        insert(0, 3, NativeFlowNode::Call { before: at(0, 2), value: value(0, 102), input: at(0, 2) });
        // Replace the environment's original operation before importing again.
        insert(0, 4, NativeFlowNode::Write { before: at(0, 3), target: reference(0, 0), value: value(0, 30), input: at(0, 3) });
        insert(0, 5, NativeFlowNode::Call { before: at(0, 4), value: value(0, 104), input: at(0, 4) });
        // An independent original continuation supplies setmetatable's table.
        insert(0, 6, NativeFlowNode::Write { before: at(0, 0), target: reference(0, 12), value: value(0, 20), input: at(0, 0) });
        insert(0, 7, NativeFlowNode::Call { before: at(0, 6), value: value(0, 100), input: at(0, 6) });
        insert(0, 8, NativeFlowNode::Call { before: at(0, 1), value: value(0, 104), input: at(0, 1) });
        // A separate continuation replaces the loader state while leaving the
        // selected require function and setmetatable binding unchanged.
        insert(0, 9, NativeFlowNode::Write { before: at(0, 1), target: reference(0, 14), value: value(0, 23), input: at(0, 1) });
        insert(0, 10, NativeFlowNode::Call { before: at(0, 9), value: value(0, 104), input: at(0, 9) });
        // A known pure function writes only its own unrelated local cell.
        insert(1, 0, NativeFlowNode::Start);
        insert(1, 1, NativeFlowNode::Write { before: at(1, 0), target: reference(1, 13), value: value(1, 21), input: at(1, 0) });
        insert(1, 2, NativeFlowNode::Return { before: at(1, 1) });
        // This function executes a captured write to the copied value.
        insert(2, 0, NativeFlowNode::Start);
        insert(2, 1, NativeFlowNode::Write { before: at(2, 0), target: reference(2, 1), value: value(2, 30), input: at(2, 0) });
        insert(2, 2, NativeFlowNode::Return { before: at(2, 1) });
        // An imported original chunk uses the environment at its invocation,
        // even though its own entry metadata describes the initial library.
        insert(4, 0, NativeFlowNode::Start);
        insert(4, 1, NativeFlowNode::Call { before: at(4, 0), value: value(4, 105), input: at(4, 0) });
        insert(4, 2, NativeFlowNode::Return { before: at(4, 1) });
        Self { routed, original_loader: true, extra_owner: None, long_form: false, nodes }
    }

    fn selected() -> NativeAliasIntrinsic {
        NativeAliasIntrinsic::Standard(site(0, NativeIntrinsicKind::SetMetatable))
    }

    fn execution(region: u32) -> NativeAliasExecution {
        NativeAliasExecution { entry: at(region, 0), terminals: vec![at(region, 2)] }
    }
    fn function_heads(original: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> {
        if original == declaration(1) { return None; }
        Some(match original.index {
            0 => vec![signature(0), signature(9)],
            7 => vec![signature(7), signature(8)],
            index => vec![signature(index)],
        })
    }
}

impl NativeTypeProvider for StandardRecipes {
    fn project_flow_provider(&self) -> Option<&dyn NativeFlowProvider> {
        self.routed.then_some(self as &dyn NativeFlowProvider)
    }

    fn contains_global_environment(&self, original: NativeEnvironmentKey) -> bool { original == environment(0) }
    fn contains_global_binding(&self, original: NativeGlobalBindingKey) -> bool {
        original.environment == environment(0) && original.index < 8
    }
    fn global_environment_self_binding(&self, original: NativeEnvironmentKey) -> Option<NativeGlobalBindingKey> {
        (original == environment(0)).then_some(binding(1))
    }
    fn global_binding_declarations(&self, original: NativeGlobalBindingKey) -> Vec<NativeValueDeclarationKey> {
        let mut owners = vec![declaration(original.index)];
        if original == binding(0) { owners.extend(self.extra_owner); }
        owners
    }
    fn contains_value_declaration(&self, original: NativeValueDeclarationKey) -> bool {
        original.snapshot == declaration(0).snapshot && original.index < 8
    }
    fn value_declaration_header(&self, original: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader {
            role: if original == declaration(6) { GlobalSourceRole::Implementation } else { GlobalSourceRole::Declaration },
            kind: if original == declaration(1) { GlobalValueKind::Variable } else { GlobalValueKind::Function },
        }
    }
    fn value_declaration_signatures(&self, original: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> {
        if self.long_form { None } else { Self::function_heads(original) }
    }
    fn value_declaration_function_signatures(&self, original: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> {
        Self::function_heads(original)
    }
    fn contains_signature(&self, original: NativeSignatureKey) -> bool {
        original.snapshot == signature(0).snapshot && original.index < 10
    }
    fn signature_intrinsic(&self, original: NativeSignatureKey) -> Option<NativeIntrinsicSite> {
        Some(match original.index {
            0 | 9 => site(0, NativeIntrinsicKind::SetMetatable),
            2 => site(2, NativeIntrinsicKind::Require),
            // These written heads claim another binding or declaration.
            3 => NativeIntrinsicSite { binding: binding(0), declaration: declaration(3), kind: NativeIntrinsicKind::SetMetatable },
            4 => NativeIntrinsicSite { binding: binding(4), declaration: declaration(0), kind: NativeIntrinsicKind::SetMetatable },
            6 => site(6, NativeIntrinsicKind::SetMetatable),
            7 => site(7, NativeIntrinsicKind::SetMetatable),
            8 => site(7, NativeIntrinsicKind::Type),
            _ => return None,
        })
    }

    fn value_declaration_contract(&self, _: NativeValueDeclarationKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("standard value identity must not demand a declaration contract")
    }
    fn value_declaration_initializer(&self, _: NativeValueDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        panic!("standard value identity must not inspect an ambient initializer")
    }
    fn signature_header(&self, _: NativeSignatureKey) -> SignatureHeader {
        panic!("original signature membership is sufficient; no signature type is needed")
    }
    fn signature_parameters(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("a callable parameter shape does not establish its runtime identity")
    }
    fn signature_declared_return_pack(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> ValuePack {
        panic!("a callable return shape does not establish its runtime identity")
    }
    fn contains_function(&self, original: NativeFunctionKey) -> bool {
        original.snapshot == SNAPSHOT && (1..=3).contains(&original.index)
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("original execution proof must not inspect function parameter types")
    }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("original execution proof must not inspect function return types")
    }
    fn function_declared_scalar_contract(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<ValuePack> {
        panic!("original execution proof must not inspect a declared result pack")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("original execution proof must not infer function bodies")
    }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("unexpected return inference") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("unexpected return inference") }
}

impl NativeFlowProvider for StandardRecipes {
    fn reference_alias_initial(&self, original: NativeReferenceKey) -> NativeAliasInitial {
        match original.reference {
            0 | 10 => NativeAliasInitial::GlobalValue(binding(0)),
            2 => NativeAliasInitial::GlobalValue(binding(1)),
            3 => NativeAliasInitial::Environment(environment(0)),
            4 => NativeAliasInitial::GlobalValue(binding(2)),
            5 => NativeAliasInitial::GlobalValue(binding(3)),
            6 => NativeAliasInitial::GlobalValue(binding(4)),
            7 => NativeAliasInitial::GlobalValue(binding(5)),
            8 => NativeAliasInitial::GlobalValue(binding(6)),
            9 => NativeAliasInitial::Environment(environment(1)),
            11 => NativeAliasInitial::GlobalValue(binding(7)),
            15 => NativeAliasInitial::GlobalValue(NativeGlobalBindingKey { environment: environment(1), index: 0 }),
            _ => NativeAliasInitial::Opaque,
        }
    }

    fn value_alias_source(&self, original: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        let reference_index = match original.index {
            0 => Some(0), 1 => Some(2), 2 => Some(3), 3 => Some(5), 4 => Some(6),
            5 => Some(7), 6 => Some(8), 7 => Some(9), 8 => Some(11), 9 => Some(15),
            10 => Some(10), 40 => Some(4), 50 => Some(1),
            _ => None,
        };
        if let Some(index) = reference_index {
            return NativeAliasValue::Reference { reference: NativeReferenceKey { graph: input.graph, reference: index }, input };
        }
        match original.index {
            20 => NativeAliasValue::Allocation,
            21..=23 => NativeAliasValue::NotTable,
            30..=32 => NativeAliasValue::Function { function: function(match original.index { 30 => 3, 31 => 1, _ => 2 }), creation: at(0, 0) },
            100 | 101 | 102 | 104 | 105 => self.alias_call_source(original, input).map_or(NativeAliasValue::Unknown, |call| NativeAliasValue::Call { callee: call.callee.value, input: call.callee.input }),
            _ => NativeAliasValue::Unknown,
        }
    }

    fn value_alias_environment_member(&self, original: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasEnvironmentMember> {
        let (receiver, member, lookup) = match original.index {
            60 => (1, 0, input),
            61 => (20, 0, input), // An ordinary allocation with the same slot candidate.
            62 => (7, 0, input), // A foreign environment cannot attest this binding.
            63 => (1, 15, input), // A candidate from the wrong environment.
            64 => (3, 0, input), // A callable value is not the environment object.
            65 => (1, 0, at(0, 0)), // A saved read precedes the later replacement.
            66 => (1, 0, at(0, 4)), // Receiver freezes first; lookup follows the write.
            67 => (67, 0, input), // Incomplete receiver provenance must stay pending.
            _ => return None,
        };
        Some(NativeAliasEnvironmentMember {
            receiver: NativeAliasReturnValue { value: value(0, receiver), input: at(0, 0) },
            reference: reference(0, member),
            input: lookup,
        })
    }

    fn alias_call_source(&self, original: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasCall> {
        let (callee, callee_input, arguments) = match original.index {
            100 => (value(0, 0), at(0, 0), vec![
                NativeAliasReturnValue { value: value(0, 20), input: at(0, 0) },
                NativeAliasReturnValue { value: value(0, 21), input: at(0, 0) },
            ]),
            101 => (value(0, 31), input, vec![]),
            102 => (value(0, 32), input, vec![]),
            104 => (value(0, 40), input, vec![NativeAliasReturnValue { value: value(0, 22), input }]),
            105 => (value(4, 0), at(4, 0), vec![
                NativeAliasReturnValue { value: value(4, 20), input: at(4, 0) },
                NativeAliasReturnValue { value: value(4, 21), input: at(4, 0) },
            ]),
            _ => return None,
        };
        Some(NativeAliasCall {
            callee: NativeAliasReturnValue { value: callee, input: callee_input },
            arguments,
            execution_input: input,
        })
    }

    fn alias_node_dispatch(&self, original: NativeFlowKey) -> NativeAliasDispatch {
        if self.node(original).is_some() { NativeAliasDispatch::NoDispatch }
        else { NativeAliasDispatch::Unknown }
    }
    fn function_alias_execution(&self, original: NativeFunctionKey, creation: NativeFlowKey) -> Option<NativeAliasExecution> {
        if creation != at(0, 0) { return None; }
        match original.index {
            1 | 2 if original.snapshot == SNAPSHOT => Some(Self::execution(original.index)),
            _ => None,
        }
    }
    fn require_alias_execution(&self, original: NativeValueKey, _: NativeFlowKey) -> Option<Vec<NativeAliasExecution>> {
        (original == value(0, 104)).then_some(vec![Self::execution(4)])
    }
    fn require_alias_loader_reference(&self, original: NativeValueKey, input: NativeFlowKey) -> Option<NativeReferenceKey> {
        (self.original_loader && original == value(0, 104)).then_some(NativeReferenceKey { graph: input.graph, reference: 14 })
    }
    fn reference_is_cell(&self, original: NativeReferenceKey) -> bool {
        matches!(original.reference, 1 | 12..=14)
    }
    fn reference_declaration(&self, original: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        let (owner, writes) = match original.reference {
            0 if original.graph == graph(4) => (graph(4), vec![]),
            1 => (graph(0), vec![NativeVariableWrite { owner: graph(2), statement_end: 1 }]),
            12 => (graph(0), vec![]),
            13 if original.graph == graph(1) => (graph(1), vec![]),
            14 => (graph(0), vec![NativeVariableWrite { owner: graph(0), statement_end: 9 }]),
            _ => return None,
        };
        Some(NativeVariableDeclaration { owner, reference: NativeReferenceKey { graph: owner, reference: original.reference }, writes })
    }
    fn reference_in_graph(&self, original: NativeReferenceKey, target: NativeGraphKey) -> Option<NativeReferenceKey> {
        let projected = NativeReferenceKey { graph: target, reference: original.reference };
        (self.contains_reference(original) && self.contains_reference(projected)).then_some(projected)
    }
    fn contains_graph(&self, original: NativeGraphKey) -> bool {
        self.nodes.contains_key(&NativeFlowKey { graph: original, node: 0 })
    }
    fn contains_reference(&self, original: NativeReferenceKey) -> bool {
        self.contains_graph(original.graph) && original.reference < 16 && (original.reference != 13 || original.graph == graph(1))
    }
    fn node(&self, original: NativeFlowKey) -> Option<NativeFlowNode> { self.nodes.get(&original).cloned() }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn write_affects(&self, left: NativeReferenceKey, right: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if left == right { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
    }
    fn value_type(&self, original: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        // The shared flow worker checks this original LoaderState sentinel;
        // no expression or callable inference is exposed to origin proofs.
        if original == value(0, 23) { return Type::BooleanLiteral(false); }
        panic!("original standard value identity must not be borrowed from expression types")
    }
    fn call_effects(&self, original: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects {
        // These original calls return normally. In particular, the captured
        // writer 102 has no reachability effect while still failing the
        // independent call-preservation proof for the value that it mutates.
        if [100, 101, 102, 104].iter().any(|&index| original == value(0, index)) {
            return NativeCallEffects::None;
        }
        panic!("unexpected reachability demand outside the original loader flow")
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("invalid original test graph: {error:?}") }
}

fn not_preserved(result: NativeCallPreservation) {
    assert!(matches!(result, NativeCallPreservation::Unknown | NativeCallPreservation::Pending), "{result:?}");
}

#[test]
fn original_global_value_and_environment_metadata_attest_the_selected_standard_operation() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || StandardRecipes::new(routed), |recipes, queries| {
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 0), at(0, 0)), StandardRecipes::selected());
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 10), at(0, 0)), StandardRecipes::selected());
            assert_eq!(queries.flow_alias_relation(recipes, reference(0, 0), at(0, 0), value(0, 10), at(0, 0)), NativeAliasRelation::Same);
            assert_eq!(queries.flow_alias_relation(recipes, reference(0, 2), at(0, 0), value(0, 2), at(0, 0)), NativeAliasRelation::Same);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 1), at(0, 0)), NativeAliasIntrinsic::Unknown);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 2), at(0, 0)), NativeAliasIntrinsic::Unknown);
        });
    }
}

#[test]
fn written_callable_heads_and_forged_standard_membership_do_not_attest_runtime_identity() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || StandardRecipes::new(routed), |recipes, queries| {
            // Public heads exist for the ordinary callable, but none carry an
            // original standard-value attestation; its structural shape is irrelevant.
            assert_eq!(recipes.value_declaration_signatures(declaration(5)), Some(vec![signature(5)]));
            for index in 3..=9 {
                assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, index), at(0, 0)), NativeAliasIntrinsic::Unknown, "untrusted original recipe {index}");
            }
            assert_ne!(queries.flow_alias_relation(recipes, reference(0, 2), at(0, 0), value(0, 7), at(0, 0)), NativeAliasRelation::Same);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 0), at(0, 0)), StandardRecipes::selected());
        });
    }
}

#[test]
fn fixed_member_identity_requires_the_actual_environment_and_uses_the_lookup_epoch() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || StandardRecipes::new(routed), |recipes, queries| {
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 60), at(0, 0)), StandardRecipes::selected());
            for index in 61..=64 {
                assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, index), at(0, 0)), NativeAliasIntrinsic::Unknown, "unproven receiver/candidate {index}");
            }
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 60), at(0, 4)), NativeAliasIntrinsic::Unknown);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 65), at(0, 4)), StandardRecipes::selected());
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 66), at(0, 4)), NativeAliasIntrinsic::Unknown);
        });
    }
}

#[test]
fn fixed_member_identity_does_not_cache_incomplete_receiver_provenance_as_absent() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || StandardRecipes::new(routed), |recipes, queries| {
            for _ in 0..2 {
                assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 67), at(0, 0)), NativeAliasIntrinsic::Pending);
            }
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 60), at(0, 0)), StandardRecipes::selected());
        });
    }
}

#[test]
fn a_loop_provenance_barrier_is_completed_unknown_without_attesting_a_standard_operation() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut source = StandardRecipes::new(routed);
            source.nodes.insert(at(0, 11), NativeFlowNode::Loop { edges: vec![at(0, 1), at(0, 11)] });
            source
        }, |source, queries| {
            for _ in 0..2 {
                // The original standard value and its saved mutable copy
                // cannot be identified across the conservative loop barrier.
                // That completed lack of proof must not poison reachability
                // with a Pending result, or borrow a callable type instead.
                assert_eq!(queries.flow_alias_intrinsic(source, value(0, 0), at(0, 11)), NativeAliasIntrinsic::Unknown);
                assert_eq!(queries.flow_alias_intrinsic(source, value(0, 50), at(0, 11)), NativeAliasIntrinsic::Unknown);
                assert_eq!(queries.flow_alias_relation(source, reference(0, 0), at(0, 11), value(0, 0), at(0, 0)), NativeAliasRelation::Possible);
            }
            // Retain the earlier frozen read and keep genuine recursive
            // metadata Pending; neither query may poison the other's memo.
            assert_eq!(queries.flow_alias_intrinsic(source, value(0, 50), at(0, 1)), StandardRecipes::selected());
            assert_eq!(queries.flow_alias_intrinsic(source, value(0, 67), at(0, 0)), NativeAliasIntrinsic::Pending);
            assert_eq!(queries.flow_alias_intrinsic(source, value(0, 0), at(0, 11)), NativeAliasIntrinsic::Unknown);
        });
    }
}

#[test]
fn future_implementation_owners_do_not_rewrite_the_initial_standard_origin() {
    for routed in [false, true] {
        for owner in [6, 3] {
            Engine::new().with_native_provider(move || {
                let mut source = StandardRecipes::new(routed);
                source.extra_owner = Some(declaration(owner));
                source
            }, |source, queries| {
                let initial = queries.flow_alias_intrinsic(source, value(0, 0), at(0, 0));
                let copy = queries.flow_alias_intrinsic(source, value(0, 50), at(0, 1));
                if owner == 6 {
                    assert_eq!(initial, StandardRecipes::selected());
                    assert_eq!(copy, StandardRecipes::selected());
                    assert_eq!(queries.flow_alias_intrinsic(source, value(0, 0), at(0, 4)), NativeAliasIntrinsic::Unknown);
                } else {
                    assert_eq!(initial, NativeAliasIntrinsic::Unknown);
                    assert_eq!(copy, NativeAliasIntrinsic::Unknown);
                }
            });
        }
    }
}

#[test]
fn original_long_form_heads_do_not_turn_a_runtime_global_into_a_bodyless_declaration() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || {
            let mut source = StandardRecipes::new(routed);
            source.long_form = true;
            source.extra_owner = Some(declaration(6));
            source
        }, |source, queries| {
            assert_eq!(source.value_declaration_signatures(declaration(0)), None);
            assert_eq!(source.value_declaration_function_signatures(declaration(0)), Some(vec![signature(0), signature(9)]));
            assert_eq!(queries.flow_alias_intrinsic(source, value(0, 0), at(0, 0)), StandardRecipes::selected());
            assert_eq!(queries.flow_alias_intrinsic(source, value(0, 50), at(0, 1)), StandardRecipes::selected());
            assert_eq!(queries.flow_alias_intrinsic(source, value(0, 0), at(0, 4)), NativeAliasIntrinsic::Unknown);
        });
    }
}

#[test]
fn selected_standard_setmetatable_preserves_the_actual_argument_allocation() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || StandardRecipes::new(routed), |recipes, queries| {
            assert_eq!(queries.flow_call_preservation(recipes, reference(0, 0), value(0, 100), at(0, 6)), NativeCallPreservation::Preserved);
            assert_eq!(queries.flow_alias_relation(recipes, reference(0, 12), at(0, 6), value(0, 100), at(0, 6)), NativeAliasRelation::Same);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 100), at(0, 6)), NativeAliasIntrinsic::Unknown);
        });
    }
}

#[test]
fn copied_standard_value_survives_a_pure_call_but_not_an_executed_captured_write() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || StandardRecipes::new(routed), |recipes, queries| {
            for node in [1, 2] {
                assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 50), at(0, node)), StandardRecipes::selected());
            }
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 50), at(0, 3)), NativeAliasIntrinsic::Unknown);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 50), at(0, 2)), StandardRecipes::selected());
        });
    }
}

#[test]
fn imported_execution_inherits_the_caller_environment_instead_of_reseeding_standard_values() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || StandardRecipes::new(routed), |recipes, queries| {
            assert_eq!(recipes.reference_declaration(reference(4, 0)).unwrap().owner, graph(4));
            assert!(!recipes.reference_is_cell(reference(4, 0)));
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 0), at(0, 1)), StandardRecipes::selected());
            assert_eq!(queries.flow_call_preservation(recipes, reference(0, 0), value(0, 104), at(0, 1)), NativeCallPreservation::Preserved);
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 0), at(0, 4)), NativeAliasIntrinsic::Unknown);
            not_preserved(queries.flow_call_preservation(recipes, reference(0, 0), value(0, 104), at(0, 4)));
            assert_eq!(queries.flow_call_preservation(recipes, reference(0, 0), value(0, 104), at(0, 1)), NativeCallPreservation::Preserved);
        });
    }
}

#[test]
fn genuine_standard_require_does_not_certify_a_replaced_or_unattested_loader() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || StandardRecipes::new(routed), |recipes, queries| {
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 40), at(0, 9)), NativeAliasIntrinsic::Standard(site(2, NativeIntrinsicKind::Require)));
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 0), at(0, 9)), StandardRecipes::selected());
            not_preserved(queries.flow_call_preservation(recipes, reference(0, 0), value(0, 104), at(0, 9)));
            assert_eq!(queries.flow_call_preservation(recipes, reference(0, 0), value(0, 104), at(0, 1)), NativeCallPreservation::Preserved);
        });
        Engine::new().with_native_provider(move || {
            let mut recipes = StandardRecipes::new(routed);
            recipes.original_loader = false;
            recipes
        }, |recipes, queries| {
            assert_eq!(queries.flow_alias_intrinsic(recipes, value(0, 40), at(0, 1)), NativeAliasIntrinsic::Standard(site(2, NativeIntrinsicKind::Require)));
            not_preserved(queries.flow_call_preservation(recipes, reference(0, 0), value(0, 104), at(0, 1)));
        });
    }
}
