//! Cache-policy taint is inherited through a real native argument query.
//! These providers use original function/expression/flow identities; no TS AST
//! or synthetic frontend type stands in for an unresolved query.
use super::*;
use std::cell::Cell;

const SNAPSHOT: u64 = 12_439;
fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn expression() -> NativeExpressionKey {
    NativeExpressionKey {
        snapshot: SNAPSHOT,
        node: 0,
    }
}
fn graph() -> NativeGraphKey {
    NativeGraphKey {
        snapshot: SNAPSHOT,
        region: 0,
        context: 0,
    }
}
fn at(node: u32) -> NativeFlowKey {
    NativeFlowKey {
        graph: graph(),
        node,
    }
}
fn reference(index: u32) -> NativeReferenceKey {
    NativeReferenceKey {
        graph: graph(),
        reference: index,
    }
}
fn value(index: u32) -> NativeValueKey {
    NativeValueKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn environment() -> NativeEnvironmentKey {
    NativeEnvironmentKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn declaration(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn member(index: u32) -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: SNAPSHOT,
        index,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    Constant,
    PendingMember,
    Recursive,
}

struct ContextQueries {
    effect: Effect,
    effect_calls: Cell<usize>,
    origin_calls: Cell<usize>,
    argument_checks: Cell<usize>,
    inherited_pending_reads: Cell<usize>,
    pending_member: Cell<bool>,
    inside_effect: Cell<bool>,
}
impl ContextQueries {
    fn new(effect: Effect) -> Self {
        Self {
            effect,
            effect_calls: Cell::new(0),
            origin_calls: Cell::new(0),
            argument_checks: Cell::new(0),
            inherited_pending_reads: Cell::new(0),
            pending_member: Cell::new(true),
            inside_effect: Cell::new(false),
        }
    }
    fn read(&self, queries: &mut dyn TypeQueries) -> NativeFlowResult {
        queries.flow_type(self, at(1), reference(0), &Type::Number, &Type::Number)
    }
}

impl NativeTypeProvider for ContextQueries {
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn function_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        if key == function(1) {
            vec![Parameter {
                name: "argument".into(),
                ty: Type::Number,
                optional: false,
            }]
        } else {
            Vec::new()
        }
    }
    fn function_declared_return(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        (key == function(1)).then_some(Type::Number)
    }
    fn function_inferred_return(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        assert_eq!(key, function(0));
        let result = queries.call_native_arguments(
            &Type::NativeFunction(function(1)),
            None,
            &[PackArgument::Expression(expression())],
            &[],
            None,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        Type::Number
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("the original return provider is not recursive")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("the original return provider is finite")
    }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        key == expression()
    }
    fn check_expression_pack(
        &self,
        key: NativeExpressionKey,
        _: Option<&ValuePack>,
        _: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        assert_eq!(key, expression());
        self.argument_checks.set(self.argument_checks.get() + 1);
        // This real original member is pending while the native argument's
        // inference scope is active. It taints the outer return query; the
        // independent effect and origin queries below inherit cache policy.
        assert_eq!(
            queries.property_type_checked(
                &Type::NativeNamedReference {
                    declaration: declaration(0),
                    arguments: Vec::new()
                },
                "gate"
            ),
            Err(TableError::PendingContext)
        );
        self.inherited_pending_reads
            .set(self.inherited_pending_reads.get() + 1);
        let before = self.effect_calls.get();
        for _ in 0..2 {
            let result = self.read(queries);
            assert_eq!(result.ty, Type::Number);
            let pending = self.effect != Effect::Constant;
            assert_eq!(
                result.incomplete, pending,
                "only a fresh semantic dependency makes this flow incomplete"
            );
        }
        if self.effect == Effect::Constant {
            assert_eq!(
                self.effect_calls.get(),
                before + 2,
                "current-only effects must not enter a shared completed cache"
            );
            let before = self.origin_calls.get();
            for _ in 0..2 {
                assert_eq!(
                    queries.flow_standard_operation(self, value(10), at(0)),
                    NativeStandardOperation::Ordinary
                );
            }
            assert_eq!(
                self.origin_calls.get(),
                before + 2,
                "current-only standard origins must not enter a shared cache"
            );
        }
        ValuePack::single(Type::Number)
    }
    fn assumed_standard_environment(&self) -> Option<NativeEnvironmentKey> {
        Some(environment())
    }
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool {
        key == environment()
    }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn type_declaration_header(&self, _: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: TypeDeclarationKind::Shape,
            name: "PendingMember".into(),
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: Vec::new(),
            outer_parameters: Vec::new(),
        }
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        vec![member(key.index)]
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(declaration(key.index)),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(b"gate".to_vec())),
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        }
    }
    fn type_member_activation(
        &self,
        key: NativeTypeMemberKey,
        _: &mut dyn TypeQueries,
    ) -> TypeMemberActivation {
        if key.index == 0 || self.pending_member.get() {
            TypeMemberActivation::Pending
        } else {
            TypeMemberActivation::Active
        }
    }
    fn type_member_type(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        Type::Number
    }
}

impl NativeFlowProvider for ContextQueries {
    fn contains_graph(&self, key: NativeGraphKey) -> bool {
        key == graph()
    }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool {
        key.graph == graph() && key.reference < 2
    }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        if key.graph != graph() {
            return None;
        }
        match key.node {
            0 => Some(NativeFlowNode::Start),
            1 => Some(NativeFlowNode::Call {
                before: at(0),
                value: value(0),
                input: at(0),
            }),
            _ => None,
        }
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool {
        false
    }
    fn write_affects(
        &self,
        _: NativeReferenceKey,
        _: NativeReferenceKey,
        _: NativeFlowKey,
    ) -> NativeWriteRelation {
        NativeWriteRelation::Unaffected
    }
    fn value_alias_source(&self, key: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        assert_eq!(key, value(10));
        self.origin_calls.set(self.origin_calls.get() + 1);
        NativeAliasValue::Reference {
            reference: reference(1),
            input,
        }
    }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        panic!("a constant explicit call effect does not infer a value")
    }
    fn call_effects(
        &self,
        _: NativeValueKey,
        _: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        self.effect_calls.set(self.effect_calls.get() + 1);
        match self.effect {
            Effect::Constant => {}
            Effect::PendingMember => {
                let actual = queries.property_type_checked(
                    &Type::NativeNamedReference {
                        declaration: declaration(1),
                        arguments: Vec::new(),
                    },
                    "gate",
                );
                if self.pending_member.get() {
                    assert_eq!(actual, Err(TableError::PendingContext));
                } else {
                    assert_eq!(actual, Ok(Some(Type::Number)));
                }
            }
            Effect::Recursive => {
                assert!(
                    !self.inside_effect.replace(true),
                    "the native query guard must stop provider reentry"
                );
                assert!(self.read(queries).incomplete);
                self.inside_effect.set(false);
            }
        }
        NativeCallEffects::None
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        panic!("invalid original graph: {error:?}")
    }
}

fn tainted_argument(provider: &ContextQueries, queries: &mut dyn TypeQueries) {
    let _ = queries.function_result_pack(&Type::NativeFunction(function(0)));
    assert!(
        provider.argument_checks.get() > 0,
        "the native original argument must actually be checked"
    );
    assert!(
        provider.inherited_pending_reads.get() > 0,
        "the enclosing native return query must actually be tainted"
    );
}

#[test]
fn inherited_cache_policy_taint_keeps_completed_native_results_current_and_uncached() {
    Engine::new().with_native_provider(
        || ContextQueries::new(Effect::Constant),
        |provider, queries| {
            tainted_argument(provider, queries);
            let before = provider.effect_calls.get();
            assert!(!provider.read(queries).incomplete);
            assert_eq!(
                provider.effect_calls.get(),
                before + 1,
                "a later clean context must recompute the effect"
            );
            assert!(!provider.read(queries).incomplete);
            assert_eq!(
                provider.effect_calls.get(),
                before + 1,
                "ordinary completed effects can still be cached"
            );
            let before = provider.origin_calls.get();
            assert_eq!(
                queries.flow_standard_operation(provider, value(10), at(0)),
                NativeStandardOperation::Ordinary
            );
            assert_eq!(provider.origin_calls.get(), before + 1);
            assert_eq!(
                queries.flow_standard_operation(provider, value(10), at(0)),
                NativeStandardOperation::Ordinary
            );
            assert_eq!(
                provider.origin_calls.get(),
                before + 1,
                "ordinary completed origins can still be cached"
            );
        },
    );
}

#[test]
fn a_fresh_pending_member_is_not_swallowed_as_inherited_cache_policy() {
    Engine::new().with_native_provider(
        || ContextQueries::new(Effect::PendingMember),
        |provider, queries| {
            tainted_argument(provider, queries);
            provider.pending_member.set(false);
            let before = provider.effect_calls.get();
            assert!(!provider.read(queries).incomplete);
            assert!(
                provider.effect_calls.get() > before,
                "a provisional effect must not become a completed-cache hit"
            );
        },
    );
}

#[test]
fn a_fresh_recursive_effect_stays_incomplete_with_and_without_ambient_inference() {
    Engine::new().with_native_provider(
        || ContextQueries::new(Effect::Recursive),
        |provider, queries| {
            tainted_argument(provider, queries);
            assert!(provider.read(queries).incomplete);
            assert!(provider.read(queries).incomplete);
        },
    );
}
