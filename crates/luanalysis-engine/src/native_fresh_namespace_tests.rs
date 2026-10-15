use crate::*;
use std::collections::{HashMap, HashSet};

const SNAPSHOT: u64 = 8_150;

fn graph(region: u32) -> NativeGraphKey {
    NativeGraphKey { snapshot: SNAPSHOT, region, context: 0 }
}
fn at(region: u32, node: u32) -> NativeFlowKey { NativeFlowKey { graph: graph(region), node } }
fn reference(region: u32, reference: u32) -> NativeReferenceKey { NativeReferenceKey { graph: graph(region), reference } }
fn value(index: u32) -> NativeValueKey { NativeValueKey { snapshot: SNAPSHOT, index } }
fn function() -> NativeFunctionKey { NativeFunctionKey { snapshot: SNAPSHOT, index: 0 } }
fn environment() -> NativeEnvironmentKey { NativeEnvironmentKey { snapshot: 8_151, index: 0 } }
fn binding() -> NativeGlobalBindingKey { NativeGlobalBindingKey { environment: environment(), index: 0 } }
fn declaration() -> NativeValueDeclarationKey { NativeValueDeclarationKey { snapshot: 8_152, index: 0 } }
fn signature() -> NativeSignatureKey { NativeSignatureKey { snapshot: 8_152, index: 0 } }
fn standard() -> NativeIntrinsicSite {
    NativeIntrinsicSite { binding: binding(), declaration: declaration(), kind: NativeIntrinsicKind::SetMetatable }
}

#[derive(Clone, Copy, Debug)]
enum Scenario { Single, Multiple, MultipleReceivers, RawRead, Captured, CapturedMetatable, NonNilRawSlot, NilRawSlot, MetatableOnClass, MetatableBeforeClass, UnknownPass, ForeignAllocation, Loop }

/// Original execution recipes for a factory with `local Class = {}` and
/// function-literal field stores. Heap stores deliberately remain dispatch
/// barriers: their receivers, rather than a fabricated NoDispatch annotation,
/// must establish the absence of a metatable.
struct Namespace {
    routed: bool,
    scenario: Scenario,
    nodes: HashMap<NativeFlowKey, NativeFlowNode>,
    receivers: HashMap<NativeFlowKey, NativeAliasReturnValue>,
    method_targets: HashMap<NativeFlowKey, NativeReferenceKey>,
    unknown: HashSet<NativeFlowKey>,
    reads: HashMap<NativeFlowKey, NativeReferenceKey>,
    standard_calls: HashMap<NativeValueKey, NativeAliasCall>,
    terminal: NativeFlowKey,
    current: NativeFlowKey,
    next: u32,
}

impl Namespace {
    fn new(scenario: Scenario, routed: bool) -> Self {
        let mut source = Self {
            routed, scenario,
            nodes: HashMap::from([(at(0, 0), NativeFlowNode::Start), (at(1, 0), NativeFlowNode::Start)]),
            receivers: HashMap::new(), method_targets: HashMap::new(), unknown: HashSet::new(), reads: HashMap::new(),
            standard_calls: HashMap::new(), terminal: at(1, 0), current: at(1, 0), next: 1,
        };
        if matches!(scenario, Scenario::ForeignAllocation) {
            source.nodes.insert(at(2, 0), NativeFlowNode::Start);
            source.nodes.insert(at(2, 1), NativeFlowNode::Write {
                before: at(2, 0), target: reference(2, 1), value: value(2), input: at(2, 0),
            });
        }
        if matches!(scenario, Scenario::MetatableBeforeClass) {
            source.local(4, 2);
            source.install_metatable(12);
        }
        if source.captured() {
            source.nodes.insert(at(0, 1), NativeFlowNode::Write { before: at(0, 0), target: reference(0, 1), value: value(1), input: at(0, 0) });
            if matches!(scenario, Scenario::CapturedMetatable) {
                source.standard_calls.insert(value(100), NativeAliasCall {
                    callee: NativeAliasReturnValue { value: value(0), input: at(0, 1) },
                    arguments: vec![NativeAliasReturnValue { value: value(11), input: at(0, 1) }, NativeAliasReturnValue { value: value(50), input: at(0, 1) }],
                    execution_input: at(0, 1),
                });
                source.nodes.insert(at(0, 2), NativeFlowNode::Call { before: at(0, 1), value: value(100), input: at(0, 1) });
            }
        } else { source.local(1, 1); }
        match scenario {
            Scenario::MetatableOnClass | Scenario::NonNilRawSlot | Scenario::NilRawSlot => source.install_metatable(11),
            Scenario::MultipleReceivers => {
                source.local(4, 2);
                let before = source.current;
                let left = source.push(NativeFlowNode::Write { before, target: reference(1, 5), value: value(13), input: before });
                let right = source.push(NativeFlowNode::Write { before, target: reference(1, 5), value: value(12), input: before });
                source.push(NativeFlowNode::Merge { edges: vec![left, right] });
            },
            Scenario::RawRead => {
                let read = source.push(NativeFlowNode::Pass { before: source.current });
                source.reads.insert(read, reference(1, 2));
            },
            Scenario::UnknownPass => {
                let barrier = source.push(NativeFlowNode::Pass { before: source.current });
                source.unknown.insert(barrier);
            },
            Scenario::Loop => {
                let header = at(1, source.next);
                let back = at(1, source.next + 1);
                source.push(NativeFlowNode::Loop { edges: vec![source.current, back] });
                source.push(NativeFlowNode::Pass { before: header });
                source.current = header;
            },
            _ => {},
        }
        source.method(if matches!(scenario, Scenario::RawRead) { 3 } else { 2 });
        if matches!(scenario, Scenario::Multiple | Scenario::MultipleReceivers) { source.method(3); }
        source.terminal = source.push(NativeFlowNode::Return { before: source.current });
        source
    }

    fn push(&mut self, recipe: NativeFlowNode) -> NativeFlowKey {
        let at = at(1, self.next);
        self.next += 1;
        self.nodes.insert(at, recipe);
        self.current = at;
        at
    }

    fn local(&mut self, target: u32, rhs: u32) {
        self.push(NativeFlowNode::Write {
            before: self.current, target: reference(1, target), value: value(rhs), input: self.current,
        });
    }

    fn method(&mut self, target: u32) {
        let receiver_input = self.current;
        // Evaluating the RHS function literal creates a closure without
        // executing its body. No field-value origin is supplied by this mock.
        self.push(NativeFlowNode::Pass { before: self.current });
        let target = reference(1, target);
        let store = self.push(NativeFlowNode::Write {
            before: self.current, target, value: value(20), input: receiver_input,
        });
        self.method_targets.insert(store, target);
        self.receivers.insert(store, NativeAliasReturnValue { value: value(11), input: receiver_input });
    }

    fn install_metatable(&mut self, receiver: u32) {
        let input = self.current;
        self.standard_calls.insert(value(100), NativeAliasCall {
            callee: NativeAliasReturnValue { value: value(0), input },
            arguments: vec![
                NativeAliasReturnValue { value: value(receiver), input },
                // The original metatable contains an __newindex closure;
                // its field-value origin is deliberately not supplied.
                NativeAliasReturnValue { value: value(50), input },
            ],
            execution_input: input,
        });
        self.push(NativeFlowNode::Call { before: input, value: value(100), input });
    }

    fn preserve(&self, queries: &mut dyn TypeQueries) -> NativeCallPreservation {
        let input = if matches!(self.scenario, Scenario::CapturedMetatable) { at(0, 2) } else { self.creation() };
        queries.flow_call_preservation(self, reference(0, 0), value(200), input)
    }
    fn captured(&self) -> bool { matches!(self.scenario, Scenario::Captured | Scenario::CapturedMetatable) }
    fn creation(&self) -> NativeFlowKey { if self.captured() { at(0, 1) } else { at(0, 0) } }
}

impl NativeTypeProvider for Namespace {
    fn project_flow_provider(&self) -> Option<&dyn NativeFlowProvider> {
        self.routed.then_some(self as &dyn NativeFlowProvider)
    }
    fn contains_function(&self, original: NativeFunctionKey) -> bool { original == function() }
    fn contains_global_environment(&self, original: NativeEnvironmentKey) -> bool { original == environment() }
    fn contains_global_binding(&self, original: NativeGlobalBindingKey) -> bool { original == binding() }
    fn global_binding_declarations(&self, _: NativeGlobalBindingKey) -> Vec<NativeValueDeclarationKey> { vec![declaration()] }
    fn contains_value_declaration(&self, original: NativeValueDeclarationKey) -> bool { original == declaration() }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader { role: GlobalSourceRole::Declaration, kind: GlobalValueKind::Function }
    }
    fn value_declaration_signatures(&self, _: NativeValueDeclarationKey) -> Option<Vec<NativeSignatureKey>> { Some(vec![signature()]) }
    fn contains_signature(&self, original: NativeSignatureKey) -> bool { original == signature() }
    fn signature_intrinsic(&self, original: NativeSignatureKey) -> Option<NativeIntrinsicSite> { (original == signature()).then_some(standard()) }

    fn value_declaration_contract(&self, _: NativeValueDeclarationKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("fresh-namespace proof must not inspect a global callable contract")
    }
    fn value_declaration_initializer(&self, _: NativeValueDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        panic!("fresh-namespace proof must not inspect an ambient body")
    }
    fn signature_header(&self, _: NativeSignatureKey) -> SignatureHeader { panic!("no signature-shaped proof") }
    fn signature_parameters(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("no signature-shaped proof") }
    fn signature_declared_return_pack(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> ValuePack { panic!("no return-shaped proof") }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("no function-type inference") }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { panic!("no return-shaped proof") }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { panic!("no body inference") }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("no return inference") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("no return inference") }
}

impl NativeFlowProvider for Namespace {
    fn reference_alias_initial(&self, original: NativeReferenceKey) -> NativeAliasInitial {
        if original.reference == 0 { NativeAliasInitial::GlobalValue(binding()) } else { NativeAliasInitial::Opaque }
    }
    fn value_alias_source(&self, original: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        match original.index {
            0 => NativeAliasValue::Reference { reference: NativeReferenceKey { graph: input.graph, reference: 0 }, input },
            1 if matches!(self.scenario, Scenario::ForeignAllocation) => NativeAliasValue::Reference { reference: reference(2, 1), input: at(2, 1) },
            1 | 2 | 50 => NativeAliasValue::Allocation,
            10 => NativeAliasValue::Function { function: function(), creation: self.creation() },
            11 | 12 | 13 => NativeAliasValue::Reference { reference: NativeReferenceKey { graph: input.graph, reference: match original.index { 11 if matches!(self.scenario, Scenario::MultipleReceivers) => 5, 11 | 13 => 1, _ => 4 } }, input },
            40 => NativeAliasValue::NonNilScalar,
            41 => NativeAliasValue::NotTable,
            // The method literal remains deliberately untracked. Its value
            // cannot be used as evidence that a missing target field is raw.
            _ => NativeAliasValue::Unknown,
        }
    }
    fn alias_call_source(&self, original: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasCall> {
        if original == value(200) {
            return Some(NativeAliasCall {
                callee: NativeAliasReturnValue { value: value(10), input: self.creation() },
                arguments: vec![], execution_input: input,
            });
        }
        self.standard_calls.get(&original).filter(|recipe| recipe.execution_input == input).cloned()
    }
    fn function_alias_execution(&self, original: NativeFunctionKey, creation: NativeFlowKey) -> Option<NativeAliasExecution> {
        (original == function() && creation == self.creation()).then_some(NativeAliasExecution { entry: at(1, 0), terminals: vec![self.terminal] })
    }
    fn alias_node_dispatch(&self, original: NativeFlowKey) -> NativeAliasDispatch {
        if let Some(&target) = self.method_targets.get(&original) { return NativeAliasDispatch::ReferenceRead(target); }
        if let Some(&reference) = self.reads.get(&original) { return NativeAliasDispatch::ReferenceRead(reference); }
        if self.unknown.contains(&original) { return NativeAliasDispatch::Unknown; }
        match self.node(original) {
            // Original control-flow merges execute no Lua operation. Match the
            // source adapter instead of introducing an implicit dispatch here.
            Some(NativeFlowNode::Start | NativeFlowNode::Pass { .. } | NativeFlowNode::Merge { .. } | NativeFlowNode::Return { .. }
                | NativeFlowNode::Loop { .. }) => NativeAliasDispatch::NoDispatch,
            Some(NativeFlowNode::Write { target, .. }) if self.reference_is_cell(target) => NativeAliasDispatch::NoDispatch,
            _ => NativeAliasDispatch::Unknown,
        }
    }
    fn write_alias_receiver(&self, original: NativeFlowKey) -> Option<NativeAliasReturnValue> { self.receivers.get(&original).copied() }
    fn value_alias_member_source(&self, original: NativeValueKey, input: NativeFlowKey, key: &LuaKey) -> Option<NativeAliasReturnValue> {
        if original != value(1) || *key != LuaKey::String(b"first".to_vec()) { return None; }
        let source = match self.scenario { Scenario::NonNilRawSlot | Scenario::RawRead => 40, Scenario::NilRawSlot => 41, _ => return None };
        Some(NativeAliasReturnValue { value: value(source), input })
    }
    fn reference_member_path(&self, original: NativeReferenceKey, member: NativeReferenceKey) -> Option<Vec<LuaKey>> {
        let owner = if matches!(self.scenario, Scenario::MultipleReceivers) { 5 } else { 1 };
        if original.graph != member.graph || original.reference != owner { return None; }
        Some(vec![LuaKey::String(match member.reference { 2 => b"first".to_vec(), 3 => b"second".to_vec(), _ => return None })])
    }
    fn reference_is_cell(&self, original: NativeReferenceKey) -> bool { matches!(original.reference, 1 | 4 | 5) }
    fn reference_declaration(&self, original: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        let owner = if self.captured() && original.reference == 1 { graph(0) } else { original.graph };
        self.reference_is_cell(original).then_some(NativeVariableDeclaration { owner, reference: original, writes: vec![] })
    }
    fn reference_in_graph(&self, original: NativeReferenceKey, target: NativeGraphKey) -> Option<NativeReferenceKey> {
        if original.graph != target && original.reference != 0 && !(self.captured() && original.reference <= 3) { return None; }
        let projected = NativeReferenceKey { graph: target, reference: original.reference };
        (self.contains_reference(original) && self.contains_reference(projected)).then_some(projected)
    }
    fn contains_graph(&self, original: NativeGraphKey) -> bool { self.nodes.contains_key(&NativeFlowKey { graph: original, node: 0 }) }
    fn contains_reference(&self, original: NativeReferenceKey) -> bool {
        self.contains_graph(original.graph) && match original.graph.region {
            0 => original.reference == 0 || self.captured() && original.reference <= 3,
            1 => original.reference < 6,
            2 => matches!(original.reference, 0 | 1),
            _ => false,
        }
    }
    fn node(&self, original: NativeFlowKey) -> Option<NativeFlowNode> { self.nodes.get(&original).cloned() }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn write_affects(&self, query: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if query == target { NativeWriteRelation::Exact }
        else if self.reference_member_path(target, query).is_some() { NativeWriteRelation::Ancestor }
        else if self.reference_member_path(query, target).is_some() { NativeWriteRelation::Descendant }
        else { NativeWriteRelation::Unaffected }
    }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type { panic!("fresh allocation evidence must not inspect expression types") }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { panic!("normal-return effects cannot establish a missing metatable") }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("invalid original namespace fixture: {error:?}") }
}

#[test]
fn fresh_namespace_allows_one_or_multiple_original_method_stores_without_type_inference() {
    for routed in [false, true] {
        for scenario in [Scenario::Single, Scenario::Multiple] {
            Engine::new().with_native_provider(move || Namespace::new(scenario, routed), |source, queries| {
                assert!(source.method_targets.keys().all(|&at| matches!(source.alias_node_dispatch(at), NativeAliasDispatch::ReferenceRead(_))));
                assert_eq!(source.preserve(queries), NativeCallPreservation::Preserved, "{scenario:?}");
                assert_eq!(source.preserve(queries), NativeCallPreservation::Preserved, "completed certificate can be reused");
            });
        }
    }
}

#[test]
fn standard_setmetatable_on_the_same_class_blocks_missing_field_dispatch_proof() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || Namespace::new(Scenario::MetatableOnClass, routed), |source, queries| {
            let call = source.standard_calls.get(&value(100)).unwrap();
            assert_eq!(queries.flow_alias_intrinsic(source, call.callee.value, call.callee.input), NativeAliasIntrinsic::Standard(standard()));
            assert_eq!(queries.flow_call_preservation(source, reference(1, 0), value(100), call.execution_input), NativeCallPreservation::Preserved,
                "preserving a raw slot does not prove absence of a newly installed __newindex");
            assert_eq!(source.preserve(queries), NativeCallPreservation::Unknown);
        });
    }
}

#[test]
fn standard_setmetatable_on_another_object_before_class_allocation_does_not_taint_the_new_table() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || Namespace::new(Scenario::MetatableBeforeClass, routed), |source, queries| {
            assert_eq!(source.preserve(queries), NativeCallPreservation::Preserved);
        });
    }
}

#[test]
fn missing_field_dispatch_proof_refuses_unknown_operations_foreign_allocations_and_loops() {
    for routed in [false, true] {
        for scenario in [Scenario::UnknownPass, Scenario::ForeignAllocation, Scenario::Loop] {
            Engine::new().with_native_provider(move || Namespace::new(scenario, routed), |source, queries| {
                let expected = if matches!(scenario, Scenario::Loop) { NativeCallPreservation::Pending } else { NativeCallPreservation::Unknown };
                assert_eq!(source.preserve(queries), expected, "{scenario:?}");
            });
        }
    }
}

#[test]
fn present_primitive_raw_slots_do_not_dispatch_but_nil_slots_still_do() {
    for routed in [false, true] {
        for scenario in [Scenario::NonNilRawSlot, Scenario::NilRawSlot] {
            Engine::new().with_native_provider(move || Namespace::new(scenario, routed), |source, queries| {
                let expected = if matches!(scenario, Scenario::NonNilRawSlot) { NativeCallPreservation::Preserved } else { NativeCallPreservation::Unknown };
                assert_eq!(source.preserve(queries), expected, "{scenario:?}");
            });
        }
    }
}

#[test]
fn multiple_fresh_receivers_can_all_be_raw_without_becoming_one_object() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || Namespace::new(Scenario::MultipleReceivers, routed), |source, queries| {
            assert_eq!(source.preserve(queries), NativeCallPreservation::Preserved);
            let NativeFlowNode::Return { before } = source.node(source.terminal).unwrap() else { unreachable!() };
            assert_eq!(queries.flow_alias_relation(source, reference(1, 1), before, value(11), before), NativeAliasRelation::Possible);
        });
    }
}

#[test]
fn a_proven_raw_read_preserves_metatable_absence_for_a_later_missing_slot_store() {
    for routed in [false, true] {
        Engine::new().with_native_provider(move || Namespace::new(Scenario::RawRead, routed), |source, queries| {
            assert_eq!(source.preserve(queries), NativeCallPreservation::Preserved);
        });
    }
}

#[test]
fn captured_fresh_tables_use_actual_execution_entry_without_ignoring_prior_setmetatable() {
    for routed in [false, true] {
        for scenario in [Scenario::Captured, Scenario::CapturedMetatable] {
            Engine::new().with_native_provider(move || Namespace::new(scenario, routed), |source, queries| {
                let expected = if matches!(scenario, Scenario::Captured) { NativeCallPreservation::Preserved } else { NativeCallPreservation::Unknown };
                assert_eq!(source.preserve(queries), expected, "{scenario:?}");
            });
        }
    }
}
