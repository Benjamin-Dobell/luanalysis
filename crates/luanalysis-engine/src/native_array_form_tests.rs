use super::*;

fn numeric_dictionary(value: Type) -> TableType {
    TableType {
        indexers: vec![TableIndexer::declared(Type::Number, value)],
        ..Default::default()
    }
}

#[test]
fn lua_array_admission_distinguishes_sequences_from_dictionaries_and_shapes() {
    Engine::new().with_session(|q| {
        let array = q.array_type(&Type::Number);
        assert!(matches!(array, Type::SessionType(_)));
        let dictionary = q.table_type(&numeric_dictionary(Type::Number)).unwrap();
        assert!(q.is_assignable(&array, &dictionary));
        assert!(!q.is_assignable(&dictionary, &array));
        for fresh in [false, true] {
            for keys in [vec![], vec![1], vec![1, 2, 3]] {
                let dense = q
                    .table_type(&TableType {
                        fields: keys
                            .into_iter()
                            .map(|key| TableField::new(LuaKey::Integer(key), Type::Number))
                            .collect(),
                        fresh,
                        ..Default::default()
                    })
                    .unwrap();
                assert!(q.is_assignable(&dense, &array), "{dense:?}");
            }
            let mut invalid = vec![
                TableType {
                    fields: vec![TableField::new(
                        LuaKey::String(b"enabled".to_vec()),
                        Type::Boolean,
                    )],
                    ..Default::default()
                },
                TableType {
                    fields: vec![
                        TableField::new(LuaKey::Integer(1), Type::Number),
                        TableField::new(LuaKey::String(b"n".to_vec()), Type::Number),
                    ],
                    ..Default::default()
                },
                TableType {
                    fields: vec![TableField::new(LuaKey::Number(1.5), Type::Number)],
                    ..Default::default()
                },
                TableType {
                    fields: vec![TableField::new(LuaKey::Integer(1), Type::String)],
                    ..Default::default()
                },
                numeric_dictionary(Type::Number),
                TableType {
                    indexers: vec![TableIndexer::declared(
                        Type::Union(vec![Type::Number, Type::String]),
                        Type::Number,
                    )],
                    ..Default::default()
                },
            ];
            invalid.extend(
                [vec![1, 3], vec![0, 1], vec![-1, 1]]
                    .into_iter()
                    .map(|keys| TableType {
                        fields: keys
                            .into_iter()
                            .map(|key| TableField::new(LuaKey::Integer(key), Type::Number))
                            .collect(),
                        ..Default::default()
                    }),
            );
            for mut description in invalid {
                description.fresh = fresh;
                let actual = q.table_type(&description).unwrap();
                assert!(!q.is_assignable(&actual, &array), "{description:?}");
            }
        }
        let string_array = q.array_type(&Type::String);
        assert!(!q.is_assignable(&string_array, &array));
        assert!(
            q.table_write(
                &array,
                &TableKey::Literal(LuaKey::Integer(1)),
                &Type::String
            )
            .is_err()
        );
        assert!(
            q.table_write(
                &array,
                &TableKey::Literal(LuaKey::String(b"field".to_vec())),
                &Type::Number
            )
            .is_err()
        );
        assert!(
            q.table_write(
                &array,
                &TableKey::Literal(LuaKey::Integer(1)),
                &Type::Number
            )
            .is_ok()
        );
    });
}

#[test]
fn array_session_exports_and_widening_keep_the_written_sequence_contract() {
    Engine::new().with_session(|q| {
        let array = q.array_type(&Type::Number);
        let dictionary = q.table_type(&numeric_dictionary(Type::Number)).unwrap();
        let inspected = q.inspect_type(&array);
        let widened = q.widen(&array);
        let reduced = q.union_reduced(&[array.clone(), array.clone()]);
        for retained in [&array, &inspected, &widened, &reduced] {
            assert!(matches!(retained, Type::SessionType(_)), "{retained:?}");
            assert!(q.is_assignable(retained, &array));
            assert!(q.is_assignable(retained, &dictionary));
            assert!(!q.is_assignable(&dictionary, retained));
            assert_eq!(q.format(retained), "number[]");
            assert_eq!(
                q.table_lookup(retained, &TableKey::Literal(LuaKey::Integer(1))),
                Ok(Type::Number)
            );
        }
    });
}

fn parameter() -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: 571921,
        index: 0,
    }
}
struct Parameters;
impl NativeTypeProvider for Parameters {
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("no function")
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("no function")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("no function")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no function")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no function")
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key == parameter()
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "T".into()
    }
}

#[test]
fn materialized_array_mapper_substitution_preserves_form_and_element_types() {
    with_program(|program, handles| {
        let provider = Parameters;
        let adapter = ProviderAdapter {
            provider: &provider,
            handles,
        };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        let formal = checker
            .native_type_parameter(core_parameter_key(parameter()))
            .unwrap();
        let raw = checker.native_array_table(formal, false).unwrap();
        let mapper = checker.mapper_from(&[formal], &[TypeId::NUMBER]);
        let mapped = checker.instantiate(raw, mapper);
        let TypeData::NativeTable(table) = checker.data(mapped) else {
            panic!("array schema")
        };
        assert!(table.is_array);
        assert_eq!(table.indexers[0].value, TypeId::NUMBER);
        let exported = export_type(&mut checker, handles, mapped);
        assert!(matches!(exported, Type::SessionType(_)));
        assert_eq!(import_type(&mut checker, handles, &exported), mapped);
        let mut q = SessionQueries {
            checker: &mut checker,
            handles,
        };
        let dictionary = q.table_type(&numeric_dictionary(Type::Number)).unwrap();
        assert!(!q.is_assignable(&dictionary, &exported));
        assert!(q.is_assignable(&exported, &dictionary));
        assert_eq!(q.format(&exported), "number[]");
    });
}

#[test]
fn typescript_intrinsic_arrays_keep_readonly_and_element_relations() {
    with_checker(|checker| {
        let mutable = checker.native_array_of(TypeId::NUMBER, false);
        let readonly = checker.native_array_of(TypeId::NUMBER, true);
        let wrong = checker.native_array_of(TypeId::STRING, false);
        assert!(checker.is_assignable(mutable, readonly));
        assert!(!checker.is_assignable(readonly, mutable));
        assert!(!checker.is_assignable(wrong, mutable));
        assert!(checker.is_assignable(mutable, mutable));
    });
}

#[test]
fn original_nil_slots_keep_sequence_types_without_changing_stored_fields() {
    Engine::new().with_session(|q| {
        let nullable = q.array_type(&Type::Union(vec![Type::Number, Type::Nil]));
        let nonnullable = q.array_type(&Type::Number);
        let source = q
            .table_type(&TableType {
                fields: vec![
                    TableField::new(LuaKey::Integer(1), Type::Nil),
                    TableField::new(LuaKey::Integer(2), Type::Number),
                ],
                fresh: true,
                ..Default::default()
            })
            .unwrap();
        assert!(matches!(source, Type::SessionType(_)));
        let inspected = q.inspect_type(&source);
        let widened = q.widen(&source);
        for value in [&source, &inspected, &widened] {
            assert!(q.is_assignable(value, &nullable), "{value:?}");
            assert!(!q.is_assignable(value, &nonnullable), "{value:?}");
            assert!(
                q.table_lookup(value, &TableKey::Literal(LuaKey::Integer(1)))
                    .is_err(),
                "nil is absent storage"
            );
            assert_eq!(
                q.table_lookup(value, &TableKey::Literal(LuaKey::Integer(2))),
                Ok(Type::Number)
            );
        }
        for fresh in [false, true] {
            let gap = q
                .table_type(&TableType {
                    fields: vec![TableField::new(LuaKey::Integer(2), Type::Number)],
                    fresh,
                    ..Default::default()
                })
                .unwrap();
            assert!(!q.is_assignable(&gap, &nullable));
        }
        let dictionary = q
            .table_type(&numeric_dictionary(Type::Union(vec![
                Type::Number,
                Type::Nil,
            ])))
            .unwrap();
        assert!(!q.is_assignable(&dictionary, &nullable));
        let corrected = q
            .table_type(&TableType {
                fields: vec![
                    TableField::new(LuaKey::Integer(1), Type::Number),
                    TableField::new(LuaKey::Integer(2), Type::Number),
                ],
                fresh: true,
                ..Default::default()
            })
            .unwrap();
        assert!(q.is_assignable(&corrected, &nonnullable));
    });
}

#[test]
fn original_constructor_pack_maps_hidden_nil_and_generic_slots_before_export() {
    with_program(|program, handles| {
        let provider = Parameters;
        let adapter = ProviderAdapter {
            provider: &provider,
            handles,
        };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        checker.enable_native_arrays();
        let formal = checker
            .native_type_parameter(core_parameter_key(parameter()))
            .unwrap();
        let raw = checker
            .native_table(
                &[
                    bun_sema::types::NativeTableField {
                        key: bun_sema::types::NativeTableKey::Integer(1),
                        value: TypeId::UNDEFINED,
                        optional: false,
                        readonly: false,
                        origin: bun_sema::types::NativeTableFieldOrigin::Inferred,
                    },
                    bun_sema::types::NativeTableField {
                        key: bun_sema::types::NativeTableKey::Integer(2),
                        value: formal,
                        optional: false,
                        readonly: false,
                        origin: bun_sema::types::NativeTableFieldOrigin::Inferred,
                    },
                ],
                &[],
                bun_sema::types::NativeTableFlavor::Literal,
            )
            .unwrap();
        let TypeData::NativeTable(table) = checker.data(raw) else {
            panic!("constructor")
        };
        assert!(!table.is_array);
        assert!(table.constructor_sequence.is_some());
        let mapper = checker.mapper_from(&[formal], &[TypeId::NUMBER]);
        let mapped = checker.instantiate(raw, mapper);
        let TypeData::NativeTable(table) = checker.data(mapped) else {
            panic!("mapped constructor")
        };
        let pack = table.constructor_sequence.unwrap();
        assert_eq!(checker.native_pack_slot(pack, 0), TypeId::UNDEFINED);
        assert_eq!(checker.native_pack_slot(pack, 1), TypeId::NUMBER);
        let exported = export_type(&mut checker, handles, mapped);
        assert!(matches!(exported, Type::SessionType(_)));
        assert_eq!(import_type(&mut checker, handles, &exported), mapped);
        let mut q = SessionQueries {
            checker: &mut checker,
            handles,
        };
        let nullable = q.array_type(&Type::Union(vec![Type::Number, Type::Nil]));
        let nonnullable = q.array_type(&Type::Number);
        let widened = q.widen(&exported);
        assert!(q.is_assignable(&widened, &nullable));
        assert!(!q.is_assignable(&widened, &nonnullable));
    });
}

#[test]
fn retained_constructor_rows_follow_the_same_literal_context_as_stored_fields() {
    Engine::new().with_session(|q| {
        let target = q.array_type(&Type::Union(vec![Type::NumberLiteral(1.0), Type::Nil]));
        let description = |number| TableType {
            fields: vec![
                TableField::new(LuaKey::Integer(1), Type::Nil),
                TableField::new(LuaKey::Integer(2), Type::NumberLiteral(number)),
            ],
            fresh: true,
            ..Default::default()
        };
        let contextual = q.table_type_in_context(&description(1.0), &target).unwrap();
        assert!(q.is_assignable(&contextual, &target));
        let stored = q
            .table_lookup(&contextual, &TableKey::Literal(LuaKey::Integer(2)))
            .unwrap();
        assert!(q.is_assignable(&stored, &Type::NumberLiteral(1.0)));
        let wrong = q.table_type_in_context(&description(2.0), &target).unwrap();
        assert!(!q.is_assignable(&wrong, &target));
        let inferred = q.table_type(&description(1.0)).unwrap();
        assert!(!q.is_assignable(&inferred, &target));
        let broad = q.array_type(&Type::Union(vec![Type::Number, Type::Nil]));
        assert!(q.is_assignable(&inferred, &broad));
    });
}

#[test]
fn array_assertions_compare_numeric_facets_without_allowing_packed_assignment() {
    Engine::new().with_session(|q| {
        let array = q.array_type(&Type::Number);
        let packed = q
            .table_type(&TableType {
                fields: vec![TableField::declared(
                    LuaKey::String(b"n".to_vec()),
                    Type::Number,
                )],
                ..numeric_dictionary(Type::Number)
            })
            .unwrap();
        assert!(q.assertion_overlap(&array, &packed));
        assert!(q.assertion_overlap(&packed, &array));
        assert!(!q.is_assignable(&array, &packed));
        assert!(!q.is_assignable(&packed, &array));
        let labelled = q
            .table_type(&TableType {
                fields: vec![TableField::declared(
                    LuaKey::String(b"label".to_vec()),
                    Type::String,
                )],
                ..numeric_dictionary(Type::Number)
            })
            .unwrap();
        assert!(q.assertion_overlap(&array, &labelled));
        assert!(!q.is_assignable(&labelled, &array));
    });
}

#[test]
fn array_assertions_still_check_elements_and_nonoverlapping_indexer_domains() {
    Engine::new().with_session(|q| {
        let array = q.array_type(&Type::Number);
        for description in [
            TableType {
                fields: vec![TableField::declared(
                    LuaKey::String(b"n".to_vec()),
                    Type::Number,
                )],
                ..numeric_dictionary(Type::String)
            },
            TableType {
                indexers: vec![TableIndexer::declared(Type::String, Type::Number)],
                ..Default::default()
            },
            TableType {
                indexers: vec![TableIndexer::declared(Type::Boolean, Type::Number)],
                ..Default::default()
            },
            TableType {
                indexers: vec![
                    TableIndexer::declared(Type::Number, Type::Number),
                    TableIndexer::declared(Type::String, Type::Boolean),
                ],
                ..Default::default()
            },
            TableType {
                fields: vec![
                    TableField::declared(LuaKey::Integer(1), Type::String),
                    TableField::declared(LuaKey::String(b"n".to_vec()), Type::Number),
                ],
                ..numeric_dictionary(Type::Union(vec![Type::Number, Type::String]))
            },
        ] {
            let other = q.table_type(&description).unwrap();
            assert!(!q.assertion_overlap(&array, &other), "{description:?}");
            assert!(!q.assertion_overlap(&other, &array), "{description:?}");
            assert!(!q.is_assignable(&other, &array), "{description:?}");
        }
    });
}
