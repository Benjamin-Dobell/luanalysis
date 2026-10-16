//! Demand guards and cache policy for reached computed numeric stores.
//! Every callback uses an original declaration, expression, or flow identity.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WrittenNumeric {
    None,
    Fixed,
    Indexer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiverResult {
    Matching,
    Foreign,
    PendingMember,
    Recursive,
}

struct NumericDemands {
    written: WrittenNumeric,
    receiver: ReceiverResult,
    receiver_calls: Cell<usize>,
    future_calls: Cell<usize>,
    argument_checks: Cell<usize>,
    inherited_pending_reads: Cell<usize>,
    pending_member: Cell<bool>,
    inside_receiver: Cell<bool>,
    target: RefCell<Option<Type>>,
}

impl NumericDemands {
    fn new(written: WrittenNumeric, receiver: ReceiverResult) -> Self {
        Self {
            written,
            receiver,
            receiver_calls: Cell::new(0),
            future_calls: Cell::new(0),
            argument_checks: Cell::new(0),
            inherited_pending_reads: Cell::new(0),
            pending_member: Cell::new(true),
            inside_receiver: Cell::new(false),
            target: RefCell::new(None),
        }
    }

    fn allocation(&self, queries: &mut dyn TypeQueries, index: u32, actual: TableType) -> Type {
        let actual = queries.table_type(&actual).unwrap();
        queries.runtime_table_value(declaration(index), &actual).unwrap()
    }

    fn install_target(&self, queries: &mut dyn TypeQueries) -> Type {
        let target = self.allocation(queries, 0, TableType::default());
        *self.target.borrow_mut() = Some(target.clone());
        target
    }

    fn admission(&self, queries: &mut dyn TypeQueries) -> Result<(), TableError> {
        let target = self.target.borrow().clone().expect("the original allocation exists before its store");
        let flow = DemandFlow::new(Type::Number, false, true);
        queries.table_write_at_flow(&flow, at(1), &target, &TableKey::Type(Type::Number), &Type::String)
    }

    fn pending_read(&self, queries: &mut dyn TypeQueries, index: u32) -> Result<Option<Type>, TableError> {
        queries.property_type_checked(
            &Type::NativeNamedReference { declaration: declaration(index), arguments: vec![] },
            "gate",
        )
    }

    fn tainted_argument(&self, queries: &mut dyn TypeQueries) {
        let _ = queries.function_result_pack(&Type::NativeFunction(function(0)));
        assert!(self.argument_checks.get() > 0, "the native argument must actually be checked");
        assert!(self.inherited_pending_reads.get() > 0, "the enclosing return query must actually be tainted");
    }
}

impl NativeTypeProvider for NumericDemands {
    fn contains_function(&self, original: NativeFunctionKey) -> bool {
        original.snapshot == 6400 && original.index < 3
    }
    fn function_parameters(&self, original: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        if original == function(1) {
            vec![Parameter { name: "argument".into(), ty: Type::Number, optional: false }]
        } else { vec![] }
    }
    fn function_declared_return(&self, original: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> {
        (original.index > 0).then_some(Type::Number)
    }
    fn function_inferred_return(&self, original: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Type {
        assert_eq!(original, function(0));
        let result = queries.call_native_arguments(
            &Type::NativeFunction(function(1)), None,
            &[PackArgument::Expression(expression(2))], &[], None,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        Type::Number
    }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("the original function return is not recursive") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("the original function return is finite") }
    fn contains_expression(&self, original: NativeExpressionKey) -> bool {
        original.snapshot == 6400 && original.node < 3
    }
    fn check_expression(&self, original: NativeExpressionKey, expected: Option<&Type>, mode: NativeCheckMode, queries: &mut dyn TypeQueries) -> Type {
        let expected = expected.map(|ty| ValuePack::single(ty.clone()));
        let pack = self.check_expression_pack(original, expected.as_ref(), mode, queries);
        queries.pack_slot(&pack, 0)
    }
    fn check_expression_pack(&self, original: NativeExpressionKey, _: Option<&ValuePack>, _: NativeCheckMode, queries: &mut dyn TypeQueries) -> ValuePack {
        assert_eq!(original, expression(2));
        self.argument_checks.set(self.argument_checks.get() + 1);
        assert_eq!(self.pending_read(queries, 2), Err(TableError::PendingContext));
        self.inherited_pending_reads.set(self.inherited_pending_reads.get() + 1);
        let before = self.receiver_calls.get();
        for _ in 0..2 {
            let result = self.admission(queries);
            if self.receiver == ReceiverResult::Foreign {
                assert_eq!(result, Ok(()), "inherited cache policy must not invent a pending numeric contract");
            } else {
                assert_eq!(result, Err(TableError::PendingContext), "a fresh dependency remains semantically pending");
            }
        }
        assert_eq!(self.receiver_calls.get(), before + 2, "an inherited-tainted numeric contract must not enter the completed cache");
        ValuePack::single(Type::Number)
    }
    fn contains_type_parameter(&self, original: NativeTypeParameterKey) -> bool { original == parameter() }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String { "N".into() }
    fn type_parameter_owner(&self, _: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(TypeDeclarationOwner::Function(function(2)))
    }
    fn function_type_parameters(&self, original: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<NativeTypeParameterKey> {
        if original == function(2) { vec![parameter()] } else { vec![] }
    }
    fn has_type_parameter_constraint(&self, _: NativeTypeParameterKey) -> bool { true }
    fn type_parameter_constraint(&self, _: NativeTypeParameterKey, _: &mut dyn TypeQueries) -> Option<Type> { Some(Type::Number) }
    fn contains_type_declaration(&self, original: NativeTypeDeclarationKey) -> bool {
        original.snapshot == 6400 && original.index < 4
    }
    fn type_declaration_header(&self, original: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: if original.index < 2 { TypeDeclarationKind::RuntimeTable } else { TypeDeclarationKind::Shape },
            name: format!("numeric demand original {}", original.index),
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: vec![], outer_parameters: vec![],
        }
    }
    fn runtime_table_constructor(&self, original: NativeTypeDeclarationKey) -> Option<NativeExpressionKey> {
        (original.index < 2).then_some(expression(original.index))
    }
    fn runtime_table_seed(&self, _: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("numeric admission must use the original value bounds without demanding the evolving seed")
    }
    fn runtime_store_receiver(&self, original: NativeFlowKey, queries: &mut dyn TypeQueries) -> Option<Type> {
        assert_eq!(original, at(2), "an inferred future member must not demand its receiver");
        self.receiver_calls.set(self.receiver_calls.get() + 1);
        match self.receiver {
            ReceiverResult::Matching => return self.target.borrow().clone(),
            ReceiverResult::Foreign => {}
            ReceiverResult::PendingMember => {
                let result = self.pending_read(queries, 3);
                assert_eq!(result, if self.pending_member.get() { Err(TableError::PendingContext) } else { Ok(Some(Type::Number)) });
            }
            ReceiverResult::Recursive => {
                assert!(!self.inside_receiver.replace(true), "the native query guard must stop receiver reentry");
                assert_eq!(self.admission(queries), Err(TableError::PendingContext));
                self.inside_receiver.set(false);
            }
        }
        Some(self.allocation(queries, 1, TableType::default()))
    }
    fn type_declaration_members(&self, original: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        match original.index {
            0 if self.written != WrittenNumeric::None => vec![member(0), member(1)],
            0 | 1 => vec![member(0)],
            2 | 3 => vec![member(original.index)],
            _ => unreachable!(),
        }
    }
    fn contains_type_member(&self, original: NativeTypeMemberKey) -> bool {
        original.snapshot == 6400 && original.index < 4
    }
    fn type_member_header(&self, original: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: match original.index {
                0 => TypeMemberOwner::RuntimeStore(at(3)),
                1 => TypeMemberOwner::RuntimeStore(at(2)),
                _ => TypeMemberOwner::Declared(declaration(original.index)),
            },
            key: match original.index {
                0 => TypeMemberKeyRecipe::Indexer { sparse: true },
                1 if self.written == WrittenNumeric::Indexer => TypeMemberKeyRecipe::Indexer { sparse: false },
                1 => TypeMemberKeyRecipe::Fixed(LuaKey::Integer(1)),
                _ => TypeMemberKeyRecipe::Fixed(key("gate")),
            },
            visibility: TypeMemberVisibility::Public, optional: false, readonly: false,
        }
    }
    fn type_member_origin(&self, original: NativeTypeMemberKey) -> TableFieldOrigin {
        if original.index == 0 { TableFieldOrigin::Inferred } else { TableFieldOrigin::Declared }
    }
    fn type_member_activation(&self, original: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> TypeMemberActivation {
        if original.index == 2 || original.index == 3 && self.pending_member.get() {
            TypeMemberActivation::Pending
        } else { TypeMemberActivation::Active }
    }
    fn type_member_type(&self, original: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        if original.index >= 2 { return Type::Number; }
        self.future_calls.set(self.future_calls.get() + 1);
        panic!("numeric admission must not demand a prospective stored value")
    }
    fn type_indexer_key(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        panic!("numeric admission must decide written recipes from their immutable headers")
    }
}

struct DemandFlow {
    key: Type,
    allow_rhs: bool,
    same_scope: bool,
    key_calls: Cell<usize>,
    rhs_calls: Cell<usize>,
}
impl DemandFlow {
    fn new(key: Type, allow_rhs: bool, same_scope: bool) -> Self {
        Self { key, allow_rhs, same_scope, key_calls: Cell::new(0), rhs_calls: Cell::new(0) }
    }
}
impl NativeFlowProvider for DemandFlow {
    fn contains_graph(&self, original: NativeGraphKey) -> bool { original == graph() }
    fn contains_reference(&self, original: NativeReferenceKey) -> bool { original.graph == graph() && original.reference < 2 }
    fn node(&self, original: NativeFlowKey) -> Option<NativeFlowNode> {
        if original.graph != graph() { return None; }
        match original.node {
            0 => Some(NativeFlowNode::Start),
            1 | 2 | 3 => Some(Flow::write(0, 1, 1)),
            _ => None,
        }
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn write_affects(&self, query: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if query == reference(0) && target == reference(1) { NativeWriteRelation::Descendant } else { NativeWriteRelation::Unaffected }
    }
    fn write_index_value(&self, query: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> Option<(NativeValueKey, NativeFlowKey)> {
        (query == reference(0) && target == reference(1)).then_some((value(0), at(0)))
    }
    fn runtime_table_store_in_scope(&self, original: NativeTypeDeclarationKey, _: NativeFlowKey) -> bool {
        self.same_scope && original.index < 2
    }
    fn value_type(&self, original: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        if original == value(0) {
            self.key_calls.set(self.key_calls.get() + 1);
            return self.key.clone();
        }
        assert_eq!(original, value(1));
        self.rhs_calls.set(self.rhs_calls.get() + 1);
        assert!(self.allow_rhs, "an excluded computed store must not demand its RHS");
        Type::String
    }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { NativeCallEffects::None }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("invalid original numeric graph: {error:?}") }
}

fn unchanged_without_rhs(provider: &NumericDemands, queries: &mut dyn TypeQueries, table: &Type, flow: &DemandFlow) {
    let reached = queries.flow_type(flow, at(1), reference(0), table, table);
    assert!(!reached.incomplete, "{reached:?}");
    assert_eq!(&reached.ty, table);
    assert_eq!(flow.key_calls.get(), 1);
    assert_eq!(flow.rhs_calls.get(), 0);
    assert_eq!(provider.future_calls.get(), 0);
}

#[test]
fn excluded_computed_key_types_do_not_demand_the_store_rhs() {
    for key in [Type::String, Type::Any, Type::Unknown, Type::NativeTypeParameter(parameter())] {
        Engine::new().with_native_provider(
            || NumericDemands::new(WrittenNumeric::None, ReceiverResult::Matching),
            |provider, queries| {
                let table = provider.install_target(queries);
                unchanged_without_rhs(provider, queries, &table, &DemandFlow::new(key, false, true));
                assert_eq!(provider.receiver_calls.get(), 0);
            },
        );
    }
}

#[test]
fn written_original_bounds_and_foreign_scopes_do_not_demand_the_store_rhs() {
    for actual in [
        TableType { fields: vec![TableField::declared(key("named"), Type::Number)], ..Default::default() },
        TableType { indexers: vec![TableIndexer::declared(Type::Number, Type::Number)], ..Default::default() },
    ] {
        Engine::new().with_native_provider(
            || NumericDemands::new(WrittenNumeric::None, ReceiverResult::Matching),
            |provider, queries| {
                let table = provider.allocation(queries, 0, actual);
                unchanged_without_rhs(provider, queries, &table, &DemandFlow::new(Type::Number, false, true));
                assert_eq!(provider.receiver_calls.get(), 0);
            },
        );
    }
    Engine::new().with_native_provider(
        || NumericDemands::new(WrittenNumeric::None, ReceiverResult::Matching),
        |provider, queries| {
            let table = provider.install_target(queries);
            unchanged_without_rhs(provider, queries, &table, &DemandFlow::new(Type::Number, false, false));
            assert_eq!(provider.receiver_calls.get(), 0);
        },
    );
}

#[test]
fn written_numeric_member_and_indexer_headers_exclude_rhs_and_future_member_demands() {
    for written in [WrittenNumeric::Fixed, WrittenNumeric::Indexer] {
        Engine::new().with_native_provider(
            || NumericDemands::new(written, ReceiverResult::Matching),
            |provider, queries| {
                let table = provider.install_target(queries);
                unchanged_without_rhs(provider, queries, &table, &DemandFlow::new(Type::Number, false, true));
                assert_eq!(provider.receiver_calls.get(), 1, "the matching written recipe establishes ownership once");
            },
        );
    }
}

#[test]
fn eligible_and_excluded_union_arms_demand_rhs_once_and_evolve_only_the_eligible_arm() {
    Engine::new().with_native_provider(
        || NumericDemands::new(WrittenNumeric::Indexer, ReceiverResult::Foreign),
        |provider, queries| {
            let eligible = provider.install_target(queries);
            let excluded = provider.allocation(queries, 1, TableType {
                fields: vec![TableField::declared(LuaKey::Integer(1), Type::Boolean)], ..Default::default()
            });
            let initial = Type::Union(vec![eligible, excluded.clone()]);
            let flow = DemandFlow::new(Type::Number, true, true);
            let reached = queries.flow_type(&flow, at(1), reference(0), &initial, &initial);
            assert!(!reached.incomplete, "{reached:?}");
            assert_eq!(flow.key_calls.get(), 1);
            assert_eq!(flow.rhs_calls.get(), 1, "a computed RHS is demanded once for all prepared eligible arms");
            let Type::Union(parts) = queries.inspect_type(&reached.ty) else { panic!("the two original allocation arms must remain distinct") };
            assert_eq!(parts.len(), 2);
            for part in parts {
                match queries.runtime_table_declaration(&part) {
                    Some(original) if original == declaration(0) => {
                        let read = queries.table_lookup(&part, &TableKey::Type(Type::Number)).unwrap();
                        assert!(queries.is_assignable(&Type::String, &read));
                        assert!(queries.is_assignable(&Type::Nil, &read));
                        assert!(!queries.is_assignable(&Type::Boolean, &read));
                    }
                    Some(original) if original == declaration(1) => {
                        assert_eq!(part, excluded);
                        assert_eq!(queries.table_lookup(&part, &TableKey::Literal(LuaKey::Integer(1))), Ok(Type::Boolean));
                    }
                    other => panic!("unexpected original allocation {other:?}"),
                }
            }
            assert_eq!(provider.future_calls.get(), 0);
            assert_eq!(provider.receiver_calls.get(), 1, "applying prepared arms must not repeat written-contract receiver discovery");
        },
    );
}

#[test]
fn a_fresh_pending_written_receiver_does_not_demand_the_store_rhs() {
    Engine::new().with_native_provider(
        || NumericDemands::new(WrittenNumeric::Indexer, ReceiverResult::PendingMember),
        |provider, queries| {
            let table = provider.install_target(queries);
            let flow = DemandFlow::new(Type::Number, false, true);
            let reached = queries.flow_type(&flow, at(1), reference(0), &table, &table);
            assert!(reached.incomplete);
            assert_eq!(reached.ty, table);
            assert_eq!(flow.rhs_calls.get(), 0);
            assert_eq!(provider.receiver_calls.get(), 1);
            assert_eq!(provider.future_calls.get(), 0);
        },
    );
}

#[test]
fn inherited_pending_taint_keeps_a_false_numeric_contract_complete_and_uncached() {
    Engine::new().with_native_provider(
        || NumericDemands::new(WrittenNumeric::Indexer, ReceiverResult::Foreign),
        |provider, queries| {
            provider.install_target(queries);
            provider.tainted_argument(queries);
            let before = provider.receiver_calls.get();
            assert_eq!(provider.admission(queries), Ok(()));
            assert_eq!(provider.receiver_calls.get(), before + 1, "a later clean context must recompute a current-only false contract");
            assert_eq!(provider.admission(queries), Ok(()));
            assert_eq!(provider.receiver_calls.get(), before + 1, "an ordinary completed false contract can still be cached");
            assert_eq!(provider.future_calls.get(), 0);
        },
    );
}

#[test]
fn fresh_pending_receiver_dependencies_are_not_swallowed_as_inherited_taint() {
    Engine::new().with_native_provider(
        || NumericDemands::new(WrittenNumeric::Indexer, ReceiverResult::PendingMember),
        |provider, queries| {
            provider.install_target(queries);
            provider.tainted_argument(queries);
            provider.pending_member.set(false);
            let before = provider.receiver_calls.get();
            assert_eq!(provider.admission(queries), Ok(()));
            assert_eq!(provider.receiver_calls.get(), before + 1, "a provisional contract must not become a completed-cache hit");
            assert_eq!(provider.admission(queries), Ok(()));
            assert_eq!(provider.receiver_calls.get(), before + 1);
        },
    );
}

#[test]
fn recursive_numeric_contract_receivers_stay_pending_with_and_without_ambient_inference() {
    Engine::new().with_native_provider(
        || NumericDemands::new(WrittenNumeric::Indexer, ReceiverResult::Recursive),
        |provider, queries| {
            provider.install_target(queries);
            provider.tainted_argument(queries);
            let before = provider.receiver_calls.get();
            for _ in 0..2 { assert_eq!(provider.admission(queries), Err(TableError::PendingContext)); }
            assert_eq!(provider.receiver_calls.get(), before + 2);
            assert_eq!(provider.future_calls.get(), 0);
        },
    );
}
