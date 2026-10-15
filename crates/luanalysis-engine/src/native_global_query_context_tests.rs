//! Declaration contracts can be usable under an inherited inference/flow
//! context without being safe to publish in a completed query cache.
use super::*;
use std::cell::{Cell, RefCell};

const SNAPSHOT: u64 = 52_419;
fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey { snapshot: SNAPSHOT, index }
}
fn expression() -> NativeExpressionKey {
    NativeExpressionKey { snapshot: SNAPSHOT, node: 0 }
}
fn graph(region: u32) -> NativeGraphKey {
    NativeGraphKey { snapshot: SNAPSHOT, region, context: 0 }
}
fn at(region: u32, node: u32) -> NativeFlowKey {
    NativeFlowKey { graph: graph(region), node }
}
fn reference(region: u32) -> NativeReferenceKey {
    NativeReferenceKey { graph: graph(region), reference: 0 }
}
fn value(region: u32) -> NativeValueKey {
    NativeValueKey { snapshot: SNAPSHOT, index: region }
}
fn environment() -> NativeEnvironmentKey {
    NativeEnvironmentKey { snapshot: SNAPSHOT, index: 0 }
}
fn binding() -> NativeGlobalBindingKey {
    NativeGlobalBindingKey { environment: environment(), index: 0 }
}
fn declaration() -> NativeValueDeclarationKey {
    NativeValueDeclarationKey { snapshot: SNAPSHOT, index: 0 }
}
fn chained_declaration(index: u32) -> NativeValueDeclarationKey {
    NativeValueDeclarationKey { snapshot: SNAPSHOT, index }
}
fn member_declaration(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey { snapshot: SNAPSHOT, index }
}
fn member(index: u32) -> NativeTypeMemberKey {
    NativeTypeMemberKey { snapshot: SNAPSHOT, index }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Inherited {
    Member,
    Loop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Contract {
    Constant,
    PendingMember,
    FlowDependency,
    Recursive,
    Missing,
    Implementation,
    DepthRefusal,
}

struct GlobalContexts {
    inherited: Inherited,
    contract: Contract,
    contract_calls: Cell<usize>,
    argument_checks: Cell<usize>,
    inherited_reads: Cell<usize>,
    pending_dependency: Cell<bool>,
    inside_loop: Cell<bool>,
    errors: RefCell<Vec<GlobalQueryError>>,
}
impl GlobalContexts {
    fn new(inherited: Inherited, contract: Contract) -> Self {
        Self {
            inherited,
            contract,
            contract_calls: Cell::new(0),
            argument_checks: Cell::new(0),
            inherited_reads: Cell::new(0),
            pending_dependency: Cell::new(true),
            inside_loop: Cell::new(false),
            errors: RefCell::new(Vec::new()),
        }
    }
    fn pending_member(&self, index: u32, queries: &mut dyn TypeQueries) {
        let result = queries.property_type_checked(
            &Type::NativeNamedReference {
                declaration: member_declaration(index),
                arguments: Vec::new(),
            },
            "gate",
        );
        if index == 0 || self.pending_dependency.get() {
            assert_eq!(result, Err(TableError::PendingContext));
        } else {
            assert_eq!(result, Ok(Some(Type::Number)));
        }
    }
    fn assert_current_queries(&self, queries: &mut dyn TypeQueries) {
        let before = self.contract_calls.get();
        for binding_query in [false, false, true, true] {
            let result = if binding_query {
                queries.global_binding_type(binding())
            } else {
                queries.value_declaration_type(declaration())
            };
            match self.contract {
                Contract::Constant => {
                    let ty = result.expect("an independent declaration is usable in this context");
                    assert!(queries.is_assignable(&ty, &Type::Number));
                    assert!(!queries.is_assignable(&Type::String, &ty));
                }
                Contract::Missing => {
                    assert_eq!(result, Err(GlobalQueryError::MissingDeclarationContract(declaration())));
                }
                Contract::Recursive => assert!(matches!(
                    result,
                    Err(GlobalQueryError::CircularQuery(_) | GlobalQueryError::UnresolvedQuery(_))
                ), "recursive declarations must remain unavailable: {result:?}"),
                Contract::PendingMember | Contract::FlowDependency | Contract::Implementation => {
                    assert!(matches!(result, Err(GlobalQueryError::UnresolvedQuery(_))),
                        "a fresh dependency or implementation must remain unresolved: {result:?}");
                }
                Contract::DepthRefusal => assert!(matches!(
                    result,
                    Err(GlobalQueryError::TooDeep(_) | GlobalQueryError::UnresolvedQuery(_))
                ), "a refused declaration must not expose the provider's fallback: {result:?}"),
            }
        }
        if self.contract == Contract::Constant {
            assert_eq!(self.contract_calls.get(), before + 4,
                "neither declaration nor global queries may enter a completed cache under inherited context");
        }
        // A current-only declaration must not clear its parent's real pending
        // dependency. Read it again while the same inference scope is active.
        if self.inherited == Inherited::Member && self.contract != Contract::DepthRefusal {
            self.pending_member(0, queries);
        }
    }
    fn read_loop(&self, queries: &mut dyn TypeQueries, region: u32) -> NativeFlowResult {
        // Equal initial/declaration bounds absorb the first edge and let the
        // flow walker skip the backedge entirely. A nil entry and number write
        // must both be visited before this original loop can settle.
        queries.flow_type(
            self, at(region, 1), reference(region),
            &Type::Union(vec![Type::Number, Type::Nil]), &Type::Nil,
        )
    }
}

impl NativeTypeProvider for GlobalContexts {
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn function_parameters(&self, key: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        if key == function(1) {
            vec![Parameter { name: "argument".into(), ty: Type::Number, optional: false }]
        } else {
            Vec::new()
        }
    }
    fn function_declared_return(&self, key: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> {
        (key == function(1)).then_some(Type::Number)
    }
    fn function_inferred_return(&self, key: NativeFunctionKey, queries: &mut dyn TypeQueries) -> Type {
        assert_eq!(key, function(0));
        let result = queries.call_native_arguments(
            &Type::NativeFunction(function(1)), None,
            &[PackArgument::Expression(expression())], &[], None,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        Type::Number
    }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("the original return provider is not recursive") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("the original return provider is finite") }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool { key == expression() }
    fn check_expression(&self, key: NativeExpressionKey, expected: Option<&Type>, mode: NativeCheckMode, queries: &mut dyn TypeQueries) -> Type {
        let expected = expected.map(|ty| ValuePack::single(ty.clone()));
        let pack = self.check_expression_pack(key, expected.as_ref(), mode, queries);
        queries.pack_slot(&pack, 0)
    }
    fn check_expression_pack(&self, key: NativeExpressionKey, _: Option<&ValuePack>, _: NativeCheckMode, queries: &mut dyn TypeQueries) -> ValuePack {
        assert_eq!(key, expression());
        self.argument_checks.set(self.argument_checks.get() + 1);
        match self.inherited {
            Inherited::Member => {
                self.pending_member(0, queries);
                self.inherited_reads.set(self.inherited_reads.get() + 1);
                self.assert_current_queries(queries);
            }
            Inherited::Loop => {
                let result = self.read_loop(queries, 0);
                assert_eq!(result.ty, queries.union_reduced(&[Type::Number, Type::Nil]));
            }
        }
        ValuePack::single(Type::Number)
    }
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool { key == environment() }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool { key == binding() }
    fn global_binding_declarations(&self, _: NativeGlobalBindingKey) -> Vec<NativeValueDeclarationKey> { vec![declaration()] }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool {
        key == declaration()
            || (self.contract == Contract::DepthRefusal && key.snapshot == SNAPSHOT && key.index < 2010)
    }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader {
            role: if self.contract == Contract::Implementation { GlobalSourceRole::Implementation } else { GlobalSourceRole::Declaration },
            kind: GlobalValueKind::Variable,
        }
    }
    fn value_declaration_contract(&self, key: NativeValueDeclarationKey, queries: &mut dyn TypeQueries) -> Option<Type> {
        self.contract_calls.set(self.contract_calls.get() + 1);
        if self.contract == Contract::DepthRefusal {
            // Distinct original declarations exceed the checker's MAX_DEPTH;
            // this is a refused finite chain, not a recursive identity cycle.
            // A provider may keep computing after refusal, but its fallback
            // must never become a completed declaration value.
            return Some(if self.pending_dependency.get() && key.index < 2009 {
                queries.value_declaration_type(chained_declaration(key.index + 1))
                    .unwrap_or(Type::Number)
            } else {
                Type::StringLiteral(b"settled".to_vec())
            });
        }
        assert_eq!(key, declaration());
        match self.contract {
            Contract::PendingMember => self.pending_member(1, queries),
            Contract::FlowDependency if self.pending_dependency.get() => {
                let result = self.read_loop(queries, 1);
                assert_eq!(result.ty, queries.union_reduced(&[Type::Number, Type::Nil]));
            }
            Contract::Recursive => {
                assert!(matches!(queries.value_declaration_type(declaration()),
                    Err(GlobalQueryError::CircularQuery(_))),
                    "the native global guard must stop provider reentry");
            }
            Contract::Missing => return None,
            _ => {}
        }
        Some(Type::Number)
    }
    fn value_declaration_initializer(&self, _: NativeValueDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        panic!("a written contract never needs an initializer")
    }
    fn global_query_error(&self, error: GlobalQueryError, _: &mut dyn TypeQueries) {
        self.errors.borrow_mut().push(error);
    }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool { key.snapshot == SNAPSHOT && key.index < 2 }
    fn type_declaration_header(&self, _: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: TypeDeclarationKind::Shape, name: "PendingMember".into(),
            object_form: Some(ObjectDeclarationForm::Record), parameters: Vec::new(), outer_parameters: Vec::new(),
        }
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> { vec![member(key.index)] }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool { key.snapshot == SNAPSHOT && key.index < 2 }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(member_declaration(key.index)),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(b"gate".to_vec())),
            visibility: TypeMemberVisibility::Public, optional: false, readonly: false,
        }
    }
    fn type_member_activation(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> TypeMemberActivation {
        if key.index == 0 || self.pending_dependency.get() { TypeMemberActivation::Pending } else { TypeMemberActivation::Active }
    }
    fn type_member_type(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type { Type::Number }
}

impl NativeFlowProvider for GlobalContexts {
    fn contains_graph(&self, key: NativeGraphKey) -> bool { key.snapshot == SNAPSHOT && key.region < 2 && key.context == 0 }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool { self.contains_graph(key.graph) && key.reference == 0 }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        if !self.contains_graph(key.graph) { return None; }
        let region = key.graph.region;
        Some(match key.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Loop { edges: vec![at(region, 0), at(region, 2)] },
            2 => NativeFlowNode::Write {
                before: at(region, 1), target: reference(region), value: value(region), input: at(region, 1),
            },
            _ => return None,
        })
    }
    fn is_shared(&self, key: NativeFlowKey) -> bool { key.node == 1 }
    fn write_affects(&self, reference: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if reference == target { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
    }
    fn value_type(&self, key: NativeValueKey, input: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type {
        assert_eq!(key, value(input.graph.region));
        let active = self.read_loop(queries, input.graph.region);
        assert!(active.incomplete, "the original backedge must read its in-progress loop");
        if input.graph.region == 0 {
            assert!(!self.inside_loop.replace(true));
            self.inherited_reads.set(self.inherited_reads.get() + 1);
            self.assert_current_queries(queries);
            self.inside_loop.set(false);
        }
        Type::Number
    }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects {
        panic!("this original graph contains no calls")
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("invalid original graph: {error:?}") }
}

fn inherited_argument(provider: &GlobalContexts, queries: &mut dyn TypeQueries) {
    let _ = queries.function_result_pack(&Type::NativeFunction(function(0)));
    assert!(provider.argument_checks.get() > 0, "the native argument must actually be checked");
    assert!(provider.inherited_reads.get() > 0, "the inherited pending dependency must actually be read");
}
fn assert_later_clean_cache(provider: &GlobalContexts, queries: &mut dyn TypeQueries) {
    let before = provider.contract_calls.get();
    for _ in 0..2 {
        let ty = queries.value_declaration_type(declaration()).unwrap();
        assert!(queries.is_assignable(&ty, &Type::Number));
        let ty = queries.global_binding_type(binding()).unwrap();
        assert!(!queries.is_assignable(&Type::String, &ty));
    }
    assert_eq!(provider.contract_calls.get(), before + 1,
        "a later clean query recomputes the original contract once, then caches it");
}

#[test]
fn declaration_globals_under_inherited_member_taint_are_current_only() {
    Engine::new().with_native_provider(
        || GlobalContexts::new(Inherited::Member, Contract::Constant),
        |provider, queries| {
            inherited_argument(provider, queries);
            assert!(provider.errors.borrow().is_empty(), "{:?}", provider.errors.borrow());
            assert_later_clean_cache(provider, queries);
        },
    );
}

#[test]
fn declaration_globals_under_inherited_loop_flow_are_current_only() {
    Engine::new().with_native_provider(
        || GlobalContexts::new(Inherited::Loop, Contract::Constant),
        |provider, queries| {
            inherited_argument(provider, queries);
            assert!(provider.errors.borrow().is_empty(), "{:?}", provider.errors.borrow());
            assert_later_clean_cache(provider, queries);
        },
    );
}

#[test]
fn a_fresh_pending_global_contract_is_not_completed_as_inherited_context() {
    Engine::new().with_native_provider(
        || GlobalContexts::new(Inherited::Member, Contract::PendingMember),
        |provider, queries| {
            inherited_argument(provider, queries);
            provider.pending_dependency.set(false);
            assert_later_clean_cache(provider, queries);
        },
    );
}

#[test]
fn a_fresh_flow_dependency_in_a_global_contract_is_not_completed_as_inherited_context() {
    Engine::new().with_native_provider(
        || GlobalContexts::new(Inherited::Member, Contract::FlowDependency),
        |provider, queries| {
            inherited_argument(provider, queries);
            provider.pending_dependency.set(false);
            assert_later_clean_cache(provider, queries);
        },
    );
}

#[test]
fn recursive_global_contracts_remain_unavailable_under_inherited_context() {
    Engine::new().with_native_provider(
        || GlobalContexts::new(Inherited::Member, Contract::Recursive),
        |provider, queries| {
            inherited_argument(provider, queries);
            assert!(provider.errors.borrow().iter().any(|error| matches!(error, GlobalQueryError::CircularQuery(_))));
        },
    );
}

#[test]
fn missing_global_declaration_contracts_are_not_accepted_as_current_only_values() {
    Engine::new().with_native_provider(
        || GlobalContexts::new(Inherited::Member, Contract::Missing),
        |provider, queries| inherited_argument(provider, queries),
    );
}

#[test]
fn implementation_global_contracts_do_not_use_declaration_only_completion() {
    Engine::new().with_native_provider(
        || GlobalContexts::new(Inherited::Member, Contract::Implementation),
        |provider, queries| inherited_argument(provider, queries),
    );
}

#[test]
fn refused_global_contracts_do_not_complete_or_cache_a_provider_fallback() {
    Engine::new().with_native_provider(
        || GlobalContexts::new(Inherited::Member, Contract::DepthRefusal),
        |provider, queries| {
            inherited_argument(provider, queries);
            assert!(provider.errors.borrow().iter().any(|error| matches!(error, GlobalQueryError::TooDeep(_))),
                "the distinct declaration chain must actually reach a native query refusal");
            assert!(!provider.errors.borrow().iter().any(|error| matches!(error, GlobalQueryError::CircularQuery(_))),
                "distinct original declaration identities must not form a recursive query");

            provider.pending_dependency.set(false);
            let before = provider.contract_calls.get();
            for _ in 0..2 {
                for ty in [
                    queries.value_declaration_type(declaration()).unwrap(),
                    queries.global_binding_type(binding()).unwrap(),
                ] {
                    assert_eq!(queries.inspect_type(&ty), Type::StringLiteral(b"settled".to_vec()));
                    assert!(!queries.is_assignable(&Type::Number, &ty),
                        "the refused provider's fallback must not survive in a completed cache");
                }
            }
            assert_eq!(provider.contract_calls.get(), before + 1,
                "the later short chain must recompute once and then use the completed cache");
        },
    );
}
