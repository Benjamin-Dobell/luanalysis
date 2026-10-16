use super::*;
use std::cell::{Cell, RefCell};

const SNAPSHOT: u64 = 12_043;
fn environment() -> NativeEnvironmentKey {
    NativeEnvironmentKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn binding() -> NativeGlobalBindingKey {
    NativeGlobalBindingKey {
        environment: environment(),
        index: 0,
    }
}
fn declaration() -> NativeValueDeclarationKey {
    NativeValueDeclarationKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn signature(index: u32) -> NativeSignatureKey {
    NativeSignatureKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn function() -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn graph(context: u64) -> NativeGraphKey {
    NativeGraphKey {
        snapshot: SNAPSHOT,
        region: 0,
        context,
    }
}
fn at(context: u64, node: u32) -> NativeFlowKey {
    NativeFlowKey {
        graph: graph(context),
        node,
    }
}
fn reference(context: u64) -> NativeReferenceKey {
    NativeReferenceKey {
        graph: graph(context),
        reference: 0,
    }
}
fn value(index: u32) -> NativeValueKey {
    NativeValueKey {
        snapshot: SNAPSHOT,
        index,
    }
}

#[derive(Clone, Copy)]
enum Condition {
    Truthy,
    NotNil,
    Number,
    Not,
    And,
    False,
    Nil,
    Unknown,
    NoArguments,
}
#[derive(Clone, Copy)]
enum Callee {
    Original,
    Alias,
    Shape,
    RecursiveUntyped,
}
struct Fixture {
    condition: Condition,
    callee: Callee,
    trusted: bool,
    foreign_effect: bool,
    body_queries: Cell<usize>,
    written_result_queries: Cell<usize>,
    value_queries: Cell<usize>,
    errors: RefCell<Vec<NativeFlowError>>,
}
impl Fixture {
    fn new(condition: Condition) -> Self {
        Self {
            condition,
            callee: Callee::Original,
            trusted: true,
            foreign_effect: false,
            body_queries: Cell::new(0),
            written_result_queries: Cell::new(0),
            value_queries: Cell::new(0),
            errors: RefCell::new(vec![]),
        }
    }
    fn initial(context: u64) -> Type {
        Type::Union(vec![
            if context == 2 {
                Type::String
            } else {
                Type::Number
            },
            Type::Nil,
            Type::BooleanLiteral(false),
        ])
    }
    fn predicate(&self, input: NativeFlowKey) -> Option<NativePredicate> {
        let truthy = NativePredicate::Truthy {
            subject: Some(reference(input.graph.context)),
            value: value(0),
            input,
        };
        let number = NativePredicate::RuntimeType {
            subject: reference(input.graph.context),
            value: value(0),
            input,
            kind: NativeRuntimeType::Number,
            equals: true,
        };
        Some(match self.condition {
            Condition::Truthy => truthy,
            Condition::NotNil => NativePredicate::Equal {
                subject: reference(input.graph.context),
                other: value(1),
                input,
                equals: false,
            },
            Condition::Number => number,
            Condition::Not => NativePredicate::Not(Box::new(truthy)),
            Condition::And => NativePredicate::And(Box::new(truthy), Box::new(number)),
            Condition::False => NativePredicate::Truthy {
                subject: None,
                value: value(2),
                input,
            },
            Condition::Nil => NativePredicate::Truthy {
                subject: None,
                value: value(1),
                input,
            },
            Condition::Unknown | Condition::NoArguments => return None,
        })
    }
}
impl NativeTypeProvider for Fixture {
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool {
        key == environment()
    }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool {
        key == binding()
    }
    fn global_binding_declarations(
        &self,
        _: NativeGlobalBindingKey,
    ) -> Vec<NativeValueDeclarationKey> {
        vec![declaration()]
    }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool {
        key == declaration()
    }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader {
            role: GlobalSourceRole::Declaration,
            kind: GlobalValueKind::Function,
        }
    }
    fn value_declaration_contract(
        &self,
        _: NativeValueDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        Some(queries.signature_group_type(&[signature(0)]))
    }
    fn value_declaration_initializer(
        &self,
        _: NativeValueDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        panic!("declaration stub must not be evaluated for flow effects")
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn signature_intrinsic(&self, key: NativeSignatureKey) -> Option<NativeIntrinsicSite> {
        (self.trusted && key == signature(0)).then_some(NativeIntrinsicSite {
            binding: binding(),
            declaration: declaration(),
            kind: NativeIntrinsicKind::Assert,
        })
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::Standalone,
            site: SignatureSite {
                snapshot: SNAPSHOT,
                index: key.index,
            },
            parent: None,
            has_direct_literal_parameter: false,
        }
    }
    fn signature_parameters(
        &self,
        _: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        vec![Parameter {
            name: "value".into(),
            ty: Type::Unknown,
            optional: true,
        }]
    }
    fn signature_rest_pack(
        &self,
        _: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        Some(ValuePack::variadic(Type::Unknown))
    }
    fn signature_declared_return_pack(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        assert!(
            !self.trusted && key == signature(0),
            "intrinsic metadata and trusted assertion flow must not query a written result"
        );
        self.written_result_queries
            .set(self.written_result_queries.get() + 1);
        // Without intrinsic trust this remains an ordinary written callable
        // contract. Reading it for a nonreturning effect must remain pure.
        ValuePack::single(Type::Unknown)
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == SNAPSHOT && key.index == 0
    }
    fn has_type_parameter_constraint(&self, _: NativeTypeParameterKey) -> bool {
        true
    }
    fn type_parameter_constraint(
        &self,
        _: NativeTypeParameterKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        Some(Type::NativeSignature(signature(0)))
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "SignatureOnly".into()
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key == function()
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        vec![]
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        None
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        self.body_queries.set(self.body_queries.get() + 1);
        panic!("unused recursive callee body was queried")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("flow may not start unused return inference")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("flow may not start unused return inference")
    }
}
impl NativeFlowProvider for Fixture {
    fn contains_graph(&self, key: NativeGraphKey) -> bool {
        key.snapshot == SNAPSHOT && key.region == 0 && key.context <= 2
    }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool {
        self.contains_graph(key.graph) && key.reference == 0
    }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        if !self.contains_graph(key.graph) {
            return None;
        }
        let context = key.graph.context;
        Some(match key.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Call {
                before: at(context, 0),
                value: value(9),
                input: at(context, 0),
            },
            2 => NativeFlowNode::Predicate {
                before: at(context, 0),
                predicate: self.predicate(at(context, 0))?,
                sense: true,
            },
            _ => return None,
        })
    }
    fn is_shared(&self, key: NativeFlowKey) -> bool {
        key.node == 0
    }
    fn write_affects(
        &self,
        reference: NativeReferenceKey,
        target: NativeReferenceKey,
        _: NativeFlowKey,
    ) -> NativeWriteRelation {
        if reference == target {
            NativeWriteRelation::Exact
        } else {
            NativeWriteRelation::Unaffected
        }
    }
    // Values 1 and 2 are the original `nil` and `false` literals.
    fn value_literal_truthiness(&self, value: NativeValueKey) -> Option<bool> {
        matches!(value.index, 1 | 2).then_some(false)
    }
    fn value_type(
        &self,
        key: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.value_queries.set(self.value_queries.get() + 1);
        match key.index {
            0 => {
                let initial = Self::initial(input.graph.context);
                queries
                    .flow_type(
                        self,
                        input,
                        reference(input.graph.context),
                        &initial,
                        &initial,
                    )
                    .ty
            }
            1 => Type::Nil,
            2 => Type::BooleanLiteral(false),
            _ => panic!("unused call value must not be evaluated"),
        }
    }
    fn call_effects(
        &self,
        _: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        let original = queries.global_binding_type(binding()).unwrap();
        let callee = match self.callee {
            Callee::Original | Callee::Alias => original,
            Callee::Shape => Type::Function(FunctionType {
                params: vec![Parameter {
                    name: "value".into(),
                    ty: Type::Unknown,
                    optional: true,
                }],
                returns: Box::new(Type::Unknown),
            }),
            Callee::RecursiveUntyped => Type::NativeFunction(function()),
        };
        if queries.function_intrinsic(&callee).is_some() {
            if matches!(self.condition, Condition::NoArguments) {
                return NativeCallEffects::Never;
            }
            return NativeCallEffects::Assert {
                condition: value(8),
                input: if self.foreign_effect { at(2, 0) } else { input },
            };
        }
        if matches!(
            queries.function_declared_scalar_return(&callee),
            Some(Type::Never)
        ) {
            NativeCallEffects::Never
        } else {
            NativeCallEffects::None
        }
    }
    fn assertion_predicate(
        &self,
        condition: NativeValueKey,
        input: NativeFlowKey,
    ) -> Option<NativePredicate> {
        assert_eq!(condition, value(8));
        self.predicate(input)
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        self.errors.borrow_mut().push(error);
    }
}

#[test]
fn original_assertion_identity_is_pure_and_aliases_keep_it_while_shapes_and_mixed_heads_do_not() {
    Engine::new().with_native_provider(
        || Fixture::new(Condition::Truthy),
        |fixture, queries| {
            let original = queries.global_binding_type(binding()).unwrap();
            let site = queries.function_intrinsic(&original).unwrap();
            assert_eq!(site.declaration, declaration());
            let alias = original.clone();
            assert_eq!(queries.function_intrinsic(&alias), Some(site));
            let mixed = queries.signature_group_type(&[signature(0), signature(1)]);
            assert_eq!(queries.function_intrinsic(&mixed), None);
            assert_eq!(
                queries.function_intrinsic(&Type::NativeTypeParameter(NativeTypeParameterKey {
                    snapshot: SNAPSHOT,
                    index: 0
                })),
                None
            );
            assert_eq!(
                queries.function_intrinsic(&Type::NativeFunction(function())),
                None
            );
            assert_eq!(fixture.body_queries.get(), 0);
            assert_eq!(fixture.written_result_queries.get(), 0);
        },
    );
}

#[test]
fn assertion_reference_and_compound_transfers_match_the_existing_true_conditional_worker() {
    for condition in [
        Condition::Truthy,
        Condition::NotNil,
        Condition::Number,
        Condition::Not,
        Condition::And,
    ] {
        Engine::new().with_native_provider(
            || Fixture::new(condition),
            |fixture, queries| {
                let initial = Fixture::initial(0);
                let asserted =
                    queries.flow_type(fixture, at(0, 1), reference(0), &initial, &initial);
                let conditional =
                    queries.flow_type(fixture, at(0, 2), reference(0), &initial, &initial);
                assert!(!asserted.incomplete);
                assert!(queries.is_assignable(&asserted.ty, &conditional.ty));
                assert!(queries.is_assignable(&conditional.ty, &asserted.ty));
                match condition {
                    Condition::Truthy | Condition::Number | Condition::And => {
                        assert!(!queries.is_assignable(&Type::Nil, &asserted.ty))
                    }
                    Condition::NotNil => {
                        assert!(!queries.is_assignable(&Type::Nil, &asserted.ty));
                        assert!(queries.is_assignable(&Type::BooleanLiteral(false), &asserted.ty));
                    }
                    Condition::Not => assert!(queries.is_assignable(
                        &asserted.ty,
                        &Type::Union(vec![Type::Nil, Type::BooleanLiteral(false)])
                    )),
                    _ => unreachable!(),
                }
                assert!(fixture.errors.borrow().is_empty());
                assert_eq!(fixture.body_queries.get(), 0);
                assert_eq!(fixture.written_result_queries.get(), 0);
            },
        );
    }
}

#[test]
fn asserted_false_nil_and_missing_values_end_reachability_without_unused_call_results() {
    for condition in [Condition::False, Condition::Nil, Condition::NoArguments] {
        Engine::new().with_native_provider(
            || Fixture::new(condition),
            |fixture, queries| {
                assert!(!queries.flow_reachable(fixture, at(0, 1)));
                assert_eq!(fixture.body_queries.get(), 0);
                assert_eq!(fixture.written_result_queries.get(), 0);
                assert!(fixture.errors.borrow().is_empty());
            },
        );
    }
    Engine::new().with_native_provider(
        || Fixture::new(Condition::Unknown),
        |fixture, queries| {
            assert!(queries.flow_reachable(fixture, at(0, 1)));
            assert_eq!(fixture.value_queries.get(), 0);
            assert_eq!(fixture.body_queries.get(), 0);
            assert_eq!(fixture.written_result_queries.get(), 0);
        },
    );
}

#[test]
fn contextual_assertion_predicates_keep_the_original_graph_facet_and_reject_foreign_inputs() {
    Engine::new().with_native_provider(
        || Fixture::new(Condition::Truthy),
        |fixture, queries| {
            for context in [1, 2, 1] {
                let initial = Fixture::initial(context);
                let narrowed = queries.flow_type(
                    fixture,
                    at(context, 1),
                    reference(context),
                    &initial,
                    &initial,
                );
                assert!(!narrowed.incomplete);
                assert_eq!(
                    narrowed.ty,
                    if context == 2 {
                        Type::String
                    } else {
                        Type::Number
                    }
                );
            }
            assert!(fixture.errors.borrow().is_empty());
            assert_eq!(fixture.written_result_queries.get(), 0);
        },
    );
    Engine::new().with_native_provider(
        || Fixture {
            foreign_effect: true,
            ..Fixture::new(Condition::Truthy)
        },
        |fixture, queries| {
            assert!(!queries.flow_reachable(fixture, at(1, 1)));
            assert!(matches!(
                fixture.errors.borrow().as_slice(),
                [NativeFlowError::ForeignEdge { .. }]
            ));
        },
    );
}

#[test]
fn untrusted_policy_shaped_callees_and_recursive_unused_callees_acquire_no_assertion_effect() {
    for (trusted, callee) in [
        (false, Callee::Original),
        (true, Callee::Shape),
        (true, Callee::RecursiveUntyped),
    ] {
        Engine::new().with_native_provider(
            || Fixture {
                trusted,
                callee,
                ..Fixture::new(Condition::False)
            },
            |fixture, queries| {
                if !trusted {
                    let original = queries.global_binding_type(binding()).unwrap();
                    assert_eq!(queries.function_intrinsic(&original), None);
                    assert_eq!(fixture.written_result_queries.get(), 0);
                }
                assert!(queries.flow_reachable(fixture, at(0, 1)));
                let initial = Fixture::initial(0);
                let after = queries.flow_type(fixture, at(0, 1), reference(0), &initial, &initial);
                assert!(queries.is_assignable(&Type::Nil, &after.ty));
                assert_eq!(fixture.body_queries.get(), 0);
                if !trusted {
                    assert!(fixture.written_result_queries.get() > 0);
                } else {
                    assert_eq!(fixture.written_result_queries.get(), 0);
                }
                assert_eq!(fixture.value_queries.get(), 0);
                assert!(fixture.errors.borrow().is_empty());
            },
        );
    }
    Engine::new().with_native_provider(
        || Fixture {
            callee: Callee::Alias,
            ..Fixture::new(Condition::Truthy)
        },
        |fixture, queries| {
            let initial = Fixture::initial(0);
            let after = queries.flow_type(fixture, at(0, 1), reference(0), &initial, &initial);
            assert!(!queries.is_assignable(&Type::Nil, &after.ty));
            assert_eq!(fixture.written_result_queries.get(), 0);
        },
    );
}
