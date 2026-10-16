//! A nominal bound stays usable while its consumer is iterating an unrelated
//! loop. New ancestry dependencies must still keep a result provisional.
use super::*;

fn nominal(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey { snapshot: SNAPSHOT, index: 100 + index }
}
fn parameter(index: u32) -> NativeTypeParameterKey {
    NativeTypeParameterKey { snapshot: SNAPSHOT, index }
}
fn instance() -> Type {
    Type::NativeNamedReference { declaration: nominal(1), arguments: Vec::new() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ancestry { Complete, PendingMember, FlowDependency, Cycle }

struct NominalLoop {
    flow: GlobalContexts,
    ancestry: Ancestry,
    base_calls: Cell<usize>,
    inherited_checks: Cell<usize>,
    errors: RefCell<Vec<NamedTypeError>>,
}
impl NominalLoop {
    fn new(ancestry: Ancestry) -> Self {
        Self {
            flow: GlobalContexts::new(Inherited::Loop, Contract::Constant),
            ancestry, base_calls: Cell::new(0), inherited_checks: Cell::new(0),
            errors: RefCell::new(Vec::new()),
        }
    }
    fn read_loop(&self, queries: &mut dyn TypeQueries, region: u32) -> NativeFlowResult {
        queries.flow_type(self, at(region, 1), reference(region),
            &Type::Union(vec![Type::Number, Type::Nil]), &Type::Nil)
    }
    fn check_inherited(&self, queries: &mut dyn TypeQueries) {
        self.inherited_checks.set(self.inherited_checks.get() + 1);
        let before = self.base_calls.get();
        let actual = Type::NativeTypeParameter(parameter(0));
        let expected = instance();
        for _ in 0..2 {
            let before_relation = self.base_calls.get();
            let related = queries.is_assignable(&actual, &expected);
            if self.ancestry == Ancestry::Complete {
                assert!(self.base_calls.get() > before_relation,
                    "each inherited relation must recompute ancestry without a completed cache entry");
            }
            let before_application = self.base_calls.get();
            let application = queries.named_type_reference(nominal(2), &[actual.clone()]);
            if self.ancestry == Ancestry::Complete {
                assert!(related, "a bounded T must satisfy its exact nominal bound under inherited incomplete flow");
                application.expect("the same T must satisfy a named generic's bound");
                assert!(self.base_calls.get() > before_application,
                    "generic bound checks must not reuse an inherited completed ancestry entry");
            } else {
                assert!(!related, "a fresh ancestry dependency must not become a completed nominal relation");
                assert!(application.is_err(), "a refused ancestry must not admit a generic argument through fallback types");
            }
        }
        assert!(self.base_calls.get() > before,
            "ancestry must actually be demanded in the inherited flow context");
    }
}

impl NativeTypeProvider for NominalLoop {
    fn contains_function(&self, key: NativeFunctionKey) -> bool { key.snapshot == SNAPSHOT && key.index < 2 }
    fn function_parameters(&self, key: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        if key == function(1) { vec![Parameter { name: "argument".into(), ty: Type::Number, optional: false }] } else { Vec::new() }
    }
    fn function_declared_return(&self, key: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> {
        (key == function(1)).then_some(Type::Number)
    }
    fn function_inferred_return(&self, key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Type {
        assert_eq!(key, function(0));
        let called = queries.call_native_arguments(&Type::NativeFunction(function(1)), None,
            &[PackArgument::Expression(expression())], &[], None);
        assert!(called.errors.is_empty(), "{:?}", called.errors);
        Type::Number
    }
    fn return_cycle(&self, _: NativeFunctionKey) { unreachable!() }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { unreachable!() }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool { key == expression() }
    fn check_expression(&self, key: NativeExpressionKey, expected: Option<&Type>, mode: NativeCheckMode, queries: &mut dyn TypeQueries) -> Type {
        let expected = expected.map(|ty| ValuePack::single(ty.clone()));
        let pack = self.check_expression_pack(key, expected.as_ref(), mode, queries);
        queries.pack_slot(&pack, 0)
    }
    fn check_expression_pack(&self, key: NativeExpressionKey, _: Option<&ValuePack>, _: NativeCheckMode, queries: &mut dyn TypeQueries) -> ValuePack {
        assert_eq!(key, expression());
        let result = self.read_loop(queries, 0);
        assert_eq!(result.ty, queries.union_reduced(&[Type::Number, Type::Nil]));
        ValuePack::single(Type::Number)
    }
    fn runtime_type_declaration(&self, kind: NativeOpaqueRuntimeKind) -> Option<NativeTypeDeclarationKey> {
        (kind == NativeOpaqueRuntimeKind::Userdata).then_some(nominal(0))
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
        if key.index == 0 { "T".into() } else { "U".into() }
    }
    fn has_type_parameter_constraint(&self, _: NativeTypeParameterKey) -> bool { true }
    fn type_parameter_constraint(&self, _: NativeTypeParameterKey, _: &mut dyn TypeQueries) -> Option<Type> { Some(instance()) }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        self.flow.contains_type_declaration(key) || (key.snapshot == SNAPSHOT && (100..103).contains(&key.index))
    }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        if key.index < 100 { return self.flow.type_declaration_header(key); }
        TypeDeclarationHeader {
            kind: if key == nominal(2) { TypeDeclarationKind::Shape } else { TypeDeclarationKind::Class },
            name: match key.index { 100 => "OpaqueRoot", 101 => "Instance", _ => "Container" }.into(),
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: if key == nominal(2) { vec![parameter(1)] } else { Vec::new() },
            outer_parameters: Vec::new(),
        }
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        if key.index < 100 { self.flow.type_declaration_members(key) } else { Vec::new() }
    }
    fn type_declaration_bases(&self, key: NativeTypeDeclarationKey, queries: &mut dyn TypeQueries) -> Vec<Type> {
        if key != nominal(1) { return Vec::new(); }
        self.base_calls.set(self.base_calls.get() + 1);
        if self.flow.pending_dependency.get() {
            match self.ancestry {
                Ancestry::PendingMember => self.flow.pending_member(1, queries),
                Ancestry::FlowDependency => {
                    let result = self.read_loop(queries, 1);
                    assert_eq!(result.ty, queries.union_reduced(&[Type::Number, Type::Nil]));
                }
                Ancestry::Cycle => return vec![instance()],
                Ancestry::Complete => {},
            }
        }
        Vec::new()
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool { self.flow.contains_type_member(key) }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader { self.flow.type_member_header(key) }
    fn type_member_activation(&self, key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> TypeMemberActivation {
        self.flow.type_member_activation(key, queries)
    }
    fn type_member_type(&self, key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Type { self.flow.type_member_type(key, queries) }
    fn type_declaration_error(&self, error: NamedTypeError) { self.errors.borrow_mut().push(error); }
}

impl NativeFlowProvider for NominalLoop {
    fn contains_graph(&self, key: NativeGraphKey) -> bool { self.flow.contains_graph(key) }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool { self.flow.contains_reference(key) }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> { self.flow.node(key) }
    fn is_shared(&self, key: NativeFlowKey) -> bool { self.flow.is_shared(key) }
    fn write_affects(&self, reference: NativeReferenceKey, target: NativeReferenceKey, input: NativeFlowKey) -> NativeWriteRelation {
        self.flow.write_affects(reference, target, input)
    }
    fn value_type(&self, key: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type {
        self.flow.value_type(key, input, queries)
    }
    fn call_effects(&self, key: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> NativeCallEffects {
        assert_eq!(key, value(input.graph.region));
        let active = self.read_loop(queries, input.graph.region);
        assert!(active.incomplete, "read the active original backedge before demanding nominal ancestry");
        if input.graph.region == 0 { self.check_inherited(queries); }
        NativeCallEffects::None
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("invalid original graph: {error:?}"); }
}

#[test]
fn nominal_type_parameter_bounds_remain_usable_in_an_inherited_incomplete_loop() {
    Engine::new().with_native_provider(|| NominalLoop::new(Ancestry::Complete), |provider, queries| {
        // Match the corpus failure: the exact declared bound is already known,
        // so the unrelated active loop must not make T fail that same bound.
        let known = queries.type_parameter_constraint(parameter(0)).unwrap();
        assert_eq!(queries.named_object_declaration(&known), Some(nominal(1)));
        // Native argument checking supplies the inference context in which
        // child declaration queries inherit their parent's incomplete flow.
        queries.function_result_pack(&Type::NativeFunction(function(0)));
        assert!(provider.inherited_checks.get() > 0);
        let before = provider.base_calls.get();
        for _ in 0..2 { assert!(queries.is_assignable(&Type::NativeTypeParameter(parameter(0)), &instance())); }
        assert_eq!(provider.base_calls.get(), before + 1,
            "inherited current-only ancestry must recompute cleanly once before entering a completed cache");
        assert!(!queries.is_assignable(&Type::String, &instance()),
            "current-only completion must not bypass ordinary type or nominal compatibility");
    });
}

#[test]
fn fresh_pending_nominal_ancestry_is_not_accepted_under_an_inherited_loop() {
    for ancestry in [Ancestry::PendingMember, Ancestry::FlowDependency, Ancestry::Cycle] {
        Engine::new().with_native_provider(|| NominalLoop::new(ancestry), |provider, queries| {
            let known = queries.type_parameter_constraint(parameter(0)).unwrap();
            assert_eq!(queries.named_object_declaration(&known), Some(nominal(1)));
            queries.function_result_pack(&Type::NativeFunction(function(0)));
            assert!(provider.inherited_checks.get() > 0);
            if ancestry != Ancestry::Cycle {
                provider.flow.pending_dependency.set(false);
                let before = provider.base_calls.get();
                assert!(queries.is_assignable(&Type::NativeTypeParameter(parameter(0)), &instance()),
                    "a fresh refusal must not leave a completed negative nominal relation");
                assert!(provider.base_calls.get() > before);
            } else {
                assert!(provider.errors.borrow().iter().any(|error| matches!(error, NamedTypeError::CircularBase(_))));
            }
        });
    }
}
