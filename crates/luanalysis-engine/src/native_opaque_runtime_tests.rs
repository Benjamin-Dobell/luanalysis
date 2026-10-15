use super::*;
use std::cell::{Cell, RefCell};

const SNAPSHOT: u64 = 179301;
fn declaration(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn parameter() -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn member(index: u32) -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn reference(index: u32) -> Type {
    Type::NativeNamedReference {
        declaration: declaration(index),
        arguments: Vec::new(),
    }
}
fn signature() -> NativeSignatureKey {
    NativeSignatureKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
struct Provider {
    roots: bool,
    metadata_only: bool,
    ready: Cell<bool>,
    refuse: Cell<bool>,
    base_demands: Cell<usize>,
    errors: RefCell<Vec<NamedTypeError>>,
}
impl Provider {
    fn new(metadata_only: bool) -> Self {
        Self {
            roots: true,
            metadata_only,
            ready: Cell::new(false),
            refuse: Cell::new(false),
            base_demands: Cell::new(0),
            errors: RefCell::new(Vec::new()),
        }
    }
}
impl NativeTypeProvider for Provider {
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("runtime ancestry must not demand function parameters")
    }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("runtime ancestry must not demand function returns")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("runtime ancestry must not infer function bodies")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("runtime ancestry must not enter a function return cycle")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("runtime ancestry must not enter function return inference")
    }
    fn runtime_type_declaration(
        &self,
        kind: NativeOpaqueRuntimeKind,
    ) -> Option<NativeTypeDeclarationKey> {
        self.roots.then_some(declaration(match kind {
            NativeOpaqueRuntimeKind::Userdata => 0,
            NativeOpaqueRuntimeKind::Thread => 1,
        }))
    }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        key.snapshot == SNAPSHOT && (key.index < 9 || (100..2010).contains(&key.index))
    }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: if key.index >= 100 {
                TypeDeclarationKind::Alias
            } else if key.index == 7 {
                TypeDeclarationKind::Shape
            } else {
                TypeDeclarationKind::Class
            },
            // Root identity deliberately differs from the source spelling.
            name: match key.index {
                0 => "OpaqueMemory".into(),
                1 => "SchedulerValue".into(),
                4 => "userdata".into(),
                _ => format!("Class{}", key.index),
            },
            object_form: (key.index < 100).then_some(ObjectDeclarationForm::Record),
            parameters: if key.index == 2 {
                vec![parameter()]
            } else {
                Vec::new()
            },
            outer_parameters: Vec::new(),
        }
    }
    fn type_declaration_bases(
        &self,
        key: NativeTypeDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        match key.index {
            2 | 3 | 6 => vec![reference(0)],
            5 => vec![reference(5)],
            8 => {
                self.base_demands.set(self.base_demands.get() + 1);
                if self.refuse.get() {
                    // Distinct alias declarations force a genuine native query
                    // refusal; the callback still returns a fallback base.
                    let _ = queries.named_type_reference(declaration(100), &[]);
                    return vec![reference(0)];
                }
                if self.ready.get() {
                    return vec![reference(1)];
                }
                // An actually unfinished provider dependency must taint the
                // category query even though its provisional base is known.
                let _ = queries.property_type_checked(&reference(7), "pending");
                vec![reference(0)]
            }
            _ => Vec::new(),
        }
    }
    fn type_alias_target(
        &self,
        key: NativeTypeDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        assert!((100..2010).contains(&key.index));
        if key.index == 2009 {
            Type::Number
        } else {
            queries
                .named_type_reference(declaration(key.index + 1), &[])
                .unwrap_or(Type::Number)
        }
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        assert!(
            !self.metadata_only,
            "runtime category demanded member inference"
        );
        match key.index {
            2 => vec![member(0)],
            7 => vec![member(1)],
            _ => Vec::new(),
        }
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(declaration(if key.index == 0 { 2 } else { 7 })),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(if key.index == 0 {
                b"value".to_vec()
            } else {
                b"pending".to_vec()
            })),
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
        if key.index == 1 && !self.ready.get() {
            TypeMemberActivation::Pending
        } else {
            TypeMemberActivation::Active
        }
    }
    fn type_member_type(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        if key.index == 0 {
            Type::NativeTypeParameter(parameter())
        } else {
            Type::Number
        }
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key == parameter()
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "T".into()
    }
    fn type_parameter_owner(&self, _: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(TypeDeclarationOwner::NamedType(declaration(2)))
    }
    fn type_parameter_siblings(&self, _: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        vec![parameter()]
    }
    fn type_declaration_call_signatures(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> Vec<NativeSignatureKey> {
        assert!(
            !self.metadata_only,
            "runtime category demanded callable contracts"
        );
        if key.index == 6 {
            vec![signature()]
        } else {
            Vec::new()
        }
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key == signature()
    }
    fn signature_header(&self, _: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::NamedType(declaration(6)),
            site: SignatureSite {
                snapshot: SNAPSHOT,
                index: 0,
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
        Vec::new()
    }
    fn signature_declared_return_pack(
        &self,
        _: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(Type::Number)
    }
    fn contains_class_definition(&self, key: NativeClassDefinitionKey) -> bool {
        key.snapshot == SNAPSHOT && key.index == 6
    }
    fn class_definition_header(&self, _: NativeClassDefinitionKey) -> ClassDefinitionHeader {
        ClassDefinitionHeader {
            declaration: declaration(6),
        }
    }
    fn type_declaration_error(&self, error: NamedTypeError) {
        self.errors.borrow_mut().push(error);
    }
}
struct Graph {
    context: u64,
    value: Type,
    kind: NativeRuntimeType,
}
impl Graph {
    fn graph(&self) -> NativeGraphKey {
        NativeGraphKey {
            snapshot: SNAPSHOT,
            region: 0,
            context: self.context,
        }
    }
    fn at(&self, node: u32) -> NativeFlowKey {
        NativeFlowKey {
            graph: self.graph(),
            node,
        }
    }
    fn reference(&self) -> NativeReferenceKey {
        NativeReferenceKey {
            graph: self.graph(),
            reference: 0,
        }
    }
    fn narrowed(&self, queries: &mut dyn TypeQueries, sense: bool) -> NativeFlowResult {
        queries.flow_type(
            self,
            self.at(if sense { 1 } else { 2 }),
            self.reference(),
            &self.value,
            &self.value,
        )
    }
}
impl NativeFlowProvider for Graph {
    fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        graph == self.graph()
    }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
        reference == self.reference()
    }
    fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
        if at.graph != self.graph() {
            return None;
        }
        Some(match at.node {
            0 => NativeFlowNode::Start,
            1 | 2 => NativeFlowNode::Predicate {
                before: self.at(0),
                predicate: NativePredicate::RuntimeType {
                    subject: self.reference(),
                    value: NativeValueKey {
                        snapshot: SNAPSHOT,
                        index: 0,
                    },
                    input: self.at(0),
                    kind: self.kind.clone(),
                    equals: true,
                },
                sense: at.node == 1,
            },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool {
        false
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
        self.value.clone()
    }
    fn call_effects(
        &self,
        _: NativeValueKey,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        NativeCallEffects::None
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        panic!("unexpected domain flow error: {error:?}");
    }
}

#[test]
fn opaque_categories_use_original_roots_and_bases_without_member_or_call_demands() {
    Engine::new().with_native_provider(
        || Provider::new(true),
        |_, queries| {
            let child = queries
                .named_type_reference(declaration(2), &[Type::Number])
                .unwrap();
            let thread = queries.named_type_reference(declaration(1), &[]).unwrap();
            let value = Type::Union(vec![
                child.clone(),
                thread.clone(),
                Type::Number,
                Type::RuntimeTable,
                Type::RuntimeFunction,
            ]);
            for (context, kind, expected) in [
                (1, NativeRuntimeType::Userdata, child),
                (2, NativeRuntimeType::Thread, thread),
            ] {
                let graph = Graph {
                    context,
                    value: value.clone(),
                    kind,
                };
                let yes = graph.narrowed(queries, true);
                assert!(!yes.incomplete);
                assert_eq!(yes.ty, expected);
                let no = graph.narrowed(queries, false);
                assert!(!no.incomplete);
                assert!(!queries.is_assignable(&expected, &no.ty));
            }
            let impostor = queries.named_type_reference(declaration(4), &[]).unwrap();
            let graph = Graph {
                context: 3,
                value: impostor.clone(),
                kind: NativeRuntimeType::Userdata,
            };
            assert_eq!(graph.narrowed(queries, true).ty, Type::Never);
            assert_eq!(graph.narrowed(queries, false).ty, impostor);
        },
    );
}

#[test]
fn opaque_unknown_and_type_parameters_keep_the_nominal_root_intersection() {
    Engine::new().with_native_provider(
        || Provider::new(true),
        |_, queries| {
            let root = queries.named_type_reference(declaration(0), &[]).unwrap();
            let graph = Graph {
                context: 4,
                value: Type::Unknown,
                kind: NativeRuntimeType::Userdata,
            };
            assert_eq!(graph.narrowed(queries, true).ty, root.clone());
            assert_eq!(graph.narrowed(queries, false).ty, Type::Unknown);
            let formal = Type::NativeTypeParameter(parameter());
            let expected = queries.regular_type(&Type::Intersection(vec![formal.clone(), root]));
            let graph = Graph {
                context: 5,
                value: formal.clone(),
                kind: NativeRuntimeType::Userdata,
            };
            assert_eq!(graph.narrowed(queries, true).ty, expected);
            assert_eq!(
                graph.narrowed(queries, false).ty,
                queries.regular_type(&formal)
            );
        },
    );
}

#[test]
fn opaque_instances_keep_mapped_members_and_calls_but_namespaces_are_physical_tables() {
    Engine::new().with_native_provider(
        || Provider::new(false),
        |_, queries| {
            let child = queries
                .named_type_reference(declaration(2), &[Type::Number])
                .unwrap();
            assert_eq!(queries.property_type(&child, "value"), Some(Type::Number));
            assert!(!queries.is_assignable(&child, &Type::RuntimeTable));
            assert!(!queries.is_assignable(&child, &Type::RuntimeFunction));
            assert!(!queries.is_assignable(&Type::Table(TableType::default()), &child));
            let root = queries.named_type_reference(declaration(0), &[]).unwrap();
            assert!(
                !queries.is_assignable(&root, &child),
                "a category grants no particular subtype"
            );
            let callable = queries.named_type_reference(declaration(6), &[]).unwrap();
            let called = queries.call(&callable, &[]);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(called.return_type, Type::Number);
            assert!(!queries.is_assignable(&callable, &Type::RuntimeFunction));
            let namespace = queries
                .class_definition_value(
                    NativeClassDefinitionKey {
                        snapshot: SNAPSHOT,
                        index: 6,
                    },
                    &callable,
                    &Type::Table(TableType::default()),
                )
                .unwrap();
            assert!(queries.is_assignable(&namespace, &Type::RuntimeTable));
            assert!(
                !queries.is_assignable(&namespace, &callable),
                "namespace tables are not userdata instances"
            );
            let graph = Graph {
                context: 6,
                value: Type::Union(vec![namespace.clone(), callable.clone()]),
                kind: NativeRuntimeType::Userdata,
            };
            assert_eq!(graph.narrowed(queries, true).ty, callable);
            assert_eq!(graph.narrowed(queries, false).ty, namespace);
        },
    );
}

#[test]
fn opaque_root_absence_and_cyclic_or_pending_bases_are_not_guessed() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new(true);
            provider.roots = false;
            provider
        },
        |_, queries| {
            let graph = Graph {
                context: 7,
                value: Type::Unknown,
                kind: NativeRuntimeType::Userdata,
            };
            for _ in 0..3 {
                assert_eq!(graph.narrowed(queries, true).ty, Type::Unknown);
            }
        },
    );
    Engine::new().with_native_provider(
        || Provider::new(false),
        |provider, queries| {
            let graph = Graph {
                context: 8,
                value: reference(5),
                kind: NativeRuntimeType::Userdata,
            };
            for _ in 0..3 {
                let result = graph.narrowed(queries, true);
                assert_ne!(
                    result.ty,
                    Type::Never,
                    "a cyclic declaration is not a proved non-userdata arm"
                );
            }
            assert!(
                provider
                    .errors
                    .borrow()
                    .iter()
                    .any(|error| matches!(error, NamedTypeError::CircularBase(_)))
            );
        },
    );
    Engine::new().with_native_provider(
        || Provider::new(false),
        |provider, queries| {
            let graph = Graph {
                context: 9,
                value: reference(8),
                kind: NativeRuntimeType::Thread,
            };
            let original = queries.named_type_reference(declaration(8), &[]).unwrap();
            for _ in 0..3 {
                let result = graph.narrowed(queries, true);
                assert!(
                    result.incomplete,
                    "unfinished base dependency remains Pending"
                );
                assert_eq!(
                    result.ty, original,
                    "the provisional userdata base cannot exclude a possible thread"
                );
            }
            assert!(
                provider.base_demands.get() >= 3,
                "pending bases must not enter a completed cache"
            );
            provider.ready.set(true);
            let result = graph.narrowed(queries, true);
            assert!(!result.incomplete);
            assert_eq!(result.ty, original);
            let before = provider.base_demands.get();
            assert!(!graph.narrowed(queries, true).incomplete);
            assert_eq!(
                provider.base_demands.get(),
                before,
                "completed bases can use their normal cache"
            );
        },
    );
}

#[test]
fn refused_domain_dependencies_never_publish_the_callback_fallback() {
    Engine::new().with_native_provider(
        || {
            let provider = Provider::new(false);
            provider.refuse.set(true);
            provider
        },
        |provider, queries| {
            let graph = Graph {
                context: 10,
                value: reference(8),
                kind: NativeRuntimeType::Thread,
            };
            let original = queries.named_type_reference(declaration(8), &[]).unwrap();
            for _ in 0..2 {
                let result = graph.narrowed(queries, true);
                assert!(result.incomplete);
                assert_eq!(
                    result.ty, original,
                    "refused aliases cannot attest the fallback userdata base"
                );
            }
            assert!(
                provider
                    .errors
                    .borrow()
                    .iter()
                    .any(|error| matches!(error, NamedTypeError::TooDeep(_))),
                "the finite original alias chain must actually reach native refusal"
            );
            provider.refuse.set(false);
            provider.ready.set(true);
            let result = graph.narrowed(queries, true);
            assert!(!result.incomplete);
            assert_eq!(result.ty, original);
        },
    );
}

#[test]
fn materialized_host_namespaces_keep_the_definition_facet_in_runtime_predicates() {
    with_program(|program, handles| {
        let provider = Provider::new(false);
        let adapter = ProviderAdapter {
            provider: &provider,
            handles,
        };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        checker.enable_native_arrays();
        let class = checker
            .native_type_reference(native_named::core_declaration(declaration(6)), &[])
            .unwrap();
        let actual = import_type(&mut checker, handles, &Type::Table(TableType::default()));
        let namespace = checker
            .native_definition_value(
                bun_sema::types::NativeDefinitionId {
                    snapshot: SNAPSHOT,
                    index: 6,
                },
                class,
                actual,
            )
            .unwrap();
        let materialized = checker.native_object_resolved(namespace).unwrap();
        let table = checker.native_runtime_top(bun_sema::types::NativeRuntimeKind::Table);
        for (index, namespace) in [namespace, materialized].into_iter().enumerate() {
            assert!(checker.is_assignable(namespace, table));
            assert!(!checker.is_assignable(namespace, class));
            let value = handles.retain(namespace);
            let mut queries = SessionQueries {
                checker: &mut checker,
                handles,
            };
            let graph = Graph {
                context: 11 + index as u64,
                value: value.clone(),
                kind: NativeRuntimeType::Userdata,
            };
            assert_eq!(graph.narrowed(&mut queries, true).ty, Type::Never);
            assert_eq!(graph.narrowed(&mut queries, false).ty, value);
        }
    });
}
