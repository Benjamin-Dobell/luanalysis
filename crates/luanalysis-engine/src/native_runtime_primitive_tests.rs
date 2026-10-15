use crate::*;

fn proposed_table() -> Type {
    Type::Table(TableType {
        indexers: vec![TableIndexer::new(Type::Any, Type::Any)],
        ..Default::default()
    })
}

fn proposed_function(queries: &mut dyn TypeQueries) -> Type {
    queries.pack_function(&PackFunctionType {
        params: Vec::new(),
        rest: Some(ValuePack::variadic(Type::Any)),
        type_parameters: Vec::new(),
        returns: ValuePack::variadic(Type::Any),
    })
}

#[test]
fn proposed_open_dictionary_rejects_function_runtime_values() {
    Engine::new().with_session(|queries| {
        let callable = queries.pack_function(&PackFunctionType {
            params: vec![Parameter {
                name: "value".to_owned(),
                ty: Type::Number,
                optional: false,
            }],
            rest: None,
            type_parameters: Vec::new(),
            returns: ValuePack::single(Type::Number),
        });
        assert!(!queries.is_assignable(&callable, &proposed_table()));
    });
}

#[test]
fn proposed_generic_callable_accepts_concrete_functions_and_rejects_tables() {
    Engine::new().with_session(|queries| {
        let target = proposed_function(queries);
        for (params, returns) in [
            (Vec::new(), ValuePack::empty()),
            (
                vec![Parameter {
                    name: "value".to_owned(),
                    ty: Type::Number,
                    optional: false,
                }],
                ValuePack::single(Type::Number),
            ),
            (Vec::new(), ValuePack::single(Type::String)),
        ] {
            let function = queries.pack_function(&PackFunctionType {
                params,
                rest: None,
                type_parameters: Vec::new(),
                returns,
            });
            assert!(queries.is_assignable(&function, &target));
        }
        for invalid in [
            Type::Number,
            Type::String,
            Type::Nil,
            Type::Boolean,
            proposed_table(),
            Type::Table(TableType::default()),
        ] {
            assert!(!queries.is_assignable(&invalid, &target), "{invalid:?}");
        }
        let called = queries.call_pack(&target, &[Type::Number, Type::String], &[], None);
        assert!(called.errors.is_empty(), "{:?}", called.errors);
        assert_eq!(queries.pack_slot(&called.return_pack, 0), Type::Any);
        assert_eq!(queries.pack_slot(&called.return_pack, 3), Type::Any);
    });
}

#[test]
fn runtime_tops_keep_native_identity_and_reject_other_runtime_categories() {
    Engine::new().with_session(|queries| {
        for primitive in [Type::RuntimeTable, Type::RuntimeFunction] {
            assert_eq!(queries.inspect_type(&primitive), primitive);
            assert_eq!(queries.widen(&primitive), primitive);
            assert_eq!(
                queries.union_reduced(&[primitive.clone(), primitive.clone()]),
                primitive
            );
            assert!(queries.is_assignable(&primitive, &primitive));
            assert!(queries.is_assignable(&Type::Never, &primitive));
            assert!(queries.is_assignable(&Type::Any, &primitive));
            assert!(queries.is_assignable(&primitive, &Type::Unknown));
            for invalid in [Type::Number, Type::String, Type::Boolean, Type::Nil] {
                assert!(
                    !queries.is_assignable(&invalid, &primitive),
                    "{invalid:?} to {primitive:?}"
                );
                assert!(
                    !queries.is_assignable(&primitive, &invalid),
                    "{primitive:?} to {invalid:?}"
                );
            }
        }
        assert!(!queries.is_assignable(&Type::RuntimeTable, &Type::RuntimeFunction));
        assert!(!queries.is_assignable(&Type::RuntimeFunction, &Type::RuntimeTable));
    });
}

#[test]
fn runtime_table_has_open_native_keys_while_runtime_function_has_result_pack_calls() {
    Engine::new().with_session(|queries| {
        for key in [
            LuaKey::String(b"field".to_vec()),
            LuaKey::Integer(1),
            LuaKey::Boolean(false),
        ] {
            let key = TableKey::Literal(key);
            assert_eq!(
                queries.table_lookup(&Type::RuntimeTable, &key),
                Ok(Type::Any)
            );
            assert_eq!(
                queries.table_write(&Type::RuntimeTable, &key, &Type::Number),
                Ok(())
            );
            assert!(queries.table_lookup(&Type::RuntimeFunction, &key).is_err());
        }
        let callable = proposed_function(queries);
        assert!(queries.is_assignable(&callable, &Type::RuntimeFunction));
        assert!(!queries.is_assignable(&callable, &Type::RuntimeTable));
        assert!(queries.is_assignable(&proposed_table(), &Type::RuntimeTable));
        assert!(!queries.is_assignable(&proposed_table(), &Type::RuntimeFunction));
        let called = queries.call_pack(
            &Type::RuntimeFunction,
            &[Type::Number, Type::String],
            &[],
            None,
        );
        assert!(called.errors.is_empty(), "{:?}", called.errors);
        assert_eq!(queries.pack_slot(&called.return_pack, 0), Type::Any);
        assert_eq!(queries.pack_slot(&called.return_pack, 4), Type::Any);
        assert!(
            queries
                .call_pack(&Type::RuntimeTable, &[], &[], None)
                .errors
                .len()
                > 0
        );
    });
}
