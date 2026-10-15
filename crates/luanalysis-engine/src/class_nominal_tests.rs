//! Native class origins are checked by the ordinary recursive relation worker. The source
//! frontend has no top-level branding filter, and structural recipes cannot construct origins.
use super::*;
use std::cell::RefCell;

#[path = "class_definition_tests.rs"]
mod definitions;

fn key(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: 91001,
        index,
    }
}
fn parameter() -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: 91001,
        index: 0,
    }
}
fn reference(index: u32, arguments: Vec<Type>) -> Type {
    Type::NativeNamedReference {
        declaration: key(index),
        arguments,
    }
}
fn table(value: Type) -> Type {
    Type::Table(TableType {
        fields: vec![TableField::new(LuaKey::String(b"value".to_vec()), value)],
        ..TableType::default()
    })
}
fn empty() -> Type {
    Type::Table(TableType::default())
}
fn function(params: Vec<Type>, returned: Type) -> Type {
    Type::Function(FunctionType {
        params: params
            .into_iter()
            .enumerate()
            .map(|(i, ty)| Parameter {
                name: format!("p{i}"),
                ty,
                optional: false,
            })
            .collect(),
        returns: Box::new(returned),
    })
}
fn pack(types: Vec<Type>) -> ValuePack {
    ValuePack::Alternatives(vec![PackAlternative {
        fixed: types.into_iter().map(PackSlot::required).collect(),
        tail: None,
    }])
}

struct Decl {
    key: NativeTypeDeclarationKey,
    kind: TypeDeclarationKind,
    fields: Vec<Type>,
    generic: bool,
    target: Option<Type>,
    bases: Vec<Type>,
    optional: bool,
}
impl Decl {
    fn class(index: u32, fields: Vec<Type>) -> Self {
        Self {
            key: key(index),
            kind: TypeDeclarationKind::Class,
            fields,
            generic: false,
            target: None,
            bases: vec![],
            optional: false,
        }
    }
    fn shape(index: u32, fields: Vec<Type>) -> Self {
        Self {
            kind: TypeDeclarationKind::Shape,
            ..Self::class(index, fields)
        }
    }
    fn alias(index: u32, target: Type) -> Self {
        Self {
            kind: TypeDeclarationKind::Alias,
            target: Some(target),
            ..Self::class(index, vec![])
        }
    }
}
#[derive(Default)]
struct Provider {
    declarations: Vec<Decl>,
    errors: RefCell<Vec<NamedTypeError>>,
    constraint: Option<Type>,
}
impl Provider {
    fn decl(&self, key: NativeTypeDeclarationKey) -> &Decl {
        self.declarations
            .iter()
            .find(|decl| decl.key == key)
            .unwrap()
    }
    fn field(&self, key: NativeTypeMemberKey) -> (&Decl, usize) {
        let owner = key.index / 100;
        let field = (key.index % 100) as usize;
        let declaration = NativeTypeDeclarationKey {
            snapshot: key.snapshot,
            index: owner,
        };
        (self.decl(declaration), field)
    }
}
impl NativeTypeProvider for Provider {
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        self.declarations.iter().any(|decl| decl.key == key)
    }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        let declaration = self.decl(key);
        TypeDeclarationHeader {
            kind: declaration.kind,
            name: format!("Class{}", key.index),
            object_form: if declaration.kind == TypeDeclarationKind::Alias {
                None
            } else {
                Some(ObjectDeclarationForm::Record)
            },
            parameters: if declaration.generic {
                vec![parameter()]
            } else {
                vec![]
            },
            outer_parameters: vec![],
        }
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        self.declarations.iter().any(|decl| {
            decl.key.snapshot == key.snapshot
                && decl.key.index == key.index / 100
                && (key.index % 100) < decl.fields.len() as u32
        })
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        let (declaration, index) = self.field(key);
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(declaration.key),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(if index == 0 {
                b"value".to_vec()
            } else {
                format!("field{index}").into_bytes()
            })),
            visibility: TypeMemberVisibility::Public,
            optional: declaration.optional,
            readonly: false,
        }
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        (0..self.decl(key).fields.len())
            .map(|index| NativeTypeMemberKey {
                snapshot: key.snapshot,
                index: key.index * 100 + index as u32,
            })
            .collect()
    }
    fn type_member_type(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        let (declaration, index) = self.field(key);
        declaration.fields[index].clone()
    }
    fn type_indexer_key(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        panic!("no indexers")
    }
    fn type_alias_target(&self, key: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        self.decl(key).target.clone().unwrap()
    }
    fn type_declaration_bases(
        &self,
        key: NativeTypeDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        self.decl(key).bases.clone()
    }
    fn type_declaration_error(&self, error: NamedTypeError) {
        self.errors.borrow_mut().push(error)
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key == parameter()
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "T".into()
    }
    fn type_parameter_owner(&self, _: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(TypeDeclarationOwner::NamedType(key(0)))
    }
    fn has_type_parameter_constraint(&self, _: NativeTypeParameterKey) -> bool {
        self.constraint.is_some()
    }
    fn type_parameter_constraint(
        &self,
        _: NativeTypeParameterKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.constraint.clone()
    }
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("no runtime methods")
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("no runtime methods")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("no runtime methods")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no runtime methods")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no runtime methods")
    }
}

fn basic() -> Provider {
    Provider {
        declarations: vec![
            Decl::class(0, vec![]),
            Decl::class(1, vec![]),
            Decl::class(2, vec![Type::Number]),
            Decl::class(3, vec![Type::Number]),
            Decl::shape(4, vec![Type::Number]),
            Decl::alias(5, reference(2, vec![])),
        ],
        ..Provider::default()
    }
}

#[test]
fn unrelated_empty_and_equal_shaped_classes_require_original_origin() {
    Engine::new().with_native_provider(basic, |provider, q| {
        for index in [0, 2] {
            let own = q.named_type_reference(key(index), &[]).unwrap();
            let other = q.named_type_reference(key(index + 1), &[]).unwrap();
            assert!(q.is_assignable(&own, &own));
            assert!(!q.is_assignable(&own, &other));
            assert!(!q.is_assignable(&other, &own));
            assert!(!q.is_assignable(&empty(), &own));
            assert!(!q.is_assignable(&table(Type::Number), &own));
            assert!(matches!(q.inspect_type(&own), Type::SessionType(_)));
        }
        assert!(provider.errors.borrow().is_empty());
    });
}

#[test]
fn compatible_class_sources_satisfy_shapes_but_structures_do_not_become_classes() {
    Engine::new().with_native_provider(basic, |_, q| {
        let class = q.named_type_reference(key(2), &[]).unwrap();
        let shape = q.named_type_reference(key(4), &[]).unwrap();
        assert!(q.is_assignable(&class, &shape));
        assert!(q.is_assignable(&class, &table(Type::Number)));
        assert!(q.is_assignable(&class, &empty()));
        assert!(!q.is_assignable(&shape, &class));
        assert!(!q.is_assignable(&table(Type::Number), &class));
    });
}

#[test]
fn aliases_and_reused_indices_across_snapshots_preserve_nominal_declaration_identity() {
    Engine::new().with_native_provider(
        || {
            let mut p = basic();
            let mut foreign = Decl::class(2, vec![Type::Number]);
            foreign.key.snapshot += 1;
            p.declarations.push(foreign);
            p
        },
        |_, q| {
            let class = q.named_type_reference(key(2), &[]).unwrap();
            let alias = q.named_type_reference(key(5), &[]).unwrap();
            let foreign = q
                .named_type_reference(
                    NativeTypeDeclarationKey {
                        snapshot: 91002,
                        index: 2,
                    },
                    &[],
                )
                .unwrap();
            assert_eq!(class, alias);
            assert!(q.is_assignable(&alias, &class));
            assert!(!q.is_assignable(&foreign, &class));
            assert!(!q.is_assignable(&class, &foreign));
        },
    );
}

#[test]
fn accepting_origin_still_checks_instantiated_primitive_member_types() {
    Engine::new().with_native_provider(
        || {
            let mut class = Decl::class(0, vec![Type::NativeTypeParameter(parameter())]);
            class.generic = true;
            Provider {
                declarations: vec![class],
                ..Provider::default()
            }
        },
        |_, q| {
            let number = q.named_type_reference(key(0), &[Type::Number]).unwrap();
            let string = q.named_type_reference(key(0), &[Type::String]).unwrap();
            assert_eq!(q.property_type(&number, "value"), Some(Type::Number));
            assert_eq!(q.property_type(&string, "value"), Some(Type::String));
            assert!(
                !q.is_assignable(&number, &string),
                "same nominal origin does not replace member checking"
            );
            assert!(!q.is_assignable(&string, &number));
            assert!(
                q.table_write(
                    &number,
                    &TableKey::Literal(LuaKey::String(b"value".to_vec())),
                    &Type::Number
                )
                .is_ok()
            );
            assert!(
                q.table_write(
                    &number,
                    &TableKey::Literal(LuaKey::String(b"value".to_vec())),
                    &Type::String
                )
                .is_err()
            );
        },
    );
}

#[test]
fn class_origins_are_checked_inside_record_array_callback_union_and_pack_relations() {
    Engine::new().with_native_provider(basic, |_, q| {
        let a = q.named_type_reference(key(0), &[]).unwrap();
        let b = q.named_type_reference(key(1), &[]).unwrap();
        assert!(!q.is_assignable(&table(a.clone()), &table(b.clone())));
        let aa = q.array_type(&a);
        let bb = q.array_type(&b);
        assert!(!q.is_assignable(&aa, &bb));
        assert!(!q.is_assignable(
            &function(vec![a.clone()], a.clone()),
            &function(vec![b.clone()], b.clone())
        ));
        assert!(!q.is_assignable(&function(vec![], a.clone()), &function(vec![], b.clone())));
        assert!(!q.is_assignable(
            &function(vec![a.clone()], Type::Void),
            &function(vec![b.clone()], Type::Void)
        ));
        let au = Type::Union(vec![a.clone(), Type::Nil]);
        let bu = Type::Union(vec![b.clone(), Type::Nil]);
        assert!(!q.is_assignable(&au, &bu));
        assert!(q.is_assignable(&a, &au));
        assert!(!q.is_assignable(&b, &au));
        assert!(!q.pack_assignable(
            &pack(vec![Type::Number, a.clone()]),
            &pack(vec![Type::Number, b.clone()])
        ));
        let af = q.pack_function(&PackFunctionType {
            params: vec![],
            rest: None,
            type_parameters: vec![],
            returns: pack(vec![Type::Number, a.clone()]),
        });
        let bf = q.pack_function(&PackFunctionType {
            params: vec![],
            rest: None,
            type_parameters: vec![],
            returns: pack(vec![Type::Number, b.clone()]),
        });
        assert!(!q.is_assignable(&af, &bf));
        assert!(q.is_assignable(&af, &af));
    });
}

#[test]
fn unions_of_distinct_empty_origins_do_not_collapse_by_empty_object_classification() {
    Engine::new().with_native_provider(basic, |_, q| {
        let a = q.named_type_reference(key(0), &[]).unwrap();
        let b = q.named_type_reference(key(1), &[]).unwrap();
        let joined = q.union_reduced(&[a.clone(), b.clone()]);
        assert!(q.is_assignable(&a, &joined));
        assert!(q.is_assignable(&b, &joined));
        assert!(!q.is_assignable(&joined, &a));
        assert!(!q.is_assignable(&joined, &b));
        assert!(!q.is_assignable(&empty(), &joined));
        assert!(matches!(q.inspect_type(&joined),Type::Union(parts) if parts.len()==2));
    });
}

#[test]
fn optional_and_weak_class_members_never_accept_a_structural_origin() {
    Engine::new().with_native_provider(
        || {
            let mut class = Decl::class(0, vec![Type::Number]);
            class.optional = true;
            Provider {
                declarations: vec![class],
                ..Provider::default()
            }
        },
        |_, q| {
            let target = q.named_type_reference(key(0), &[]).unwrap();
            assert!(!q.is_assignable(&empty(), &target));
            assert!(!q.is_assignable(&table(Type::Number), &target));
            assert!(q.is_assignable(&target, &target));
        },
    );
}

#[test]
fn generic_phantom_arguments_keep_ts_default_structural_behavior_after_same_origin() {
    Engine::new().with_native_provider(
        || {
            let mut class = Decl::class(0, vec![]);
            class.generic = true;
            Provider {
                declarations: vec![class],
                ..Provider::default()
            }
        },
        |_, q| {
            let number = q.named_type_reference(key(0), &[Type::Number]).unwrap();
            let string = q.named_type_reference(key(0), &[Type::String]).unwrap();
            assert!(q.is_assignable(&number, &string));
            assert!(q.is_assignable(&string, &number));
            assert!(!q.is_assignable(&empty(), &number));
        },
    );
}

#[test]
fn base_inheritance_remains_explicitly_gated() {
    Engine::new().with_native_provider(
        || {
            let mut p = basic();
            p.declarations[1].bases = vec![reference(0, vec![])];
            p
        },
        |_, q| {
            assert!(q.named_declared_type(key(1)).is_ok());
            assert_eq!(
                q.validate_named_declaration(key(1)),
                Err(NamedTypeError::UnsupportedBases(key(1)))
            );
        },
    );
}

#[test]
fn any_never_constraints_and_composite_types_keep_default_recursive_workers() {
    Engine::new().with_native_provider(
        || {
            let mut p = basic();
            p.constraint = Some(reference(0, vec![]));
            p
        },
        |_, q| {
            let a = q.named_type_reference(key(0), &[]).unwrap();
            let b = q.named_type_reference(key(1), &[]).unwrap();
            assert!(q.is_assignable(&Type::Any, &a));
            assert!(q.is_assignable(&Type::Never, &a));
            assert!(!q.is_assignable(&Type::Unknown, &a));
            assert!(!q.is_assignable(&Type::Nil, &a));
            assert!(q.is_assignable(&Type::NativeTypeParameter(parameter()), &a));
            assert!(!q.is_assignable(&Type::NativeTypeParameter(parameter()), &b));
            let intersection = Type::Intersection(vec![a.clone(), empty()]);
            assert!(q.is_assignable(&intersection, &a));
            assert!(!q.is_assignable(&intersection, &b));
            assert!(!q.is_assignable(&empty(), &intersection));
        },
    );
}

#[test]
fn materialized_class_descriptions_keep_origin_through_mapping_widening_and_opaque_export() {
    with_program(|program, handles| {
        let mut declaration = Decl::class(0, vec![Type::NativeTypeParameter(parameter())]);
        declaration.generic = true;
        let provider = Provider {
            declarations: vec![declaration, Decl::class(1, vec![Type::Number])],
            ..Provider::default()
        };
        let adapter = ProviderAdapter {
            provider: &provider,
            handles,
        };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        checker.enable_native_arrays();
        let generic = checker
            .native_declared_head(native_named::core_declaration(key(0)))
            .unwrap();
        let generic_description = checker.native_object_resolved(generic).unwrap();
        let TypeData::NativeTable(data) = checker.data(generic_description) else {
            panic!("class must use the native table member worker")
        };
        assert!(data.nominal_origin.is_some());
        let param = checker
            .native_type_parameter(core_parameter_key(parameter()))
            .unwrap();
        let number_mapper = checker.mapper_from(&[param], &[TypeId::NUMBER]);
        let string_mapper = checker.mapper_from(&[param], &[TypeId::STRING]);
        let number = checker.instantiate(generic_description, number_mapper);
        let string = checker.instantiate(generic_description, string_mapper);
        let other = checker
            .native_type_reference(native_named::core_declaration(key(1)), &[])
            .unwrap();
        let other_description = checker.native_object_resolved(other).unwrap();
        assert!(!checker.is_assignable(number, string));
        assert!(!checker.is_assignable(number, other_description));
        let owned = export_type(&mut checker, handles, number);
        assert!(matches!(owned, Type::SessionType(_)));
        assert_eq!(import_type(&mut checker, handles, &owned), number);
        assert!(String::from_utf8_lossy(&checker.type_to_string(number)).contains("Class0"));
        let mut q = SessionQueries {
            checker: &mut checker,
            handles,
        };
        let widened = q.widen(&owned);
        assert!(matches!(widened, Type::SessionType(_)));
        assert!(q.is_assignable(&widened, &owned));
        assert!(!q.is_assignable(&table(Type::Number), &widened));
        assert!(!q.is_assignable(&widened, &reference(1, vec![])));
        assert_eq!(q.property_type(&widened, "value"), Some(Type::Number));
    });
}

#[test]
fn runtime_shape_class_field_names_and_static_name_spellings_do_not_establish_origin() {
    Engine::new().with_native_provider(basic, |_, q| {
        let target = q.named_type_reference(key(0), &[]).unwrap();
        let raw = Type::Table(TableType {
            fields: vec![
                TableField::new(LuaKey::String(b"__class".to_vec()), target.clone()),
                TableField::new(LuaKey::String(b"__index".to_vec()), target.clone()),
                TableField::new(
                    LuaKey::String(b"ge_tts__static_Class0".to_vec()),
                    Type::Number,
                ),
            ],
            ..TableType::default()
        });
        assert!(!q.is_assignable(&raw, &target));
    });
}
