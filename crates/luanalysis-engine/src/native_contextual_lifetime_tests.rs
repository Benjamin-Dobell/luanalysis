//! Contextual callback work follows the query-frame lifetime that assigned its
//! parameters, including lexical children created in that exact environment.
use super::*;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

const SNAPSHOT: u64 = 94_117;
fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn expression(node: u32) -> NativeExpressionKey {
    NativeExpressionKey {
        snapshot: SNAPSHOT,
        node,
    }
}
fn graph(region: u32) -> NativeGraphKey {
    NativeGraphKey {
        snapshot: SNAPSHOT,
        region,
        context: 0,
    }
}
fn at(region: u32, node: u32) -> NativeFlowKey {
    NativeFlowKey {
        graph: graph(region),
        node,
    }
}
fn reference(region: u32) -> NativeReferenceKey {
    NativeReferenceKey {
        graph: graph(region),
        reference: 0,
    }
}
fn value(region: u32) -> NativeValueKey {
    NativeValueKey {
        snapshot: SNAPSHOT,
        index: region,
    }
}
fn declaration() -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn member() -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}

#[derive(Clone, Copy, Debug)]
enum ContextSource {
    PendingMember,
    IncompleteLoop,
    IncompleteLoopWithPendingMember,
}

struct Contexts {
    source: ContextSource,
    region: Cell<u32>,
    created: RefCell<HashSet<u32>>,
    observed: RefCell<Vec<(Type, ContextualFunctionContext, Type)>>,
    inherited_reads: Cell<usize>,
}

impl Contexts {
    fn new(source: ContextSource) -> Self {
        Self {
            source,
            region: Cell::new(0),
            created: RefCell::default(),
            observed: RefCell::default(),
            inherited_reads: Cell::new(0),
        }
    }

    fn state(&self) -> NativeFunctionCheckState {
        match self.source {
            ContextSource::IncompleteLoop => NativeFunctionCheckState::Final,
            _ => NativeFunctionCheckState::Provisional,
        }
    }

    fn pending_member(&self, queries: &mut dyn TypeQueries) {
        assert_eq!(
            queries.property_type_checked(
                &Type::NativeNamedReference {
                    declaration: declaration(),
                    arguments: Vec::new(),
                },
                "gate"
            ),
            Err(TableError::PendingContext)
        );
    }

    fn callback(queries: &mut dyn TypeQueries) -> (Type, ContextualFunctionContext, Type) {
        let expected = Type::Function(FunctionType {
            params: vec![Parameter {
                name: "objectState".into(),
                ty: Type::Shape(vec![Property::new("GUID", Type::String)]),
                optional: false,
            }],
            returns: Box::new(Type::Boolean),
        });
        let callback = queries.contextual_function(
            function(2),
            expression(1),
            &expected,
            NativeCheckMode::CONTEXTUAL,
        );
        let capture = queries.function_capture_context(&callback).unwrap();
        let context = queries.captured_function_context(&capture).unwrap();
        let child = queries.captured_function_with_parent(function(3), &[], Some(&capture));
        (callback, context, child)
    }

    fn create_in_current_frame(&self, queries: &mut dyn TypeQueries) {
        if !self.created.borrow_mut().insert(self.region.get()) {
            return;
        }
        let previous = self.observed.borrow().clone();
        for (callback, context, child) in previous {
            let expected = match self.state() {
                NativeFunctionCheckState::Final => NativeFunctionCheckState::Final,
                _ => NativeFunctionCheckState::Expired,
            };
            assert_eq!(
                queries.contextual_function_check_state(&context),
                expected,
                "a later query at a reused stack depth must not revive the first frame's callback"
            );
            assert_eq!(queries.function_check_state(&callback), expected);
            assert_eq!(queries.function_check_state(&child), expected);
        }
        let (callback, context, child) = Self::callback(queries);
        assert_eq!(
            queries.contextual_function_check_state(&context),
            self.state()
        );
        assert_eq!(queries.function_check_state(&callback), self.state());
        assert_eq!(
            queries.function_check_state(&child),
            self.state(),
            "the child's own raw signature must not erase its lexical parent's lifetime"
        );
        let parameter = queries.contextual_parameter_type(&context, 0).unwrap();
        assert_eq!(
            queries.property_type(&parameter, "GUID"),
            Some(Type::String),
            "a provisional assignment remains usable inside its actual inference query"
        );
        self.observed.borrow_mut().push((callback, context, child));
    }

    fn read_loop(&self, queries: &mut dyn TypeQueries) -> NativeFlowResult {
        let region = self.region.get();
        queries.flow_type(
            self,
            at(region, 1),
            reference(region),
            &Type::Union(vec![Type::Number, Type::Nil]),
            &Type::Nil,
        )
    }
}

impl NativeTypeProvider for Contexts {
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 5
    }
    fn function_lexical_parent(&self, key: NativeFunctionKey) -> Option<NativeFunctionKey> {
        (key == function(3)).then_some(function(2))
    }
    fn function_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        match key.index {
            1 => vec![Parameter {
                name: "argument".into(),
                ty: Type::Number,
                optional: false,
            }],
            2 => vec![Parameter {
                name: "objectState".into(),
                ty: Type::Unknown,
                optional: false,
            }],
            _ => Vec::new(),
        }
    }
    fn contextual_parameter_indices(&self, key: NativeFunctionKey) -> Vec<usize> {
        if key == function(2) {
            vec![0]
        } else {
            Vec::new()
        }
    }
    fn function_parameters_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        assert_eq!(key, function(2));
        vec![Parameter {
            name: "objectState".into(),
            ty: queries.contextual_parameter_type(context, 0).unwrap(),
            optional: false,
        }]
    }
    fn function_declared_return(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        match key.index {
            1 => Some(Type::Number),
            2 | 3 => Some(Type::Boolean),
            _ => None,
        }
    }
    fn function_inferred_return(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        assert!(key == function(0) || key == function(4));
        let call = queries.call_native_arguments(
            &Type::NativeFunction(function(1)),
            None,
            &[PackArgument::Expression(expression(0))],
            &[],
            None,
        );
        assert!(call.errors.is_empty(), "{:?}", call.errors);
        Type::Number
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("the original function return is not recursive")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("the original query graph is finite")
    }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        key.snapshot == SNAPSHOT && key.node < 2
    }
    fn check_expression(
        &self,
        key: NativeExpressionKey,
        _: Option<&Type>,
        _: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        assert_eq!(key, expression(0));
        match self.source {
            ContextSource::PendingMember => {
                self.pending_member(queries);
                self.create_in_current_frame(queries);
            }
            _ => {
                self.read_loop(queries);
            }
        }
        Type::Number
    }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        key == declaration()
    }
    fn type_declaration_header(&self, _: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: TypeDeclarationKind::Shape,
            name: "Pending".into(),
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: Vec::new(),
            outer_parameters: Vec::new(),
        }
    }
    fn type_declaration_members(&self, _: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        vec![member()]
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        key == member()
    }
    fn type_member_header(&self, _: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(declaration()),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(b"gate".to_vec())),
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        }
    }
    fn type_member_activation(
        &self,
        _: NativeTypeMemberKey,
        _: &mut dyn TypeQueries,
    ) -> TypeMemberActivation {
        TypeMemberActivation::Pending
    }
    fn type_member_type(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        Type::Number
    }
}

impl NativeFlowProvider for Contexts {
    fn contains_graph(&self, key: NativeGraphKey) -> bool {
        key.snapshot == SNAPSHOT && key.region < 2 && key.context == 0
    }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool {
        self.contains_graph(key.graph) && key.reference == 0
    }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        if !self.contains_graph(key.graph) {
            return None;
        }
        let region = key.graph.region;
        Some(match key.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Loop {
                edges: vec![at(region, 0), at(region, 2)],
            },
            2 => NativeFlowNode::Write {
                before: at(region, 3),
                target: reference(region),
                value: value(region),
                input: at(region, 3),
            },
            3 => NativeFlowNode::Call {
                before: at(region, 1),
                value: value(region),
                input: at(region, 1),
            },
            _ => return None,
        })
    }
    fn is_shared(&self, key: NativeFlowKey) -> bool {
        key.node == 1
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
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        Type::Number
    }
    fn call_effects(
        &self,
        _: NativeValueKey,
        _: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        let active = self.read_loop(queries);
        assert!(
            active.incomplete,
            "the callback must be created under an actual in-progress flow-loop read"
        );
        self.inherited_reads.set(self.inherited_reads.get() + 1);
        if matches!(self.source, ContextSource::IncompleteLoopWithPendingMember) {
            self.pending_member(queries);
        }
        self.create_in_current_frame(queries);
        NativeCallEffects::None
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        panic!("invalid original graph: {error:?}")
    }
}

#[test]
fn contextual_callback_assignments_expire_with_their_exact_native_query_frame() {
    for source in [
        ContextSource::PendingMember,
        ContextSource::IncompleteLoop,
        ContextSource::IncompleteLoopWithPendingMember,
    ] {
        Engine::new().with_native_provider(
            || Contexts::new(source),
            |provider, queries| {
                for (region, outer) in [(0, 0), (1, 4)] {
                    provider.region.set(region);
                    queries
                        .function_result_pack(&Type::NativeFunction(function(outer)))
                        .unwrap();
                }
                assert_eq!(
                    provider.observed.borrow().len(),
                    2,
                    "both original query frames must actually assign a callback: {source:?}"
                );
                if !matches!(source, ContextSource::PendingMember) {
                    assert!(provider.inherited_reads.get() >= 2);
                }
                let expected = if matches!(source, ContextSource::IncompleteLoop) {
                    NativeFunctionCheckState::Final
                } else {
                    NativeFunctionCheckState::Expired
                };
                let observed = provider.observed.borrow().clone();
                for (callback, context, child) in observed {
                    assert_eq!(
                        queries.contextual_function_check_state(&context),
                        expected,
                        "{source:?}"
                    );
                    assert_eq!(
                        queries.function_check_state(&callback),
                        expected,
                        "{source:?}"
                    );
                    assert_eq!(queries.function_check_state(&child), expected, "{source:?}");
                }
                let (clean, context, child) = Contexts::callback(queries);
                assert_eq!(
                    queries.contextual_function_check_state(&context),
                    NativeFunctionCheckState::Final
                );
                assert_eq!(
                    queries.function_check_state(&clean),
                    NativeFunctionCheckState::Final
                );
                assert_eq!(
                    queries.function_check_state(&child),
                    NativeFunctionCheckState::Final
                );
            },
        );
    }
}
