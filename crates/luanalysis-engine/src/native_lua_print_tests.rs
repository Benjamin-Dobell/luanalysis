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
