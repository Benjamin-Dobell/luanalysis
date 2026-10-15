use crate::*;
use std::cell::RefCell;

fn declaration(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: 2801,
        index,
    }
}
fn member(snapshot: u64) -> NativeTypeMemberKey {
    NativeTypeMemberKey { snapshot, index: 0 }
}
fn signature(snapshot: u64) -> NativeSignatureKey {
    NativeSignatureKey { snapshot, index: 0 }
}
fn parameter(index: u32) -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: 2801,
        index,
    }
}
fn reference(index: u32, arguments: Vec<Type>) -> Type {
    Type::NativeNamedReference {
        declaration: declaration(index),
        arguments,
    }
}
fn definition() -> NativeClassDefinitionKey {
    NativeClassDefinitionKey {
        snapshot: 2801,
        index: 0,
    }
}

struct Member {
    key: NativeTypeMemberKey,
    header: TypeMemberHeader,
    signatures: Option<Vec<NativeSignatureKey>>,
}
struct Head {
    key: NativeSignatureKey,
    input: Type,
    output: Type,
}
struct Provider {
    members: Vec<Member>,
    heads: Vec<Head>,
    generic: bool,
    errors: RefCell<Vec<NamedTypeError>>,
}
impl Provider {
    fn new() -> Self {
        Self {
            members: [31, 42]
                .into_iter()
                .map(|snapshot| Member {
                    key: member(snapshot),
                    header: TypeMemberHeader {
                        owner: TypeMemberOwner::Declared(declaration(0)),
                        key: TypeMemberKeyRecipe::Fixed(LuaKey::String(b"choose".to_vec())),
                        visibility: TypeMemberVisibility::Public,
                        optional: false,
                        readonly: false,
                    },
                    signatures: Some(vec![signature(snapshot)]),
                })
                .collect(),
            heads: vec![
                Head {
                    key: signature(31),
                    input: Type::Number,
                    output: Type::String,
                },
                Head {
                    key: signature(42),
                    input: Type::String,
                    output: Type::Boolean,
                },
            ],
            generic: false,
            errors: RefCell::new(Vec::new()),
        }
    }
    fn member(&self, key: NativeTypeMemberKey) -> &Member {
        self.members
            .iter()
            .find(|source| source.key == key)
            .expect("original member")
    }
    fn head(&self, key: NativeSignatureKey) -> &Head {
        self.heads
            .iter()
            .find(|source| source.key == key)
            .expect("original head")
    }
}
impl NativeTypeProvider for Provider {
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        key.snapshot == 2801 && key.index < 4
    }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: if key.index == 2 {
                TypeDeclarationKind::Alias
            } else {
                TypeDeclarationKind::Class
            },
            name: format!("Class{}", key.index),
            object_form: (key.index != 2).then_some(ObjectDeclarationForm::Record),
            parameters: if self.generic && key.index < 2 {
                vec![parameter(key.index)]
            } else {
                vec![]
            },
            outer_parameters: vec![],
        }
    }
    fn type_alias_target(&self, _: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        reference(0, vec![])
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        if key.index == 0 || key.index == 3 {
            self.members.iter().map(|member| member.key).collect()
        } else {
            vec![]
        }
    }
    fn type_declaration_bases(
        &self,
        key: NativeTypeDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        if key.index == 1 {
            vec![reference(
                0,
                if self.generic {
                    vec![Type::NativeTypeParameter(parameter(1))]
                } else {
                    vec![]
                },
            )]
        } else {
            vec![]
        }
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        self.members.iter().any(|source| source.key == key)
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        self.member(key).header.clone()
    }
    fn type_member_signatures(&self, key: NativeTypeMemberKey) -> Option<Vec<NativeSignatureKey>> {
        self.member(key).signatures.clone()
    }
    fn type_member_type(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        assert!(
            self.member(key).signatures.is_none(),
            "pure members use native signature keys"
        );
        Type::RuntimeFunction
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        self.heads.iter().any(|head| head.key == key)
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::NamedType(declaration(0)),
            site: SignatureSite {
                snapshot: key.snapshot,
                index: 0,
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
        vec![Parameter {
            name: "value".into(),
            ty: self.head(key).input.clone(),
            optional: false,
        }]
    }
    fn signature_declared_return_pack(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(self.head(key).output.clone())
    }
    fn signature_outer_type_parameters(
        &self,
        _: NativeSignatureKey,
    ) -> Vec<NativeTypeParameterKey> {
        if self.generic {
            vec![parameter(0)]
        } else {
            vec![]
        }
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        self.generic && key.snapshot == 2801 && key.index < 2
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "T".into()
    }
    fn type_parameter_owner(&self, key: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(TypeDeclarationOwner::NamedType(declaration(key.index)))
    }
    fn contains_class_definition(&self, key: NativeClassDefinitionKey) -> bool {
        key == definition()
    }
    fn class_definition_header(&self, _: NativeClassDefinitionKey) -> ClassDefinitionHeader {
        ClassDefinitionHeader {
            declaration: declaration(0),
        }
    }
    fn type_declaration_error(&self, error: NamedTypeError) {
        self.errors.borrow_mut().push(error);
    }
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("no runtime function")
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("no runtime function")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("no runtime body")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no runtime function")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no runtime function")
    }
}
fn call(queries: &mut dyn TypeQueries, callable: &Type, argument: Type) -> Type {
    let called = queries.call_pack(callable, &[argument], &[], None);
    assert!(called.errors.is_empty(), "{:#?}", called.errors);
    queries.pack_slot(&called.return_pack, 0)
}
fn duplicate(result: Result<Type, NamedTypeError>) {
    assert!(
        matches!(
            result,
            Err(NamedTypeError::Table {
                error: TableError::DuplicateDeclaredField { .. },
                ..
            })
        ),
        "{result:#?}"
    );
}

#[test]
fn canonical_cross_file_pure_members_merge_for_lazy_reads_and_full_structural_checks() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            provider.members[1].header.owner = TypeMemberOwner::Reference {
                name: "Alias".into(),
                candidates: vec![declaration(2)],
            };
            provider
        },
        |provider, queries| {
            let class = queries.named_declared_type(declaration(0)).unwrap();
            let callable = queries.property_type(&class, "choose").unwrap();
            assert_eq!(queries.function_implementation_type(&callable), None);
            assert_eq!(call(queries, &callable, Type::Number), Type::String);
            assert_eq!(call(queries, &callable, Type::String), Type::Boolean);
            assert!(queries.validate_named_declaration(declaration(0)).is_ok());
            let expected = Type::Shape(vec![Property {
                name: "choose".into(),
                ty: Type::Function(FunctionType {
                    params: vec![Parameter {
                        name: "value".into(),
                        ty: Type::Number,
                        optional: false,
                    }],
                    returns: Box::new(Type::String),
                }),
                optional: false,
                readonly: false,
            }]);
            assert!(queries.is_assignable(&class, &expected));
            let original = queries.validate_named_member(member(31)).unwrap();
            assert_eq!(call(queries, &original, Type::Number), Type::String);
            assert!(
                !queries
                    .call_pack(&original, &[Type::String], &[], None)
                    .errors
                    .is_empty()
            );
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn inherited_pure_member_keeps_all_original_heads_and_composes_the_base_mapper() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            provider.generic = true;
            provider.heads[0].input = Type::NativeTypeParameter(parameter(0));
            provider.heads[0].output = Type::NativeTypeParameter(parameter(0));
            provider.heads[1].input = Type::Boolean;
            provider.heads[1].output = Type::Boolean;
            provider
        },
        |provider, queries| {
            let child = queries
                .named_type_reference(declaration(1), &[Type::Number])
                .unwrap();
            let callable = queries.property_type(&child, "choose").unwrap();
            assert_eq!(call(queries, &callable, Type::Number), Type::Number);
            assert_eq!(call(queries, &callable, Type::Boolean), Type::Boolean);
            assert!(
                !queries
                    .call_pack(&callable, &[Type::String], &[], None)
                    .errors
                    .is_empty()
            );
            assert!(queries.validate_named_declaration(declaration(1)).is_ok());
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn definition_context_keeps_complete_overload_schema_without_initializing_the_member() {
    Engine::new().with_native_provider(Provider::new, |_, queries| {
        let class = queries.named_declared_type(declaration(0)).unwrap();
        let context = queries
            .class_definition_context(definition(), &class)
            .unwrap();
        let optional = queries.property_type(&context, "choose").unwrap();
        assert!(queries.is_assignable(&Type::Nil, &optional));
        let callable = queries.exclude_type(&optional, &Type::Nil);
        assert_eq!(call(queries, &callable, Type::Number), Type::String);
        assert_eq!(call(queries, &callable, Type::String), Type::Boolean);
        let empty = queries
            .class_definition_value(definition(), &class, &Type::Table(TableType::default()))
            .unwrap();
        assert!(!queries.is_assignable(&empty, &class));
    });
}

#[test]
fn mixed_pure_and_ordinary_callable_fields_remain_duplicate_declarations() {
    for ordinary in 0..2 {
        Engine::new().with_native_provider(
            || {
                let mut provider = Provider::new();
                provider.members[ordinary].signatures = None;
                provider
            },
            |_, queries| {
                let class = queries.named_declared_type(declaration(0)).unwrap();
                assert!(queries.property_type(&class, "choose").is_none());
                duplicate(queries.validate_named_declaration(declaration(0)));
            },
        );
    }
}

#[test]
fn incompatible_optional_and_readonly_metadata_do_not_merge() {
    for readonly in [false, true] {
        Engine::new().with_native_provider(
            || {
                let mut provider = Provider::new();
                if readonly {
                    provider.members[1].header.readonly = true;
                } else {
                    provider.members[1].header.optional = true;
                }
                provider
            },
            |_, queries| {
                duplicate(queries.validate_named_declaration(declaration(0)));
            },
        );
    }
}

#[test]
fn invalid_group_contributors_report_their_original_member_identity() {
    for invalid in [None, Some(signature(99))] {
        Engine::new().with_native_provider(
            || {
                let mut provider = Provider::new();
                provider.members[1].signatures = Some(invalid.into_iter().collect());
                provider
            },
            |provider, queries| {
                let expected = NamedTypeError::InvalidMemberSignatures {
                    member: member(42),
                    signature: invalid,
                };
                assert_eq!(
                    queries.validate_named_declaration(declaration(0)),
                    Err(expected.clone())
                );
                assert!(provider.errors.borrow().contains(&expected));
            },
        );
    }
}

#[test]
fn canonical_owner_filtering_does_not_attach_pure_members_to_unrelated_classes() {
    Engine::new().with_native_provider(Provider::new, |_, queries| {
        let unrelated = queries.named_declared_type(declaration(3)).unwrap();
        assert!(queries.property_type(&unrelated, "choose").is_none());
        assert!(queries.validate_named_declaration(declaration(3)).is_ok());
    });
}

#[test]
fn grouping_is_metadata_only_and_lazy_reads_do_not_demand_unused_sibling_signatures() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let header = TypeMemberHeader {
                key: TypeMemberKeyRecipe::Fixed(LuaKey::String(b"unused".to_vec())),
                ..provider.members[0].header.clone()
            };
            provider.members.push(Member {
                key: member(53),
                header,
                signatures: Some(vec![signature(99)]),
            });
            provider
        },
        |_, queries| {
            let class = queries.named_declared_type(declaration(0)).unwrap();
            let callable = queries.property_type(&class, "choose").unwrap();
            assert_eq!(call(queries, &callable, Type::Number), Type::String);
            assert!(matches!(
                queries.validate_named_declaration(declaration(0)),
                Err(NamedTypeError::InvalidMemberSignatures {
                    member: NativeTypeMemberKey { snapshot: 53, .. },
                    ..
                })
            ));
        },
    );
}
