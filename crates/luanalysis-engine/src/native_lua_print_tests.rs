use super::*;

#[test]
fn lua_nil_spelling_is_structural_and_does_not_change_the_typescript_printer() {
    with_program(|program, handles| {
        let mut checker = program.checker();
        let owned = Type::Function(FunctionType {
            params: vec![Parameter {
                name: "value".into(),
                ty: Type::Union(vec![Type::Number, Type::Nil]),
                optional: false,
            }],
            returns: Box::new(Type::Table(TableType {
                fields: vec![
                    TableField::declared(LuaKey::String(b"value".to_vec()), Type::Nil),
                    TableField::declared(
                        LuaKey::String(b"word".to_vec()),
                        Type::StringLiteral(b"undefined".to_vec()),
                    ),
                ],
                ..Default::default()
            })),
        });
        let ty = import_type(&mut checker, handles, &owned);
        let before = checker.type_to_string(ty);
        let lua = String::from_utf8(checker.native_type_to_string(ty)).unwrap();
        assert!(lua.contains("nil"), "{lua}");
        assert!(lua.contains("\"undefined\""), "{lua}");
        assert!(
            !lua.replace("\"undefined\"", "").contains("undefined"),
            "{lua}"
        );
        assert_eq!(checker.type_to_string(ty), before);
        assert_eq!(checker.type_to_string(TypeId::UNDEFINED), b"undefined");
        assert_eq!(checker.native_type_to_string(TypeId::UNDEFINED), b"nil");
        assert_eq!(checker.type_to_string(TypeId::UNDEFINED), b"undefined");
    });
}

#[test]
fn the_lua_facade_formats_nested_nil_without_rewriting_literal_text() {
    Engine::new().with_session(|queries| {
        let ty = queries.array_type(&Type::Union(vec![
            Type::Nil,
            Type::StringLiteral(b"undefined".to_vec()),
        ]));
        let displayed = queries.format(&ty);
        assert!(displayed.contains("nil"), "{displayed}");
        assert!(displayed.contains("\"undefined\""), "{displayed}");
        assert!(
            !displayed.replace("\"undefined\"", "").contains("undefined"),
            "{displayed}"
        );
    });
}

#[test]
fn anonymous_runtime_schema_call_errors_print_actual_members_without_body_queries() {
    use std::cell::Cell;

    const DECLARATION: NativeTypeDeclarationKey = NativeTypeDeclarationKey {
        snapshot: 94_041,
        index: 0,
    };
    const SIGNATURE: NativeSignatureKey = NativeSignatureKey {
        snapshot: 94_041,
        index: 0,
    };
    const EXPRESSION: NativeExpressionKey = NativeExpressionKey {
        snapshot: 94_041,
        node: 0,
    };

    struct RuntimeSchema {
        named: bool,
        seed_reads: Cell<usize>,
        signature_reads: Cell<usize>,
    }
    impl NativeTypeProvider for RuntimeSchema {
        fn contains_function(&self, _: NativeFunctionKey) -> bool {
            panic!("printing written metatable members must not classify a runtime body")
        }
        fn function_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            panic!("printing written members must not demand runtime parameters")
        }
        fn function_declared_return(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            panic!("printing written members must not demand runtime return contracts")
        }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            panic!("printing written members must not infer a body")
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("printing written members must not enter a body cycle")
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("printing written members must not enter a body query")
        }
        fn contains_expression(&self, expression: NativeExpressionKey) -> bool {
            expression == EXPRESSION
        }
        fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
            key == DECLARATION
        }
        fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
            assert_eq!(key, DECLARATION);
            TypeDeclarationHeader {
                kind: TypeDeclarationKind::RuntimeTable,
                name: if self.named {
                    "NamedRuntime".into()
                } else {
                    String::new()
                },
                object_form: Some(ObjectDeclarationForm::Record),
                parameters: Vec::new(),
                outer_parameters: Vec::new(),
            }
        }
        fn runtime_table_constructor(
            &self,
            key: NativeTypeDeclarationKey,
        ) -> Option<NativeExpressionKey> {
            assert_eq!(key, DECLARATION);
            Some(EXPRESSION)
        }
        fn runtime_table_seed(
            &self,
            key: NativeTypeDeclarationKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            assert_eq!(key, DECLARATION);
            self.seed_reads.set(self.seed_reads.get() + 1);
            Some(Type::Table(TableType {
                fields: vec![
                    TableField {
                        readonly: true,
                        ..TableField::declared(LuaKey::String(b"marker".to_vec()), Type::String)
                    },
                    TableField::declared(
                        LuaKey::String(b"__call".to_vec()),
                        Type::NativeSignature(SIGNATURE),
                    ),
                    TableField::declared(
                        LuaKey::String(b"next".to_vec()),
                        Type::Union(vec![
                            Type::NativeNamedReference {
                                declaration: DECLARATION,
                                arguments: Vec::new(),
                            },
                            Type::Nil,
                        ]),
                    ),
                ],
                ..Default::default()
            }))
        }
        fn contains_signature(&self, key: NativeSignatureKey) -> bool {
            key == SIGNATURE
        }
        fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
            assert_eq!(key, SIGNATURE);
            SignatureHeader {
                owner: SignatureOwner::Standalone,
                site: SignatureSite {
                    snapshot: key.snapshot,
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
            assert_eq!(key, SIGNATURE);
            self.signature_reads.set(self.signature_reads.get() + 1);
            vec![Parameter {
                name: "amount".into(),
                ty: Type::Number,
                optional: false,
            }]
        }
        fn signature_declared_return_pack(
            &self,
            key: NativeSignatureKey,
            _: &mut dyn TypeQueries,
        ) -> ValuePack {
            assert_eq!(key, SIGNATURE);
            ValuePack::single(Type::String)
        }
        fn type_declaration_error(&self, error: NamedTypeError) {
            panic!("valid original runtime schema: {error:?}")
        }
    }
    for named in [false, true] {
        Engine::new().with_native_provider(
            || RuntimeSchema {
                named,
                seed_reads: Cell::new(0),
                signature_reads: Cell::new(0),
            },
            |provider, queries| {
                let schema = queries.named_type_reference(DECLARATION, &[]).unwrap();
                if named {
                    assert_eq!(queries.format(&schema), "NamedRuntime");
                    assert_eq!(
                        provider.seed_reads.get(),
                        0,
                        "named identity printing must not materialize a schema"
                    );
                    assert_eq!(provider.signature_reads.get(), 0);
                    return;
                }
                let target = Type::Function(FunctionType {
                    params: vec![Parameter {
                        name: "value".into(),
                        ty: Type::Number,
                        optional: false,
                    }],
                    returns: Box::new(Type::Nil),
                });
                let rejected = queries.call(&target, &[schema.clone()]);
                let [
                    CallError::Argument {
                        source, index: 0, ..
                    },
                ] = rejected.errors.as_slice()
                else {
                    panic!("{:?}", rejected.errors)
                };
                assert_eq!(
                    provider.seed_reads.get(),
                    0,
                    "recording the call error must defer type printing"
                );
                let displayed = queries.format(source);
                for expected in [
                    "marker",
                    "readonly",
                    "string",
                    "__call",
                    "amount: number",
                    "next",
                    "nil",
                    "...",
                ] {
                    assert!(displayed.contains(expected), "{displayed}");
                }
                assert_eq!(displayed.matches("__call").count(), 1, "{displayed}");
                for placeholder in ["contextual type", "NativeType", "snapshot", "NamedRuntime"] {
                    assert!(!displayed.contains(placeholder), "{displayed}");
                }
                assert!(provider.seed_reads.get() > 0);
                assert!(provider.signature_reads.get() > 0);
                assert!(
                    !queries.is_assignable(&schema, &Type::Number),
                    "printing must not change the actual relation"
                );
            },
        );
    }
}
