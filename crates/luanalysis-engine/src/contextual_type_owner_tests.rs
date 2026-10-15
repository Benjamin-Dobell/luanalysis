use crate::*;
use std::cell::{Cell, RefCell};

fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: 7031,
        index,
    }
}
fn declaration(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: 7031,
        index,
    }
}
fn member() -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: 7031,
        index: 0,
    }
}
fn parameter(index: u32) -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: 7031,
        index,
    }
}
fn input(context: u64) -> NativeFlowKey {
    NativeFlowKey {
        graph: NativeGraphKey {
            snapshot: 7031,
            region: 0,
            context,
        },
        node: 0,
    }
}
fn key() -> TableKey {
    TableKey::Literal(LuaKey::String(b"value".to_vec()))
}

#[derive(Default)]
struct Provider {
    pending: Cell<bool>,
    reenter: Cell<bool>,
    absent: Cell<bool>,
    ignore_pending_activation: bool,
    reads_sibling: bool,
    inactive_override: bool,
    calls: Cell<usize>,
    loop_counts: RefCell<Vec<usize>>,
    errors: RefCell<Vec<NamedTypeError>>,
}
impl NativeTypeProvider for Provider {
    fn contextual_type_owner(
        &self,
        scope: NativeFunctionKey,
        at: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> ContextualTypeOwner {
        self.calls.set(self.calls.get() + 1);
        if self.pending.get() {
            return ContextualTypeOwner::Pending;
        }
        if self.absent.get() || scope.index == 2 {
            return ContextualTypeOwner::Absent;
        }
        if self.reenter.get() {
            assert_eq!(
                queries.contextual_type_owner(scope, at),
                ContextualTypeOwner::Pending
            );
            // Deliberately attempt to return an owner after reading Pending:
            // native taint must still refuse to publish this result.
        }
        if self.reads_sibling {
            let class = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert_eq!(
                queries
                    .table_lookup(
                        &class,
                        &TableKey::Literal(LuaKey::String(b"saved".to_vec()))
                    )
                    .unwrap(),
                Type::Number
            );
        }
        let args = if scope.index == 0 {
            vec![if at.graph.context == 8 {
                Type::String
            } else {
                Type::Number
            }]
        } else {
            Vec::new()
        };
        ContextualTypeOwner::Owner(
            queries
                .named_type_reference(declaration(scope.index), &args)
                .unwrap(),
        )
    }
    fn type_member_activation(
        &self,
        member: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> TypeMemberActivation {
        if member.index == 1 {
            return TypeMemberActivation::Active;
        }
        if member.index == 2 {
            return TypeMemberActivation::Inactive;
        }
        match queries.contextual_type_owner(function(0), input(7)) {
            ContextualTypeOwner::Pending if self.ignore_pending_activation => {
                TypeMemberActivation::Inactive
            }
            ContextualTypeOwner::Pending => TypeMemberActivation::Pending,
            ContextualTypeOwner::Absent => TypeMemberActivation::Inactive,
            ContextualTypeOwner::Owner(_) => TypeMemberActivation::Active,
        }
    }
    fn contains_type_declaration(&self, id: NativeTypeDeclarationKey) -> bool {
        id.snapshot == 7031 && id.index < 3
    }
    fn type_declaration_header(&self, id: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            name: ["Base", "Derived", "Leaf"][id.index as usize].into(),
            kind: TypeDeclarationKind::Class,
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: if id.index == 0 {
                vec![parameter(0)]
            } else {
                Vec::new()
            },
            outer_parameters: Vec::new(),
        }
    }
    fn type_declaration_self_parameter(
        &self,
        id: NativeTypeDeclarationKey,
    ) -> Option<NativeTypeParameterKey> {
        Some(parameter(id.index + 1))
    }
    fn type_declaration_bases(
        &self,
        id: NativeTypeDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        if id.index == 1 {
            vec![Type::NativeNamedReference {
                declaration: declaration(0),
                arguments: vec![Type::Number],
            }]
        } else if id.index == 2 {
            vec![Type::NativeNamedReference {
                declaration: declaration(1),
                arguments: Vec::new(),
            }]
        } else {
            Vec::new()
        }
    }
    fn contains_type_member(&self, id: NativeTypeMemberKey) -> bool {
        id.snapshot == 7031
            && (id.index == 0
                || id.index == 1 && self.reads_sibling
                || id.index == 2 && self.inactive_override)
    }
    fn type_declaration_members(&self, id: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        if id.index == 0 {
            let mut members = vec![member()];
            if self.reads_sibling {
                members.push(NativeTypeMemberKey {
                    index: 1,
                    ..member()
                });
            }
            members
        } else if id.index == 1 && self.inactive_override {
            vec![NativeTypeMemberKey {
                index: 2,
                ..member()
            }]
        } else {
            Vec::new()
        }
    }
    fn type_member_header(&self, member: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(declaration(if member.index == 2 { 1 } else { 0 })),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(if member.index == 1 {
                b"saved".to_vec()
            } else {
                b"value".to_vec()
            })),
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        }
    }
    fn type_member_type(&self, member: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        match member.index {
            1 => Type::Number,
            2 => Type::String,
            _ => Type::NativeTypeParameter(parameter(0)),
        }
    }
    fn type_declaration_error(&self, error: NamedTypeError) {
        self.errors.borrow_mut().push(error);
    }
    fn contains_function(&self, id: NativeFunctionKey) -> bool {
        id.snapshot == 7031 && id.index < 3
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        Vec::new()
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        Some(Type::Number)
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        Type::Number
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("unexpected return cycle")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("unexpected return depth")
    }
    fn contains_type_parameter(&self, id: NativeTypeParameterKey) -> bool {
        id.snapshot == 7031 && id.index < 4
    }
    fn type_parameter_name(&self, id: NativeTypeParameterKey) -> String {
        if id.index == 0 {
            "T".into()
        } else {
            "self".into()
        }
    }
    fn type_parameter_owner(&self, id: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(TypeDeclarationOwner::NamedType(declaration(
            if id.index > 1 { id.index - 1 } else { 0 },
        )))
    }
    fn type_parameter_siblings(&self, id: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        vec![id]
    }
    fn has_type_parameter_constraint(&self, id: NativeTypeParameterKey) -> bool {
        id.index != 0
    }
    fn type_parameter_constraint(
        &self,
        id: NativeTypeParameterKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let args = if id.index > 1 {
            Vec::new()
        } else {
            vec![Type::NativeTypeParameter(parameter(0))]
        };
        Some(
            queries
                .named_type_reference(
                    declaration(if id.index > 1 { id.index - 1 } else { 0 }),
                    &args,
                )
                .unwrap(),
        )
    }
    fn contains_class_definition(&self, id: NativeClassDefinitionKey) -> bool {
        id.snapshot == 7031 && id.index == 0
    }
    fn class_definition_header(&self, _: NativeClassDefinitionKey) -> ClassDefinitionHeader {
        ClassDefinitionHeader {
            declaration: declaration(0),
        }
    }
}

fn owner(result: ContextualTypeOwner) -> Type {
    match result {
        ContextualTypeOwner::Owner(owner) => owner,
        other => panic!("expected owner: {other:?}"),
    }
}

#[test]
fn original_scope_and_frozen_body_facets_have_independent_completed_results() {
    Engine::new().with_native_provider(Provider::default, |provider, queries| {
        let number = owner(queries.contextual_type_owner(function(0), input(7)));
        let string = owner(queries.contextual_type_owner(function(0), input(8)));
        let derived = owner(queries.contextual_type_owner(function(1), input(7)));
        assert_eq!(queries.table_lookup(&number, &key()).unwrap(), Type::Number);
        assert_eq!(queries.table_lookup(&string, &key()).unwrap(), Type::String);
        assert_eq!(
            queries.named_object_declaration(&derived),
            Some(declaration(1))
        );
        let count = provider.calls.get();
        assert_eq!(
            queries.contextual_type_owner(function(0), input(7)),
            ContextualTypeOwner::Owner(number)
        );
        assert_eq!(provider.calls.get(), count);
        assert_eq!(
            queries.contextual_type_owner(function(2), input(7)),
            ContextualTypeOwner::Absent
        );
        let count = provider.calls.get();
        assert_eq!(
            queries.contextual_type_owner(function(2), input(7)),
            ContextualTypeOwner::Absent
        );
        assert_eq!(
            provider.calls.get(),
            count,
            "a completed absent recipe can be reused"
        );
    });
}

#[test]
fn pending_owner_does_not_publish_absence_or_an_empty_named_schema() {
    Engine::new().with_native_provider(
        || Provider {
            pending: Cell::new(true),
            ..Default::default()
        },
        |provider, queries| {
            let class = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert_eq!(
                queries.table_lookup(&class, &key()),
                Err(TableError::PendingContext)
            );
            assert!(matches!(
                queries.validate_named_declaration(declaration(0)),
                Err(NamedTypeError::PendingContext { .. })
            ));
            provider.pending.set(false);
            assert_eq!(queries.table_lookup(&class, &key()).unwrap(), Type::Number);
            let resolved = owner(queries.contextual_type_owner(function(0), input(7)));
            assert_eq!(
                queries.named_object_declaration(&resolved),
                Some(declaration(0))
            );
            assert!(
                provider
                    .errors
                    .borrow()
                    .iter()
                    .all(|error| matches!(error, NamedTypeError::PendingContext { .. }))
            );
        },
    );
}

#[test]
fn owner_recursion_taints_a_provider_result_even_when_it_ignores_pending() {
    Engine::new().with_native_provider(
        || Provider {
            reenter: Cell::new(true),
            ..Default::default()
        },
        |provider, queries| {
            assert_eq!(
                queries.contextual_type_owner(function(0), input(7)),
                ContextualTypeOwner::Pending
            );
            provider.reenter.set(false);
            let class = owner(queries.contextual_type_owner(function(0), input(7)));
            assert_eq!(queries.table_lookup(&class, &key()).unwrap(), Type::Number);
            assert_eq!(
                provider.calls.get(),
                2,
                "a recursive provisional result must be recomputed"
            );
        },
    );
}

#[test]
fn absent_activation_stays_a_real_missing_member() {
    Engine::new().with_native_provider(
        || Provider {
            absent: Cell::new(true),
            ..Default::default()
        },
        |_, queries| {
            let class = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert!(matches!(
                queries.table_lookup(&class, &key()),
                Err(TableError::MissingKey { .. })
            ));
        },
    );
}

#[test]
fn owner_can_read_a_known_sibling_without_activating_its_dependent_members() {
    Engine::new().with_native_provider(
        || Provider {
            reads_sibling: true,
            ..Default::default()
        },
        |provider, queries| {
            let class = owner(queries.contextual_type_owner(function(0), input(7)));
            assert_eq!(queries.table_lookup(&class, &key()).unwrap(), Type::Number);
            assert!(queries.validate_named_declaration(declaration(0)).is_ok());
            assert_eq!(
                provider.calls.get(),
                1,
                "the sibling proof must not cause contextual owner recursion"
            );
            assert!(
                provider.errors.borrow().is_empty(),
                "{:?}",
                provider.errors.borrow()
            );
        },
    );
}

#[test]
fn inactive_override_keeps_the_grandparent_member_visible_in_a_further_subclass() {
    Engine::new().with_native_provider(
        || Provider {
            inactive_override: true,
            ..Default::default()
        },
        |provider, queries| {
            let leaf = queries.named_type_reference(declaration(2), &[]).unwrap();
            assert_eq!(queries.table_lookup(&leaf, &key()).unwrap(), Type::Number);
            assert!(queries.validate_named_declaration(declaration(2)).is_ok());
            let base = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert!(queries.is_assignable(&leaf, &base));
            assert!(
                provider.errors.borrow().is_empty(),
                "{:?}",
                provider.errors.borrow()
            );
        },
    );
}

#[test]
fn activation_cannot_publish_absence_after_ignoring_an_incomplete_dependency() {
    Engine::new().with_native_provider(
        || Provider {
            pending: Cell::new(true),
            ignore_pending_activation: true,
            ..Default::default()
        },
        |provider, queries| {
            let class = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert_eq!(
                queries.table_lookup(&class, &key()),
                Err(TableError::PendingContext)
            );
            provider.pending.set(false);
            assert_eq!(queries.table_lookup(&class, &key()).unwrap(), Type::Number);
        },
    );
}

#[test]
fn contextual_semantic_self_keeps_actual_mapper_definition_and_derived_receiver() {
    Engine::new().with_native_provider(Provider::default, |provider, queries| {
        let class = owner(queries.contextual_type_owner(function(0), input(7)));
        let semantic_self = queries.contextual_owner_self_type(&class).unwrap();
        assert_eq!(
            queries.table_lookup(&semantic_self, &key()).unwrap(),
            Type::Number
        );
        assert_eq!(
            queries.contextual_owner_self_type(&semantic_self),
            Some(semantic_self.clone())
        );
        assert!(
            !queries.is_assignable(&class, &semantic_self),
            "semantic self is not replaced by its base constraint"
        );
        assert_eq!(
            queries.named_object_declaration(&Type::NativeTypeParameter(parameter(0))),
            None
        );
        assert_eq!(
            queries.contextual_owner_self_type(&Type::NativeTypeParameter(parameter(0))),
            None
        );
        let definition = queries
            .class_definition_value(
                NativeClassDefinitionKey {
                    snapshot: 7031,
                    index: 0,
                },
                &class,
                &Type::Table(TableType::default()),
            )
            .unwrap();
        assert_eq!(
            queries.named_object_declaration(&definition),
            Some(declaration(0))
        );
        assert!(queries.contextual_owner_self_type(&definition).is_some());
        let derived = owner(queries.contextual_type_owner(function(1), input(7)));
        let derived_value = queries.table_lookup(&derived, &key());
        assert!(
            queries.is_assignable(&derived, &class),
            "derived={} class={} value={derived_value:?} errors={:?}",
            queries.format(&derived),
            queries.format(&class),
            provider.errors.borrow()
        );
        let bound = queries.named_receiver_type(&class, &derived).unwrap();
        assert_eq!(queries.contextual_owner_self_type(&bound), Some(derived));
    });
}

impl NativeFlowProvider for Provider {
    fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        graph == input(7).graph
    }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
        self.contains_graph(reference.graph) && reference.reference == 0
    }
    fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
        if !self.contains_graph(at.graph) {
            return None;
        }
        let node = |node| NativeFlowKey { node, ..at };
        let reference = NativeReferenceKey {
            graph: at.graph,
            reference: 0,
        };
        let value = |index| NativeValueKey {
            snapshot: 7031,
            index,
        };
        Some(match at.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Write {
                before: node(0),
                target: reference,
                value: value(0),
                input: node(0),
            },
            2 => NativeFlowNode::Loop {
                edges: vec![node(1), node(3)],
            },
            3 => NativeFlowNode::Write {
                before: node(2),
                target: reference,
                value: value(1),
                input: node(2),
            },
            _ => return None,
        })
    }
    fn is_shared(&self, at: NativeFlowKey) -> bool {
        at.node == 2
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
    fn value_type(
        &self,
        value: NativeValueKey,
        _: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        assert!(matches!(
            queries.contextual_type_owner(function(0), input(7)),
            ContextualTypeOwner::Owner(_)
        ));
        let calls = self.calls.get();
        for _ in 0..8 {
            assert!(matches!(
                queries.contextual_type_owner(function(0), input(7)),
                ContextualTypeOwner::Owner(_)
            ));
        }
        assert_eq!(
            self.calls.get(),
            calls,
            "completed owner requests must reuse within one loop epoch"
        );
        self.loop_counts.borrow_mut().push(calls);
        if value.index == 0 {
            Type::Number
        } else {
            Type::String
        }
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
        panic!("unexpected native flow error: {error:?}")
    }
}

#[test]
fn native_loop_epochs_recompute_owner_queries_and_reuse_within_each_epoch() {
    Engine::new().with_native_provider(Provider::default, |provider, queries| {
        let declared = Type::Union(vec![Type::Number, Type::String, Type::Nil]);
        let result = queries.flow_type(
            provider,
            NativeFlowKey {
                node: 2,
                ..input(7)
            },
            NativeReferenceKey {
                graph: input(7).graph,
                reference: 0,
            },
            &declared,
            &Type::Nil,
        );
        assert!(queries.is_assignable(&Type::Number, &result.ty));
        assert!(queries.is_assignable(&Type::String, &result.ty));
        assert!(!queries.is_assignable(&Type::Nil, &result.ty));
        let counts = provider.loop_counts.borrow();
        assert!(counts.len() >= 2, "{counts:?}");
        assert!(
            counts[1] > counts[0],
            "a completed owner cannot cross a native backedge epoch: {counts:?}"
        );
    });
}
