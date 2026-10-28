use super::*;
use crate::*;

fn key(name: &str) -> LuaKey { LuaKey::String(name.as_bytes().to_vec()) }
fn access(name: &str) -> TableKey { TableKey::Literal(key(name)) }
fn description(q: &mut dyn TypeQueries, table: &Type, key: &TableKey) -> Result<Type, TableError> {
    let value = q.table_lookup(table, key)?;
    Ok(q.inspect_type(&value))
}

#[test]
fn written_singletons_and_unions_survive_fresh_construction_storage_and_owned_roundtrips() {
    let engine = Engine::new();
    let owned = engine.with_session(|q| {
        let table = q.table_type(&TableType {
            fields: vec![
                TableField::declared(key("number"), Type::NumberLiteral(1.0)),
                TableField::declared(key("string"), Type::StringLiteral(b"written".to_vec())),
                TableField::declared(key("boolean"), Type::BooleanLiteral(false)),
                TableField::declared(key("union"), Type::Union(vec![Type::NumberLiteral(1.0), Type::NumberLiteral(2.0)])),
                TableField::new(key("inferred"), Type::NumberLiteral(1.0)),
            ],
            fresh: true,
            ..Default::default()
        }).unwrap();
        for value in [&table, &q.widen(&table), &q.inspect_type(&table)] {
            assert_eq!(description(q, value, &access("number")), Ok(Type::NumberLiteral(1.0)));
            assert_eq!(description(q, value, &access("string")), Ok(Type::StringLiteral(b"written".to_vec())));
            assert_eq!(description(q, value, &access("boolean")), Ok(Type::BooleanLiteral(false)));
            assert_eq!(description(q, value, &access("inferred")), Ok(Type::Number));
            assert_eq!(q.table_write(value, &access("number"), &Type::NumberLiteral(1.0)), Ok(()));
            assert!(matches!(q.table_write(value, &access("number"), &Type::NumberLiteral(2.0)), Err(TableError::ValueType { .. })));
            assert_eq!(q.table_write(value, &access("union"), &Type::NumberLiteral(2.0)), Ok(()));
            assert!(matches!(q.table_write(value, &access("union"), &Type::NumberLiteral(3.0)), Err(TableError::ValueType { .. })));
            let Type::Table(description) = q.inspect_type(value) else { panic!("ordinary table stays an owned descriptor") };
            assert!(description.fields.iter().filter(|f| f.key != key("inferred")).all(|f| f.origin == TableFieldOrigin::Declared && !f.readonly));
        }
        table
    });
    engine.with_session(|q| {
        let stored = q.widen(&owned);
        assert_eq!(description(q, &stored, &access("number")), Ok(Type::NumberLiteral(1.0)));
        assert_eq!(q.table_write(&stored, &access("boolean"), &Type::BooleanLiteral(false)), Ok(()));
        assert!(q.table_write(&stored, &access("boolean"), &Type::BooleanLiteral(true)).is_err());
    });
}

#[test]
fn written_dynamic_entries_and_open_tails_keep_contracts_without_changing_key_domains() {
    Engine::new().with_session(|q| {
        let table = q.table_type(&TableType {
            fields: vec![TableField::declared(LuaKey::Integer(1), Type::BooleanLiteral(false))],
            indexers: vec![TableIndexer::declared(Type::String, Type::NumberLiteral(7.0))],
            sequence_tails: vec![TableSequenceTail {
                start: 2,
                value: Type::StringLiteral(b"tail".to_vec()),
                readonly: false,
                origin: TableFieldOrigin::Declared,
                sparse: true,
            }],
            fresh: true,
            ..Default::default()
        }).unwrap();
        let stored = q.widen(&table);
        assert_eq!(description(q, &stored, &TableKey::Literal(LuaKey::Integer(1))), Ok(Type::BooleanLiteral(false)));
        assert_eq!(description(q, &stored, &TableKey::Literal(LuaKey::Integer(2))), Ok(Type::Union(vec![Type::Nil, Type::StringLiteral(b"tail".to_vec())])));
        assert!(q.table_lookup(&stored, &TableKey::Literal(LuaKey::Number(2.5))).is_err());
        let dynamic = q.table_lookup(&stored, &access("entry")).unwrap();
        assert!(q.is_assignable(&Type::Nil, &dynamic));
        assert!(q.is_assignable(&Type::NumberLiteral(7.0), &dynamic));
        assert!(!q.is_assignable(&Type::NumberLiteral(8.0), &dynamic));
        assert_eq!(q.table_write(&stored, &access("entry"), &Type::NumberLiteral(7.0)), Ok(()));
        assert!(q.table_write(&stored, &access("entry"), &Type::NumberLiteral(8.0)).is_err());
        assert_eq!(q.table_write(&stored, &TableKey::Literal(LuaKey::Integer(3)), &Type::StringLiteral(b"tail".to_vec())), Ok(()));
        assert!(q.table_write(&stored, &TableKey::Literal(LuaKey::Integer(3)), &Type::StringLiteral(b"other".to_vec())).is_err());
        let Type::Table(description) = q.inspect_type(&stored) else { panic!("owned table") };
        assert_eq!(description.indexers[0].origin, TableFieldOrigin::Declared);
        assert!(description.indexers[0].sparse);
        assert_eq!(description.sequence_tails[0].origin, TableFieldOrigin::Declared);
    });
}

#[test]
fn constructor_collision_candidates_and_expanded_result_rows_still_use_native_unions() {
    Engine::new().with_session(|q| {
        let table = q.table_type(&TableType {
            fields: vec![
                TableField::declared(key("value"), Type::NumberLiteral(1.0)),
                TableField::declared(key("value"), Type::NumberLiteral(2.0)),
                TableField::new(key("mixed"), Type::NumberLiteral(1.0)),
                TableField::declared(key("mixed"), Type::StringLiteral(b"fixed".to_vec())),
            ],
            fresh: true,
            ..Default::default()
        }).unwrap();
        let stored = q.widen(&table);
        assert_eq!(q.table_write(&stored, &access("value"), &Type::NumberLiteral(2.0)), Ok(()));
        assert!(q.table_write(&stored, &access("value"), &Type::NumberLiteral(3.0)).is_err());
        assert_eq!(q.table_write(&stored, &access("mixed"), &Type::Number), Ok(()));
        assert_eq!(q.table_write(&stored, &access("mixed"), &Type::StringLiteral(b"fixed".to_vec())), Ok(()));
        assert!(q.table_write(&stored, &access("mixed"), &Type::StringLiteral(b"other".to_vec())).is_err());
        let expanded = q.table_with_sequence(&TableType {
            fields: vec![TableField::declared(key("contract"), Type::NumberLiteral(1.0))],
            fresh: true,
            ..Default::default()
        }, 1, &ValuePack::Alternatives(vec![PackAlternative {
            fixed: vec![PackSlot { ty: Type::BooleanLiteral(false), optional: false }],
            tail: Some(Box::new(Type::String)),
        }]), None).unwrap();
        assert_eq!(description(q, &expanded, &access("contract")), Ok(Type::NumberLiteral(1.0)));
        assert_eq!(description(q, &expanded, &TableKey::Literal(LuaKey::Integer(1))), Ok(Type::Boolean));
        // An expansion's tail reads as its element, as a TypeScript array's does.
        assert_eq!(description(q, &expanded, &TableKey::Literal(LuaKey::Integer(2))), Ok(Type::String));
        let Type::Table(description) = q.inspect_type(&expanded) else { panic!("owned expanded table") };
        assert_eq!(description.sequence_tails[0].origin, TableFieldOrigin::Inferred);
    });
}

#[test]
fn annotated_final_sequence_transports_origin_through_whole_rows_and_open_tails() {
    Engine::new().with_session(|q| {
        let source = ValuePack::Alternatives(vec![
            PackAlternative { fixed: vec![], tail: None },
            PackAlternative {
                fixed: vec![PackSlot { ty: Type::NumberLiteral(1.0), optional: false }],
                tail: Some(Box::new(Type::StringLiteral(b"tail".to_vec()))),
            },
        ]);
        let base = TableType { fresh: true, ..Default::default() };
        let declared = q.table_with_sequence_origin(&base, 1, &source, TableFieldOrigin::Declared, None).unwrap();
        let stored = q.widen(&declared);
        let context = q.table_sequence_context(&stored, 1).unwrap();
        assert!(q.pack_assignable(&ValuePack::empty(), &context));
        assert!(q.pack_assignable(&ValuePack::single(Type::NumberLiteral(1.0)), &context));
        assert!(!q.pack_assignable(&ValuePack::single(Type::NumberLiteral(2.0)), &context));
        let row = |tail: &[u8]| ValuePack::Alternatives(vec![PackAlternative {
            fixed: vec![
                PackSlot { ty: Type::NumberLiteral(1.0), optional: false },
                PackSlot { ty: Type::StringLiteral(tail.to_vec()), optional: false },
            ],
            tail: None,
        }]);
        assert!(q.pack_assignable(&row(b"tail"), &context));
        assert!(!q.pack_assignable(&row(b"other"), &context));
        let Type::Union(parts) = q.inspect_type(&stored) else { panic!("complete result rows remain alternatives") };
        for part in parts {
            let Type::Table(description) = part else { panic!("table row") };
            assert!(description.fields.iter().all(|field| field.origin == TableFieldOrigin::Declared && !field.readonly));
            assert!(description.sequence_tails.iter().all(|tail| tail.origin == TableFieldOrigin::Declared && !tail.readonly));
        }
        let inferred = q.table_with_sequence(&base, 1, &source, None).unwrap();
        let inferred_context = q.table_sequence_context(&inferred, 1).unwrap();
        assert!(q.pack_assignable(&ValuePack::single(Type::NumberLiteral(2.0)), &inferred_context));
        assert!(q.pack_assignable(&row(b"other"), &inferred_context));
    });
}

struct Contracts;
fn declaration(index: u32) -> NativeTypeDeclarationKey { NativeTypeDeclarationKey { snapshot: 94001, index } }
fn parameter(index: u32) -> NativeTypeParameterKey { NativeTypeParameterKey { snapshot: 94001, index } }
fn member(index: u32) -> NativeTypeMemberKey { NativeTypeMemberKey { snapshot: 94001, index } }
impl NativeTypeProvider for Contracts {
    fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("no implementation") }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { panic!("no implementation") }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { panic!("no implementation") }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("no implementation") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("no implementation") }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool { key.snapshot == 94001 && key.index < 4 }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: match key.index { 0 => TypeDeclarationKind::Alias, 2 => TypeDeclarationKind::Class, _ => TypeDeclarationKind::Shape },
            name: format!("Contract{}", key.index),
            object_form: (key.index != 0).then_some(ObjectDeclarationForm::Record),
            parameters: vec![parameter(key.index)],
            outer_parameters: vec![],
        }
    }
    fn type_alias_target(&self, _: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        Type::Table(TableType { fields: vec![TableField::declared(key("value"), Type::NativeTypeParameter(parameter(0)))], ..Default::default() })
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> { if key.index == 0 { vec![] } else { vec![member(key.index)] } }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool { key.snapshot == 94001 && (1..4).contains(&key.index) }
    fn type_member_header(&self, original: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader { owner: TypeMemberOwner::Declared(declaration(original.index)), key: TypeMemberKeyRecipe::Fixed(key("value")), visibility: TypeMemberVisibility::Public, optional: false, readonly: false }
    }
    fn type_member_origin(&self, original: NativeTypeMemberKey) -> TableFieldOrigin { if original.index == 3 { TableFieldOrigin::Inferred } else { TableFieldOrigin::Declared } }
    fn type_member_type(&self, original: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type { if original.index == 3 { Type::Number } else { Type::NativeTypeParameter(parameter(original.index)) } }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool { key.snapshot == 94001 && key.index < 4 }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String { "T".into() }
    fn type_parameter_owner(&self, original: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> { Some(TypeDeclarationOwner::NamedType(declaration(original.index))) }
}

#[test]
fn named_member_materialization_retains_factual_inferred_origin_from_original_provider() {
    crate::with_program(|program, handles| {
        let provider = Contracts;
        let adapter = crate::ProviderAdapter { provider: &provider, handles };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        checker.enable_native_arrays();
        for (index, expected) in [(1, TableFieldOrigin::Declared), (3, TableFieldOrigin::Inferred)] {
            let one = checker.number_literal(1.0, true);
            let reference = checker.native_type_reference(crate::native_named::core_declaration(declaration(index)), &[one]).unwrap();
            let materialized = checker.native_object_resolved(reference).unwrap();
            let Type::Table(table) = crate::export_type(&mut checker, handles, materialized) else { panic!("materialized shape is an owned description") };
            assert_eq!(table.fields[0].origin, expected);
            let mut q = crate::SessionQueries { checker: &mut checker, handles };
            let stored = q.widen(&Type::Table(table));
            assert_eq!(description(&mut q, &stored, &access("value")), Ok(if index == 3 { Type::Number } else { Type::NumberLiteral(1.0) }));
        }
    });
}

#[test]
fn a_lazy_written_named_field_is_a_regular_literal_before_native_storage_widening() {
    crate::with_program(|program, handles| {
        let provider = Contracts;
        let adapter = crate::ProviderAdapter { provider: &provider, handles };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        checker.enable_native_arrays();
        let one = checker.number_literal(1.0, true);
        let reference = checker.native_type_reference(crate::native_named::core_declaration(declaration(2)), &[one]).unwrap();
        let key = bun_sema::types::NativeTableAccessKey::Literal(bun_sema::types::NativeTableKey::String(checker.atoms().intern(b"value")));
        let read = checker.native_table_read(reference, key).unwrap();
        assert!(!checker.is_fresh_literal(read), "a lazy written field must have the same regular literal policy as its materialized description");
        let stored = checker.widened(read);
        assert!(matches!(checker.data(stored), TypeData::NumberLit { .. }));
    });
}

#[test]
fn reading_a_written_singleton_retains_its_regular_policy_across_owned_scalar_transport() {
    Engine::new().with_session(|q| {
        let table = q.table_type(&TableType { fields: vec![TableField::declared(key("one"), Type::NumberLiteral(1.0))], fresh: true, ..Default::default() }).unwrap();
        let read = q.table_lookup(&table, &access("one")).unwrap();
        let stored = q.widen(&read);
        assert_eq!(q.format(&stored), "1");
        assert!(!q.is_assignable(&Type::NumberLiteral(2.0), &stored));
    });
}

#[test]
fn original_generic_alias_and_named_member_mapping_preserve_written_mutable_literals() {
    Engine::new().with_native_provider(|| Contracts, |_, q| {
        for (index, literal) in [(0, Type::NumberLiteral(1.0)), (1, Type::StringLiteral(b"shape".to_vec())), (2, Type::BooleanLiteral(false))] {
            let reference = q.named_type_reference(declaration(index), &[literal.clone()]).unwrap();
            let widened = q.widen(&reference);
            assert_eq!(description(q, &widened, &access("value")), Ok(literal.clone()));
            assert_eq!(q.table_write(&widened, &access("value"), &literal), Ok(()));
            assert!(q.table_write(&widened, &access("value"), &Type::NumberLiteral(42.0)).is_err());
        }
        for index in 0..3 {
            let union = Type::Union(vec![Type::NumberLiteral(1.0), Type::NumberLiteral(2.0)]);
            let reference = q.named_type_reference(declaration(index), &[union.clone()]).unwrap();
            let stored = q.widen(&reference);
            let actual = q.table_lookup(&stored, &access("value")).unwrap();
            assert!(q.is_assignable(&actual, &union));
            assert!(q.is_assignable(&union, &actual));
            assert_eq!(q.table_write(&stored, &access("value"), &Type::NumberLiteral(2.0)), Ok(()));
            assert!(q.table_write(&stored, &access("value"), &Type::NumberLiteral(3.0)).is_err());
        }
    });
}

#[test]
fn declared_origin_does_not_bypass_native_session_ownership_guards() {
    let foreign = Engine::new().with_native_provider(|| Contracts, |_, q| {
        q.named_type_reference(declaration(2), &[Type::NumberLiteral(1.0)]).unwrap()
    });
    assert!(matches!(foreign, Type::SessionType(_)));
    let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Engine::new().with_session(|q| {
            q.table_type(&TableType { fields: vec![TableField::declared(key("foreign"), foreign)], fresh: true, ..Default::default() })
        })
    })).expect_err("declared fields must still validate their original native session capability");
    assert!(error.downcast::<InvalidSessionType>().is_ok());
}

#[test]
fn materialized_class_mapping_copies_original_field_origin_before_export() {
    crate::with_program(|program, handles| {
        let provider = Contracts;
        let adapter = crate::ProviderAdapter { provider: &provider, handles };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        checker.enable_native_arrays();
        let generic = checker.native_declared_head(crate::native_named::core_declaration(declaration(2))).unwrap();
        let raw = checker.native_object_resolved(generic).unwrap();
        let param = checker.native_type_parameter(crate::core_parameter_key(parameter(2))).unwrap();
        let one = checker.number_literal(1.0, true);
        let mapper = checker.mapper_from(&[param], &[one]);
        let mapped = checker.instantiate(raw, mapper);
        let TypeData::NativeTable(data) = checker.data(mapped) else { panic!("materialized class") };
        assert_eq!(data.fields[0].origin, NativeTableFieldOrigin::Declared);
        assert!(!data.fields[0].readonly);
        let owned = crate::export_type(&mut checker, handles, mapped);
        assert!(matches!(owned, Type::SessionType(_)));
        let mut q = crate::SessionQueries { checker: &mut checker, handles };
        let stored = q.widen(&owned);
        assert_eq!(description(&mut q, &stored, &access("value")), Ok(Type::NumberLiteral(1.0)));
        assert_eq!(q.table_write(&stored, &access("value"), &Type::NumberLiteral(1.0)), Ok(()));
        assert!(q.table_write(&stored, &access("value"), &Type::NumberLiteral(2.0)).is_err());
    });
}
