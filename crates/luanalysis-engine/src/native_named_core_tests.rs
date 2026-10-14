use super::*;
use std::cell::RefCell;

fn declaration(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: 777,
        index,
    }
}
fn member(index: u32) -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: 777,
        index,
    }
}
fn parameter(index: u32) -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: 777,
        index,
    }
}
fn function() -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: 777,
        index: 0,
    }
}
fn reference(index: u32, arguments: Vec<Type>) -> Type {
    Type::NativeNamedReference {
        declaration: declaration(index),
        arguments,
    }
}
fn field(key: &str, value: Type) -> TableField {
    TableField::new(LuaKey::String(key.as_bytes().to_vec()), value)
}
fn table(fields: Vec<TableField>) -> Type {
    Type::Table(TableType {
        fields,
        ..TableType::default()
    })
}

struct Declaration {
    key: NativeTypeDeclarationKey,
    header: TypeDeclarationHeader,
    target: Option<Type>,
    members: Vec<u32>,
    bases: Vec<Type>,
}
impl Declaration {
    fn alias(index: u32, target: Type) -> Self {
        Self {
            key: declaration(index),
            header: TypeDeclarationHeader {
                kind: TypeDeclarationKind::Alias,
                name: format!("Alias{index}"),
                object_form: None,
                parameters: vec![],
                outer_parameters: vec![],
            },
            target: Some(target),
            members: vec![],
            bases: vec![],
        }
    }
    fn record(index: u32, members: Vec<u32>) -> Self {
        Self {
            key: declaration(index),
            header: TypeDeclarationHeader {
                kind: TypeDeclarationKind::Shape,
                name: format!("Record{index}"),
                object_form: Some(ObjectDeclarationForm::Record),
                parameters: vec![],
                outer_parameters: vec![],
            },
            target: None,
            members,
            bases: vec![],
        }
    }
}
struct Member {
    header: TypeMemberHeader,
    value: Option<Type>,
    key: Option<Type>,
}
impl Member {
    fn field(owner: u32, key: LuaKey, value: Type) -> Self {
        Self {
            header: TypeMemberHeader {
                owner: TypeMemberOwner::Declared(declaration(owner)),
                key: TypeMemberKeyRecipe::Fixed(key),
                visibility: TypeMemberVisibility::Public,
                optional: false,
                readonly: false,
            },
            value: Some(value),
            key: None,
        }
    }
    fn indexer(owner: u32, key: Type, value: Type) -> Self {
        Self {
            header: TypeMemberHeader {
                owner: TypeMemberOwner::Declared(declaration(owner)),
                key: TypeMemberKeyRecipe::Indexer { sparse: false },
                visibility: TypeMemberVisibility::Public,
                optional: false,
                readonly: false,
            },
            value: Some(value),
            key: Some(key),
        }
    }
}
#[derive(Default)]
struct Provider {
    declarations: Vec<Declaration>,
    members: Vec<Member>,
    parameters: usize,
    constraints: Vec<Option<Type>>,
    defaults: Vec<Option<Type>>,
    function: bool,
    demands: RefCell<Vec<u32>>,
    errors: RefCell<Vec<NamedTypeError>>,
    constraint_cycles: RefCell<Vec<NativeTypeParameterKey>>,
}
impl Provider {
    fn declaration(&self, key: NativeTypeDeclarationKey) -> &Declaration {
        self.declarations
            .iter()
            .find(|decl| decl.key == key)
            .unwrap()
    }
}
impl NativeTypeProvider for Provider {
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        self.declarations.iter().any(|decl| decl.key == key)
    }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        self.declaration(key).header.clone()
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        key.snapshot == 777 && (key.index as usize) < self.members.len()
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        self.members[key.index as usize].header.clone()
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        self.declaration(key)
            .members
            .iter()
            .map(|&index| member(index))
            .collect()
    }
    fn type_alias_target(&self, key: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        self.declaration(key).target.clone().unwrap()
    }
    fn type_member_type(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        self.demands.borrow_mut().push(key.index);
        self.members[key.index as usize]
            .value
            .clone()
            .expect("unused member was demanded")
    }
    fn type_indexer_key(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        self.members[key.index as usize].key.clone().unwrap()
    }
    fn type_declaration_bases(
        &self,
        key: NativeTypeDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        self.declaration(key).bases.clone()
    }
    fn type_declaration_error(&self, error: NamedTypeError) {
        self.errors.borrow_mut().push(error)
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == 777 && (key.index as usize) < self.parameters
    }
    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
        format!("T{}", key.index)
    }
    fn type_parameter_owner(&self, key: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(if self.function && key.index == 1 {
            TypeDeclarationOwner::Function(function())
        } else {
            TypeDeclarationOwner::NamedType(declaration(0))
        })
    }
    fn type_parameter_siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        vec![key]
    }
    fn circular_type_parameter_constraint(&self, key: NativeTypeParameterKey) {
        self.constraint_cycles.borrow_mut().push(key);
    }
    fn has_type_parameter_constraint(&self, key: NativeTypeParameterKey) -> bool {
        self.constraints
            .get(key.index as usize)
            .is_some_and(Option::is_some)
    }
    fn type_parameter_constraint(
        &self,
        key: NativeTypeParameterKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.constraints.get(key.index as usize).cloned().flatten()
    }
    fn has_type_parameter_default(&self, key: NativeTypeParameterKey) -> bool {
        self.defaults
            .get(key.index as usize)
            .is_some_and(Option::is_some)
    }
    fn type_parameter_default(
        &self,
        key: NativeTypeParameterKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.defaults.get(key.index as usize).cloned().flatten()
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        self.function && key == function()
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        vec![Parameter {
            name: "value".into(),
            ty: reference(0, vec![Type::NativeTypeParameter(parameter(1))]),
            optional: false,
        }]
    }
    fn function_type_parameters(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<NativeTypeParameterKey> {
        vec![parameter(1)]
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        Some(Type::NativeTypeParameter(parameter(1)))
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("declared return must remain lazy")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no function cycle")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no function depth refusal")
    }
}

#[test]
fn primitive_and_union_aliases_remain_transparent_and_forward() {
    Engine::new().with_native_provider(
        || Provider {
            declarations: vec![
                Declaration::alias(0, reference(2, vec![])),
                Declaration::alias(1, Type::Union(vec![reference(0, vec![]), Type::Nil])),
                Declaration::alias(2, Type::Number),
            ],
            ..Provider::default()
        },
        |provider, q| {
            assert_eq!(
                q.named_type_reference(declaration(0), &[]).unwrap(),
                Type::Number
            );
            let union = q.named_type_reference(declaration(1), &[]).unwrap();
            assert!(q.is_assignable(&Type::Number, &union));
            assert!(q.is_assignable(&Type::Nil, &union));
            assert!(!q.is_assignable(&Type::String, &union));
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn unproductive_alias_cycles_report_each_original_alias_and_do_not_poison_other_heads() {
    Engine::new().with_native_provider(
        || Provider {
            declarations: vec![
                Declaration::alias(0, reference(1, vec![])),
                Declaration::alias(1, reference(0, vec![])),
                Declaration::alias(2, Type::String),
            ],
            ..Provider::default()
        },
        |provider, q| {
            assert_eq!(
                q.named_type_reference(declaration(0), &[]),
                Err(NamedTypeError::CircularAlias(declaration(0)))
            );
            assert!(
                provider
                    .errors
                    .borrow()
                    .contains(&NamedTypeError::CircularAlias(declaration(0)))
            );
            assert!(
                provider
                    .errors
                    .borrow()
                    .contains(&NamedTypeError::CircularAlias(declaration(1)))
            );
            let count = provider.errors.borrow().len();
            assert_eq!(
                q.named_type_reference(declaration(0), &[]),
                Err(NamedTypeError::CircularAlias(declaration(0)))
            );
            assert_eq!(
                provider.errors.borrow().len(),
                count,
                "completed cycle result should be cached"
            );
            assert_eq!(
                q.named_type_reference(declaration(2), &[]).unwrap(),
                Type::String
            );
        },
    );
}

#[test]
fn recursive_records_publish_heads_and_preserve_numeric_boolean_string_keys() {
    Engine::new().with_native_provider(
        || Provider {
            declarations: vec![
                Declaration::alias(0, reference(1, vec![])),
                Declaration::record(1, vec![0, 1, 2]),
                Declaration::record(2, vec![3]),
            ],
            members: vec![
                Member::field(
                    1,
                    LuaKey::String(b"next".to_vec()),
                    Type::Union(vec![reference(0, vec![]), Type::Nil]),
                ),
                Member::field(1, LuaKey::Integer(1), Type::Number),
                Member::field(1, LuaKey::Boolean(true), Type::String),
                Member::field(
                    2,
                    LuaKey::String(b"next".to_vec()),
                    Type::Union(vec![reference(2, vec![]), Type::Nil]),
                ),
            ],
            ..Provider::default()
        },
        |provider, q| {
            let node = q.named_type_reference(declaration(0), &[]).unwrap();
            assert!(matches!(node, Type::SessionType(_)));
            assert!(q.property_type(&node, "next").is_some());
            assert_eq!(
                q.table_lookup(&node, &TableKey::Literal(LuaKey::Integer(1)))
                    .unwrap(),
                Type::Number
            );
            assert_eq!(
                q.table_lookup(&node, &TableKey::Literal(LuaKey::Boolean(true)))
                    .unwrap(),
                Type::String
            );
            assert!(
                q.table_lookup(&node, &TableKey::Literal(LuaKey::String(b"1".to_vec())))
                    .is_err()
            );
            let other = q.named_type_reference(declaration(2), &[]).unwrap();
            assert!(
                q.is_assignable(&node, &other),
                "recursive relation must terminate through common relation cache"
            );
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn reading_and_printing_one_member_does_not_demand_an_unused_sibling() {
    Engine::new().with_native_provider(
        || {
            let mut unused = Member::field(0, LuaKey::String(b"unused".to_vec()), Type::Never);
            unused.value = None;
            Provider {
                declarations: vec![Declaration::record(0, vec![0, 1])],
                members: vec![
                    Member::field(0, LuaKey::String(b"value".to_vec()), Type::Number),
                    unused,
                ],
                ..Provider::default()
            }
        },
        |provider, q| {
            let record = q.named_type_reference(declaration(0), &[]).unwrap();
            assert!(provider.demands.borrow().is_empty());
            assert_eq!(q.format(&record), "Record0");
            assert_eq!(q.property_type(&record, "value"), Some(Type::Number));
            assert_eq!(*provider.demands.borrow(), vec![0]);
            assert_eq!(q.property_type(&record, "value"), Some(Type::Number));
            assert_eq!(
                *provider.demands.borrow(),
                vec![0],
                "raw member demand is cached"
            );
        },
    );
}

#[test]
fn native_generic_members_use_existing_mapper_once_and_constraints_defaults() {
    Engine::new().with_native_provider(
        || {
            let mut decl = Declaration::record(0, vec![0, 1]);
            decl.header.parameters = vec![parameter(0), parameter(1)];
            Provider {
                declarations: vec![decl],
                parameters: 2,
                members: vec![
                    Member::field(
                        0,
                        LuaKey::String(b"value".to_vec()),
                        Type::NativeTypeParameter(parameter(0)),
                    ),
                    Member::indexer(0, Type::String, Type::NativeTypeParameter(parameter(1))),
                ],
                defaults: vec![None, Some(Type::Boolean)],
                ..Provider::default()
            }
        },
        |provider, q| {
            let simultaneous = q
                .named_type_reference(
                    declaration(0),
                    &[Type::NativeTypeParameter(parameter(1)), Type::Number],
                )
                .unwrap();
            assert_eq!(
                q.property_type(&simultaneous, "value"),
                Some(Type::NativeTypeParameter(parameter(1)))
            );
            assert_eq!(
                q.table_lookup(
                    &simultaneous,
                    &TableKey::Literal(LuaKey::String(b"other".to_vec()))
                )
                .unwrap(),
                Type::Number
            );
            let defaulted = q
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert_eq!(
                q.table_lookup(
                    &defaulted,
                    &TableKey::Literal(LuaKey::String(b"other".to_vec()))
                )
                .unwrap(),
                Type::Boolean
            );
            assert!(matches!(
                q.named_type_reference(declaration(0), &[]),
                Err(NamedTypeError::TypeArgumentArity {
                    minimum: 1,
                    maximum: 2,
                    ..
                })
            ));
            assert_eq!(
                provider
                    .demands
                    .borrow()
                    .iter()
                    .filter(|&&id| id == 0)
                    .count(),
                1,
                "declaration-space member type must not be reread for each mapper"
            );
        },
    );
    Engine::new().with_native_provider(
        || {
            let mut decl = Declaration::alias(0, Type::NativeTypeParameter(parameter(0)));
            decl.header.parameters = vec![parameter(0)];
            Provider {
                declarations: vec![decl],
                parameters: 1,
                constraints: vec![Some(Type::Number)],
                ..Provider::default()
            }
        },
        |_, q| {
            assert_eq!(
                q.named_type_reference(declaration(0), &[Type::Number])
                    .unwrap(),
                Type::Number
            );
            assert!(matches!(
                q.named_type_reference(declaration(0), &[Type::String]),
                Err(NamedTypeError::TypeArgumentConstraint {
                    index: 0,
                    actual: Type::String,
                    expected: Type::Number,
                    ..
                })
            ));
        },
    );
}

#[test]
fn recursive_array_aliases_are_productive_and_declared_reads_follow_ts_default() {
    Engine::new().with_native_provider(
        || {
            let mut array = Declaration::record(1, vec![0]);
            array.header.object_form = Some(ObjectDeclarationForm::Array);
            Provider {
                declarations: vec![Declaration::alias(0, reference(1, vec![])), array],
                members: vec![Member::indexer(1, Type::Number, reference(0, vec![]))],
                ..Provider::default()
            }
        },
        |provider, q| {
            let array = q.named_type_reference(declaration(0), &[]).unwrap();
            let element = q
                .table_lookup(&array, &TableKey::Literal(LuaKey::Integer(1)))
                .unwrap();
            assert_eq!(
                element, array,
                "declared array index does not invent nil under the default policy"
            );
            let equivalent = q.array_type(&array);
            assert!(q.is_assignable(&array, &equivalent));
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn native_generic_function_inference_traverses_named_record_fields() {
    Engine::new().with_native_provider(
        || {
            let mut decl = Declaration::record(0, vec![0]);
            decl.header.parameters = vec![parameter(0)];
            Provider {
                declarations: vec![decl],
                parameters: 2,
                function: true,
                members: vec![Member::field(
                    0,
                    LuaKey::String(b"value".to_vec()),
                    Type::NativeTypeParameter(parameter(0)),
                )],
                ..Provider::default()
            }
        },
        |provider, q| {
            for actual in [Type::Number, Type::String] {
                let result = q.call(
                    &Type::NativeFunction(function()),
                    &[table(vec![field("value", actual.clone())])],
                );
                assert!(result.errors.is_empty(), "{result:?}");
                assert_eq!(result.return_type, actual);
            }
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn malformed_table_arity_fails_before_child_demands_or_head_publication() {
    Engine::new().with_native_provider(
        || {
            let mut decl = Declaration::record(0, vec![]);
            decl.header.object_form = Some(ObjectDeclarationForm::Table {
                type_argument_count: 1,
            });
            Provider {
                declarations: vec![decl],
                ..Provider::default()
            }
        },
        |provider, q| {
            assert!(matches!(
                q.named_type_reference(declaration(0), &[]),
                Err(NamedTypeError::TypeArgumentArity {
                    actual: 1,
                    minimum: 2,
                    maximum: 2,
                    ..
                })
            ));
            assert!(provider.demands.borrow().is_empty());
            assert_eq!(provider.errors.borrow().len(), 1);
        },
    );
}

#[test]
fn reused_indices_in_distinct_snapshots_never_share_heads() {
    Engine::new().with_native_provider(
        || {
            let first = Declaration::alias(0, Type::Number);
            let mut second = Declaration::alias(0, Type::String);
            second.key.snapshot += 1;
            Provider {
                declarations: vec![first, second],
                ..Provider::default()
            }
        },
        |_, q| {
            assert_eq!(
                q.named_type_reference(declaration(0), &[]).unwrap(),
                Type::Number
            );
            assert_eq!(
                q.named_type_reference(
                    NativeTypeDeclarationKey {
                        snapshot: 778,
                        index: 0
                    },
                    &[]
                )
                .unwrap(),
                Type::String
            );
            assert!(matches!(
                q.named_type_reference(
                    NativeTypeDeclarationKey {
                        snapshot: 779,
                        index: 0
                    },
                    &[]
                ),
                Err(NamedTypeError::InvalidDeclaration(_))
            ));
        },
    );
}

#[test]
fn recursive_named_generic_function_inference_uses_common_invoke_once_guard() {
    Engine::new().with_native_provider(
        || {
            let mut target = Declaration::record(0, vec![0, 1]);
            target.header.parameters = vec![parameter(0)];
            Provider {
                declarations: vec![target, Declaration::record(2, vec![2, 3])],
                parameters: 2,
                function: true,
                members: vec![
                    Member::field(
                        0,
                        LuaKey::String(b"value".to_vec()),
                        Type::NativeTypeParameter(parameter(0)),
                    ),
                    Member::field(
                        0,
                        LuaKey::String(b"next".to_vec()),
                        reference(0, vec![Type::NativeTypeParameter(parameter(0))]),
                    ),
                    Member::field(2, LuaKey::String(b"value".to_vec()), Type::Number),
                    Member::field(2, LuaKey::String(b"next".to_vec()), reference(2, vec![])),
                ],
                ..Provider::default()
            }
        },
        |provider, q| {
            let source = q.named_type_reference(declaration(2), &[]).unwrap();
            let result = q.call(&Type::NativeFunction(function()), &[source]);
            assert!(result.errors.is_empty(), "{result:?}");
            assert_eq!(result.return_type, Type::Number);
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn anonymous_alias_owner_augmentations_attach_after_canonical_head_resolution() {
    Engine::new().with_native_provider(
        || {
            let mut extension = Member::field(0, LuaKey::String(b"extra".to_vec()), Type::Number);
            extension.header.owner = TypeMemberOwner::Reference {
                name: "AliasOuter".into(),
                candidates: vec![declaration(3)],
            };
            Provider {
                declarations: vec![
                    Declaration::record(0, vec![0]),
                    Declaration::record(1, vec![0]),
                    Declaration::alias(2, reference(0, vec![])),
                    Declaration::alias(3, reference(2, vec![])),
                ],
                members: vec![extension],
                ..Provider::default()
            }
        },
        |provider, q| {
            let owner = q.named_type_reference(declaration(3), &[]).unwrap();
            let unrelated = q.named_type_reference(declaration(1), &[]).unwrap();
            assert_eq!(q.property_type(&owner, "extra"), Some(Type::Number));
            assert!(q.property_type(&unrelated, "extra").is_none());
            assert!(q.validate_named_declaration(declaration(1)).is_ok());
            assert_eq!(*provider.demands.borrow(), vec![0]);
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn nominal_base_visibility_and_duplicate_definitions_remain_explicit_errors() {
    Engine::new().with_native_provider(
        || {
            let mut class = Declaration::record(0, vec![]);
            class.header.kind = TypeDeclarationKind::Class;
            let mut module = Declaration::record(1, vec![]);
            module.header.kind = TypeDeclarationKind::Module;
            let mut derived = Declaration::record(2, vec![]);
            derived.bases = vec![reference(3, vec![])];
            let mut private = Member::field(4, LuaKey::String(b"secret".to_vec()), Type::Number);
            private.header.visibility = TypeMemberVisibility::Private;
            Provider {
                declarations: vec![
                    class,
                    module,
                    derived,
                    Declaration::record(3, vec![]),
                    Declaration::record(4, vec![0]),
                    Declaration::record(5, vec![1, 2]),
                ],
                members: vec![
                    private,
                    Member::field(5, LuaKey::Integer(1), Type::Number),
                    Member::field(5, LuaKey::Number(1.0), Type::Number),
                ],
                ..Provider::default()
            }
        },
        |_, q| {
            assert!(q.named_type_reference(declaration(0), &[]).is_ok());
            assert_eq!(
                q.named_type_reference(declaration(1), &[]),
                Err(NamedTypeError::UnsupportedKind(
                    declaration(1),
                    TypeDeclarationKind::Module
                ))
            );
            assert_eq!(
                q.validate_named_declaration(declaration(2)),
                Err(NamedTypeError::UnsupportedBases(declaration(2)))
            );
            assert_eq!(
                q.validate_named_declaration(declaration(4)),
                Err(NamedTypeError::UnsupportedVisibility(member(0)))
            );
            assert_eq!(
                q.validate_named_declaration(declaration(5)),
                Err(NamedTypeError::Table {
                    declaration: declaration(5),
                    error: TableError::DuplicateDeclaredField {
                        key: LuaKey::Integer(1)
                    }
                })
            );
        },
    );
}

#[test]
fn unknown_ambiguous_and_primitive_augmentation_owners_are_never_empty_shapes() {
    for candidates in [
        vec![],
        vec![declaration(1), declaration(2)],
        vec![declaration(1)],
    ] {
        Engine::new().with_native_provider(
            move || {
                let mut extension =
                    Member::field(0, LuaKey::String(b"value".to_vec()), Type::Number);
                extension.header.owner = TypeMemberOwner::Reference {
                    name: "Owner".into(),
                    candidates,
                };
                Provider {
                    declarations: vec![
                        Declaration::record(0, vec![0]),
                        Declaration::alias(1, Type::Number),
                        Declaration::record(2, vec![]),
                    ],
                    members: vec![extension],
                    ..Provider::default()
                }
            },
            |provider, q| {
                assert!(matches!(
                    q.validate_named_declaration(declaration(0)),
                    Err(NamedTypeError::UnknownOwner {
                        member: NativeTypeMemberKey { index: 0, .. },
                        ..
                    } | NamedTypeError::AmbiguousOwner {
                        member: NativeTypeMemberKey { index: 0, .. },
                        ..
                    })
                ));
                assert!(
                    provider.demands.borrow().is_empty(),
                    "owner must be validated before its member type"
                );
                assert_eq!(provider.errors.borrow().len(), 1);
            },
        );
    }
}

#[test]
fn optional_numeric_only_records_retain_weak_object_and_common_key_semantics() {
    Engine::new().with_native_provider(
        || {
            let mut optional = Member::field(0, LuaKey::Integer(1), Type::Number);
            optional.header.optional = true;
            Provider {
                declarations: vec![Declaration::record(0, vec![0])],
                members: vec![optional],
                ..Provider::default()
            }
        },
        |_, q| {
            let target = q.named_type_reference(declaration(0), &[]).unwrap();
            assert!(q.is_assignable(
                &table(vec![TableField::new(LuaKey::Integer(1), Type::Number)]),
                &target
            ));
            assert!(!q.is_assignable(&table(vec![field("other", Type::Number)]), &target));
            assert!(!q.is_assignable(
                &table(vec![TableField::new(LuaKey::Integer(1), Type::String)]),
                &target
            ));
            assert!(q.is_assignable(&table(vec![]), &target));
        },
    );
}

#[test]
fn recursive_generic_node_fields_keep_their_actual_argument_after_head_publication() {
    Engine::new().with_native_provider(
        || {
            let mut node = Declaration::record(0, vec![0, 1]);
            node.header.parameters = vec![parameter(0)];
            Provider {
                declarations: vec![node],
                parameters: 1,
                members: vec![
                    Member::field(
                        0,
                        LuaKey::String(b"next".to_vec()),
                        Type::Union(vec![
                            reference(0, vec![Type::NativeTypeParameter(parameter(0))]),
                            Type::Nil,
                        ]),
                    ),
                    Member::field(
                        0,
                        LuaKey::String(b"value".to_vec()),
                        Type::NativeTypeParameter(parameter(0)),
                    ),
                ],
                ..Provider::default()
            }
        },
        |provider, q| {
            assert!(q.validate_named_declaration(declaration(0)).is_ok());
            for value in [Type::Number, Type::String] {
                let node = q
                    .named_type_reference(declaration(0), &[value.clone()])
                    .unwrap();
                assert_eq!(q.property_type(&node, "value"), Some(value));
                let expected = Type::Union(vec![node.clone(), Type::Nil]);
                let next = q.property_type(&node, "next").unwrap();
                assert!(q.is_assignable(&next, &expected));
                assert!(q.is_assignable(&expected, &next));
            }
            assert!(provider.errors.borrow().is_empty());
            assert!(provider.constraint_cycles.borrow().is_empty());
        },
    );
}

#[test]
fn f_bounded_parameters_do_not_become_spurious_constraint_cycles() {
    Engine::new().with_native_provider(
        || {
            let mut node = Declaration::record(0, vec![0]);
            node.header.parameters = vec![parameter(0)];
            Provider {
                declarations: vec![node],
                parameters: 1,
                constraints: vec![Some(reference(
                    0,
                    vec![Type::NativeTypeParameter(parameter(0))],
                ))],
                members: vec![Member::field(
                    0,
                    LuaKey::String(b"next".to_vec()),
                    Type::NativeTypeParameter(parameter(0)),
                )],
                ..Provider::default()
            }
        },
        |provider, q| {
            let node = q.named_declared_type(declaration(0)).unwrap();
            assert_eq!(
                q.type_parameter_constraint(parameter(0)),
                Some(node.clone())
            );
            assert!(q.validate_named_declaration(declaration(0)).is_ok());
            assert_eq!(q.type_parameter_constraint(parameter(0)), Some(node));
            assert!(
                provider.constraint_cycles.borrow().is_empty(),
                "an F-bound is productive through a structural head"
            );
            assert!(provider.errors.borrow().is_empty());
            assert!(matches!(
                q.named_type_reference(declaration(0), &[Type::Number]),
                Err(NamedTypeError::TypeArgumentConstraint { .. })
            ));
        },
    );
}

#[test]
fn specialized_alias_augmentation_owner_mapping_remains_an_explicit_gate() {
    Engine::new().with_native_provider(
        || {
            let mut shape = Declaration::record(0, vec![0]);
            shape.header.parameters = vec![parameter(0)];
            let mut alias = Declaration::alias(
                1,
                reference(0, vec![Type::NativeTypeParameter(parameter(1))]),
            );
            alias.header.parameters = vec![parameter(1)];
            let mut extension = Member::field(
                0,
                LuaKey::String(b"extra".to_vec()),
                Type::NativeTypeParameter(parameter(1)),
            );
            extension.header.owner = TypeMemberOwner::Reference {
                name: "Alias".into(),
                candidates: vec![declaration(1)],
            };
            Provider {
                declarations: vec![shape, alias],
                parameters: 2,
                members: vec![extension],
                ..Provider::default()
            }
        },
        |provider, q| {
            assert_eq!(
                q.validate_named_declaration(declaration(0)),
                Err(NamedTypeError::UnsupportedOwnerInstantiation(member(0)))
            );
            assert!(
                provider.demands.borrow().is_empty(),
                "unproven owner transport cannot publish an unconstrained generic member"
            );
        },
    );
}
