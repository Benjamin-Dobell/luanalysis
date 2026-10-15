use super::*;
use std::cell::{Cell, RefCell};

const SNAPSHOT: u64 = 9_841;
fn environment() -> NativeEnvironmentKey {
    NativeEnvironmentKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn binding(index: u32) -> NativeGlobalBindingKey {
    NativeGlobalBindingKey {
        environment: environment(),
        index,
    }
}
fn value(index: u32) -> NativeValueDeclarationKey {
    NativeValueDeclarationKey {
        snapshot: SNAPSHOT,
        index,
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
fn signature(index: u32) -> NativeSignatureKey {
    NativeSignatureKey {
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
fn named(index: u32) -> Type {
    Type::NativeNamedReference {
        declaration: declaration(index),
        arguments: vec![],
    }
}

struct Strings {
    selected: Option<NativeEnvironmentKey>,
    selected_binding: Option<NativeGlobalBindingKey>,
    namespace: Option<Type>,
    contract_reads: Cell<usize>,
    initializers: Cell<usize>,
    errors: RefCell<Vec<GlobalQueryError>>,
    pending_member: Cell<bool>,
}
impl Default for Strings {
    fn default() -> Self {
        Self {
            selected: Some(environment()),
            selected_binding: Some(binding(0)),
            namespace: None,
            contract_reads: Cell::new(0),
            initializers: Cell::new(0),
            errors: RefCell::new(vec![]),
            pending_member: Cell::new(false),
        }
    }
}
impl NativeTypeProvider for Strings {
    fn type_member_activation(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> TypeMemberActivation {
        if key == member(0) && self.pending_member.get() {
            TypeMemberActivation::Pending
        } else {
            TypeMemberActivation::Active
        }
    }
    fn primitive_member_environment(&self) -> Option<NativeEnvironmentKey> {
        self.selected
    }
    fn primitive_member_binding(
        &self,
        selected: NativeEnvironmentKey,
        kind: NativePrimitiveMemberKind,
    ) -> Option<NativeGlobalBindingKey> {
        assert_eq!(selected, self.selected.unwrap());
        assert_eq!(kind, NativePrimitiveMemberKind::String);
        self.selected_binding
    }
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool {
        key == environment()
    }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool {
        key.environment == environment() && key.index < 2
    }
    fn global_binding_declarations(
        &self,
        key: NativeGlobalBindingKey,
    ) -> Vec<NativeValueDeclarationKey> {
        vec![value(key.index)]
    }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader {
            role: GlobalSourceRole::Declaration,
            kind: GlobalValueKind::Variable,
        }
    }
    fn value_declaration_contract(
        &self,
        key: NativeValueDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.contract_reads.set(self.contract_reads.get() + 1);
        Some(if key.index == 0 {
            self.namespace.clone().unwrap_or_else(|| named(0))
        } else {
            named(1)
        })
    }
    fn value_declaration_initializer(
        &self,
        _: NativeValueDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        self.initializers.set(self.initializers.get() + 1);
        panic!("a primitive namespace must not evaluate its declaration stub")
    }
    fn global_query_error(&self, error: GlobalQueryError, _: &mut dyn TypeQueries) {
        self.errors.borrow_mut().push(error);
    }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 4
    }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: if key.index == 3 {
                TypeDeclarationKind::Alias
            } else {
                TypeDeclarationKind::Shape
            },
            name: format!("OriginalNamespace{}", key.index),
            object_form: (key.index != 3).then_some(ObjectDeclarationForm::Record),
            parameters: if key.index == 2 {
                vec![parameter()]
            } else {
                vec![]
            },
            outer_parameters: vec![],
        }
    }
    fn type_alias_target(&self, _: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        Type::String
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        match key.index {
            0 => vec![member(0), member(1)],
            1 => vec![member(2)],
            _ => vec![],
        }
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 3
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(declaration(u32::from(key.index == 2))),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(if key.index == 1 {
                b"sub".to_vec()
            } else {
                b"lower".to_vec()
            })),
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        }
    }
    fn type_member_type(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        Type::NativeSignature(signature(key.index))
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 3
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
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        let mut parameters = vec![Parameter {
            name: "s".into(),
            ty: if key.index == 2 {
                Type::Number
            } else {
                Type::String
            },
            optional: false,
        }];
        if key.index == 1 {
            parameters.push(Parameter {
                name: "i".into(),
                ty: Type::Number,
                optional: false,
            });
            parameters.push(Parameter {
                name: "j".into(),
                ty: Type::Number,
                optional: true,
            });
        }
        parameters
    }
    fn signature_declared_return_pack(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(if key.index == 2 {
            Type::Number
        } else {
            Type::String
        })
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key == parameter()
    }
    fn type_parameter_owner(&self, _: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(TypeDeclarationOwner::NamedType(declaration(2)))
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "Text".into()
    }
    fn type_parameter_siblings(&self, _: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        vec![parameter()]
    }
    fn has_type_parameter_constraint(&self, _: NativeTypeParameterKey) -> bool {
        true
    }
    fn type_parameter_constraint(
        &self,
        _: NativeTypeParameterKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        Some(Type::String)
    }
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("no invented function identity")
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("no runtime declaration")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("no runtime declaration")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no runtime declaration")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no runtime declaration")
    }
}

#[test]
fn primitive_string_members_retain_the_original_namespace_signature_capability() {
    Engine::new().with_native_provider(Strings::default, |provider, queries| {
        let namespace = queries.global_binding_type(binding(0)).unwrap();
        let original = queries.property_type(&namespace, "sub").unwrap();
        for receiver in [Type::String, Type::StringLiteral(b"source".to_vec())] {
            let method = queries.property_type(&receiver, "sub").unwrap();
            assert_eq!(method, original);
            assert_eq!(
                queries
                    .table_lookup(
                        &receiver,
                        &TableKey::Type(Type::StringLiteral(b"sub".to_vec()))
                    )
                    .unwrap(),
                original
            );
            assert_eq!(
                queries
                    .table_lookup(
                        &receiver,
                        &TableKey::Literal(LuaKey::String(b"sub".to_vec()))
                    )
                    .unwrap(),
                original
            );
            assert_eq!(
                queries.call_signature_types(&method),
                queries.call_signature_types(&original)
            );
            let result = queries.call_native_arguments(
                &method,
                None,
                &[
                    PackArgument::Resolved(receiver),
                    PackArgument::Resolved(Type::NumberLiteral(2.0)),
                ],
                &[],
                None,
            );
            assert!(result.errors.is_empty());
            assert_eq!(result.return_type, Type::String);
        }
        assert_eq!(provider.contract_reads.get(), 1);
        assert_eq!(provider.initializers.get(), 0);
        assert!(provider.errors.borrow().is_empty());
    });
}

#[test]
fn primitive_member_pending_context_stays_typed_and_is_not_cached_as_missing() {
    Engine::new().with_native_provider(
        || Strings { pending_member: Cell::new(true), ..Default::default() },
        |provider, queries| {
            let key = TableKey::Literal(LuaKey::String(b"lower".to_vec()));
            assert_eq!(queries.table_lookup(&Type::String, &key), Err(TableError::PendingContext));
            assert_eq!(queries.property_type_checked(&Type::String, "lower"), Err(TableError::PendingContext));
            provider.pending_member.set(false);
            let method = queries.table_lookup(&Type::String, &key).unwrap();
            let result = queries.call(&method, &[Type::String]);
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(result.return_type, Type::String);
        },
    );
}

#[test]
fn standalone_and_receiver_calls_share_argument_contracts_and_original_errors() {
    Engine::new().with_native_provider(Strings::default, |_, queries| {
        let namespace = queries.global_binding_type(binding(0)).unwrap();
        let standalone = queries.property_type(&namespace, "lower").unwrap();
        let method = queries.property_type(&Type::String, "lower").unwrap();
        assert_eq!(standalone, method);
        for callee in [standalone, method] {
            assert_eq!(
                queries.call(&callee, &[Type::String]).return_type,
                Type::String
            );
            assert!(matches!(
                queries.call(&callee, &[Type::Number]).errors.as_slice(),
                [CallError::Argument { index: 0, .. }]
            ));
            assert!(matches!(
                queries.call(&callee, &[]).errors.as_slice(),
                [CallError::Arity { .. }]
            ));
        }
        assert_eq!(queries.property_type(&Type::String, "doesNotExist"), None);
        assert_eq!(queries.property_type(&Type::String, "toUpperCase"), None);
        assert!(matches!(
            queries.table_lookup(
                &Type::String,
                &TableKey::Literal(LuaKey::String(b"toUpperCase".to_vec()))
            ),
            Err(TableError::MissingKey { .. })
        ));
        let alternatives = TableKey::Type(Type::Union(vec![
            Type::StringLiteral(b"lower".to_vec()),
            Type::StringLiteral(b"sub".to_vec()),
        ]));
        let members = queries.table_lookup(&Type::String, &alternatives).unwrap();
        assert!(!matches!(members, Type::Any | Type::Unknown));
    });
}

#[test]
fn string_literal_unions_and_string_constrained_generics_use_the_selected_namespace() {
    Engine::new().with_native_provider(Strings::default, |_, queries| {
        let original = queries.property_type(&Type::String, "lower").unwrap();
        for receiver in [
            Type::Union(vec![
                Type::StringLiteral(b"a".to_vec()),
                Type::StringLiteral(b"b".to_vec()),
            ]),
            Type::NativeTypeParameter(parameter()),
        ] {
            let method = queries.property_type(&receiver, "lower").unwrap();
            assert_eq!(method, original);
            assert_eq!(
                queries
                    .table_lookup(
                        &receiver,
                        &TableKey::Literal(LuaKey::String(b"lower".to_vec()))
                    )
                    .unwrap(),
                original
            );
            let result = queries.call(&method, &[receiver]);
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(result.return_type, Type::String);
        }
        assert_eq!(
            queries.property_type(&Type::Union(vec![Type::String, Type::Number]), "lower"),
            None
        );
        assert_eq!(
            queries.property_type(&Type::Union(vec![Type::String, Type::Nil]), "lower"),
            None
        );
    });
}

#[test]
fn selected_initial_namespace_is_independent_of_other_global_value_bindings() {
    Engine::new().with_native_provider(Strings::default, |provider, queries| {
        let original = queries.property_type(&Type::String, "lower").unwrap();
        // A lexical or global variable may select another original table value.
        // That value is not the capability registered for primitive metatables.
        let replacement = queries.global_binding_type(binding(1)).unwrap();
        let replacement_lower = queries.property_type(&replacement, "lower").unwrap();
        assert_ne!(original, replacement_lower);
        assert_eq!(
            queries
                .call(&replacement_lower, &[Type::Number])
                .return_type,
            Type::Number
        );
        assert_eq!(
            queries.property_type(&Type::String, "lower"),
            Some(original)
        );
        assert_eq!(provider.contract_reads.get(), 2);
    });
}

#[test]
fn missing_selection_keeps_the_backend_property_path_and_never_demands_a_namespace() {
    Engine::new().with_native_provider(
        || Strings {
            selected: None,
            ..Strings::default()
        },
        |provider, queries| {
            assert_eq!(queries.property_type(&Type::String, "lower"), None);
            assert_eq!(
                queries.property_type(
                    &Type::Shape(vec![Property::new("ordinary", Type::Number)]),
                    "ordinary"
                ),
                Some(Type::Number)
            );
            assert_eq!(provider.contract_reads.get(), 0);
            assert!(provider.errors.borrow().is_empty());
        },
    );
    Engine::new().with_session(|queries| {
        assert_eq!(
            queries.property_type(
                &Type::Shape(vec![Property::new("ordinary", Type::String)]),
                "ordinary"
            ),
            Some(Type::String)
        );
        assert_eq!(queries.property_type(&Type::String, "lower"), None);
    });
}

#[test]
fn invalid_primitive_environment_and_binding_capabilities_fail_existing_global_guards() {
    let foreign = NativeEnvironmentKey {
        snapshot: SNAPSHOT + 1,
        index: 0,
    };
    for (selected, selected_binding, expected) in [
        (
            foreign,
            binding(0),
            GlobalQueryError::InvalidEnvironment(foreign),
        ),
        (
            environment(),
            NativeGlobalBindingKey {
                environment: foreign,
                index: 0,
            },
            GlobalQueryError::InvalidBinding(NativeGlobalBindingKey {
                environment: foreign,
                index: 0,
            }),
        ),
        (
            environment(),
            binding(9),
            GlobalQueryError::InvalidBinding(binding(9)),
        ),
    ] {
        Engine::new().with_native_provider(
            || Strings {
                selected: Some(selected),
                selected_binding: Some(selected_binding),
                ..Strings::default()
            },
            |provider, queries| {
                assert_eq!(queries.property_type(&Type::String, "lower"), None);
                assert!(
                    provider.errors.borrow().contains(&expected),
                    "{:?}",
                    provider.errors.borrow()
                );
                assert_eq!(provider.contract_reads.get(), 0);
            },
        );
    }
}

#[test]
fn invalid_primitive_namespace_contracts_never_reenter_or_become_any_members() {
    for namespace in [
        Type::Any,
        Type::Unknown,
        Type::String,
        named(3),
        Type::RuntimeTable,
        Type::RuntimeFunction,
    ] {
        Engine::new().with_native_provider(
            || Strings {
                namespace: Some(namespace),
                ..Strings::default()
            },
            |provider, queries| {
                for _ in 0..2 {
                    assert_eq!(queries.property_type(&Type::String, "lower"), None);
                }
                assert!(
                    provider.errors.borrow().contains(
                        &GlobalQueryError::InvalidPrimitiveMemberNamespace {
                            binding: binding(0),
                            kind: NativePrimitiveMemberKind::String
                        }
                    ),
                    "{:?}",
                    provider.errors.borrow()
                );
                assert_eq!(provider.contract_reads.get(), 1);
                assert_eq!(provider.initializers.get(), 0);
            },
        );
    }
}
