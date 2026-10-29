use crate::*;

fn checked(source: &str) -> File {
    let file = parse(source);
    assert!(
        file.diagnostics.is_empty(),
        "{source}\n{:#?}",
        file.diagnostics
    );
    file
}

fn record(
    declaration: &NamedTypeDeclaration,
) -> (NamedRecordKind, Option<&TypeExpr>, &[FieldDeclaration]) {
    let NamedTypeDeclarationKind::Record {
        flavor,
        bases,
        fields,
    } = &declaration.kind
    else {
        panic!("expected a named record declaration")
    };
    (*flavor, bases.first(), fields)
}

#[test]
fn standalone_and_eof_aliases_have_independent_original_declaration_ids() {
    let source = "---@alias First number\n---@alias Second First|string";
    let file = checked(source);
    assert!(file.statements.is_empty());
    assert_eq!(file.type_declarations.len(), 2);
    for (index, declaration) in file.type_declarations.iter().enumerate() {
        assert_eq!(declaration.id, NamedTypeDeclarationId(index as u32));
        assert_eq!(declaration.attachment, None);
        assert_eq!(
            &source[declaration.name.span.start..declaration.name.span.end],
            declaration.name.text
        );
    }
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[1].kind else {
        panic!("alias")
    };
    assert!(matches!(&target.kind, TypeExprKind::Union(parts) if parts.len()==2));
}

#[test]
fn multiple_generic_aliases_do_not_leak_parameters_into_a_following_function() {
    let source = "---@alias AliasInSharedComment<T> string\n---@alias AliasInSharedComment2<U> AliasInSharedComment<U>\n---@param value number\nlocal function consume(value) end";
    let file = checked(source);
    assert_eq!(file.type_declarations.len(), 2);
    assert_eq!(file.type_declarations[0].generics[0].name.text, "T");
    assert_eq!(file.type_declarations[1].generics[0].name.text, "U");
    assert_eq!(
        file.type_declarations[0].group_span,
        file.type_declarations[1].group_span
    );
    assert!(
        file.type_declarations
            .iter()
            .all(|decl| decl.attachment == Some(file.statements[0].span))
    );
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function")
    };
    assert!(function.annotations.generics.is_empty());
    assert_eq!(function.annotations.params.len(), 1);
}

#[test]
fn alias_constraints_and_targets_retain_generic_array_and_callback_forms() {
    let source = "---@alias Dictionary<K:string,V> { [K]: V }\n---@alias Callback<T:(number|string)> fun(value:T): T[]\n---@alias RecursiveList<T> T|RecursiveList<T>[]";
    let file = checked(source);
    assert_eq!(file.type_declarations.len(), 3);
    assert_eq!(file.type_declarations[0].generics.len(), 2);
    assert!(
        matches!(&file.type_declarations[0].generics[0].constraint.as_ref().unwrap().kind, TypeExprKind::Named(name) if name=="string")
    );
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[0].kind else {
        panic!("alias")
    };
    let TypeExprKind::Record(fields) = &target.kind else {
        panic!("record")
    };
    assert!(
        matches!(&fields[0].key,TypeFieldKey::Indexed(key) if matches!(&key.kind,TypeExprKind::Named(name) if name=="K"))
    );
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[1].kind else {
        panic!("alias")
    };
    assert!(
        matches!(&target.kind,TypeExprKind::Function(function) if matches!(&function.returns[0].kind,TypeExprKind::Array{..}))
    );
}

#[test]
fn shape_class_module_headers_and_fields_have_separate_owners() {
    let source = "---@shape Base<T> @ Structural shape\n---@field public value T\n---@class Derived<U:number>:Base<U>\n---@field protected [string] U[]\n---@field private secret number\n---@module Namespace\n---@field name string\nlocal Namespace={}";
    let file = checked(source);
    assert_eq!(file.type_declarations.len(), 3);
    let (flavor, base, fields) = record(&file.type_declarations[0]);
    assert_eq!(flavor, NamedRecordKind::Shape);
    assert!(base.is_none());
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].visibility.unwrap().kind, FieldVisibility::Public);
    let (flavor, base, fields) = record(&file.type_declarations[1]);
    assert_eq!(flavor, NamedRecordKind::Class);
    assert!(
        matches!(&base.unwrap().kind,TypeExprKind::Generic {base,args,..} if base.text=="Base"&&args.len()==1)
    );
    assert_eq!(fields.len(), 2);
    assert_eq!(
        fields[0].visibility.unwrap().kind,
        FieldVisibility::Protected
    );
    assert_eq!(fields[1].visibility.unwrap().kind, FieldVisibility::Private);
    assert_eq!(
        &source[fields[1].visibility.unwrap().span.start..fields[1].visibility.unwrap().span.end],
        "private"
    );
    let (flavor, _, fields) = record(&file.type_declarations[2]);
    assert_eq!(flavor, NamedRecordKind::Module);
    assert_eq!(fields.len(), 1);
    assert_eq!(
        file.type_declarations[2].attachment,
        Some(file.statements[0].span)
    );
}

#[test]
fn class_groups_preserve_numeric_string_and_domain_field_keys() {
    let source = "---@class Keys\n---@field [1] number\n---@field [\"1\"] string\n---@field [string] boolean";
    let file = checked(source);
    let (_, _, fields) = record(&file.type_declarations[0]);
    assert_eq!(fields.len(), 3);
    let TypeFieldKey::Indexed(number) = &fields[0].key else {
        panic!("numeric")
    };
    let TypeFieldKey::Indexed(string) = &fields[1].key else {
        panic!("string")
    };
    assert!(matches!(&number.kind,TypeExprKind::Literal(TypeLiteral::Number(value)) if value=="1"));
    assert!(
        matches!(&string.kind,TypeExprKind::Literal(TypeLiteral::String(value)) if value==b"1")
    );
}

#[test]
fn explicit_owner_fields_are_augmentations_not_new_named_declarations() {
    let source = "---@field private <External> value number\n---@class Local\n---@field <External> [string] boolean\n---@field own string";
    let file = checked(source);
    assert_eq!(file.type_declarations.len(), 1);
    assert_eq!(file.type_extensions.len(), 2);
    assert!(
        file.type_extensions
            .iter()
            .all(|extension| extension.owner.text == "External")
    );
    let (_, _, fields) = record(&file.type_declarations[0]);
    assert_eq!(fields.len(), 1);
    assert!(matches!(&fields[0].key,TypeFieldKey::Named(name) if name.text=="own"));
}

#[test]
fn blank_lines_and_runtime_statements_close_named_field_groups() {
    for source in [
        "---@class Example\n\n---@field missing number\nlocal item={}",
        "---@class Example\r\r---@field missing number\rlocal item={}",
        "---@class Example\nlocal item={}\n---@field missing number\nlocal other={}",
    ] {
        let file = parse(source);
        assert_eq!(file.type_declarations.len(), 1);
        assert!(record(&file.type_declarations[0]).2.is_empty());
        assert!(
            file.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("@field has no preceding")),
            "{:#?}",
            file.diagnostics
        );
    }
}

#[test]
fn standalone_alias_group_does_not_attach_across_a_blank_line() {
    for newline in ["\n", "\r\n", "\r"] {
        let file = checked(&format!(
            "---@alias Value number{newline}{newline}local unrelated=1"
        ));
        assert_eq!(file.type_declarations[0].attachment, None);
        assert_eq!(file.statements.len(), 1);
    }
}

#[test]
fn nested_declarations_keep_flat_ids_and_original_runtime_attachments() {
    let source = "---@alias Outer number\nlocal function make()\n---@alias Inner string\nlocal value=\"text\"\nreturn value\nend";
    let file = checked(source);
    assert_eq!(file.type_declarations.len(), 2);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function")
    };
    assert_eq!(
        file.type_declarations[0].attachment,
        Some(file.statements[0].span)
    );
    assert_eq!(
        file.type_declarations[1].attachment,
        Some(function.body[0].span)
    );
    assert_eq!(file.type_declarations[1].id, NamedTypeDeclarationId(1));
    assert!(function.annotations.generics.is_empty());
}

#[test]
fn block_documentation_unicode_crlf_and_trailing_prose_preserve_byte_origins() {
    let source = "-- café\r\n--[=[\r\n---@alias Label \"é😀\" human documentation\r\n---@shape Box<T> human documentation\r\n---@field value T @ Stored value\r\n]=]\r\nlocal Box={}\r\n";
    let file = checked(source);
    assert_eq!(file.type_declarations.len(), 2);
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[0].kind else {
        panic!("alias")
    };
    assert_eq!(&source[target.span.start..target.span.end], "\"é😀\"");
    assert!(
        matches!(&target.kind,TypeExprKind::Literal(TypeLiteral::String(bytes)) if bytes=="é😀".as_bytes())
    );
    let (_, _, fields) = record(&file.type_declarations[1]);
    assert_eq!(
        &source[fields[0].span.start..fields[0].span.end],
        "@field value T"
    );
    assert!(file.type_declarations[1].group_span.start < file.type_declarations[1].span.start);
}

#[test]
fn malformed_named_headers_and_fields_recover_at_the_next_runtime_statement() {
    for malformed in [
        "---@alias",
        "---@alias Missing",
        "---@alias Empty<> number",
        "---@alias Trailing<T,> T",
        "---@alias Broken<T string",
        "---@alias Bare<T:number|string> T",
        "---@class",
        "---@class Broken<T:> :",
        "---@class Owner\n---@field [] number",
        "---@field private <Broken value number",
    ] {
        let source = format!("{malformed}\nlocal recovery=42");
        let file = parse(&source);
        assert!(
            !file.diagnostics.is_empty(),
            "malformed declaration accepted: {malformed}"
        );
        assert!(file.statements.iter().any(|statement| matches!(&statement.kind,StmtKind::Local{names,..} if names[0].text=="recovery")), "{malformed}: {:#?}", file.diagnostics);
    }
}

#[test]
fn recursive_alias_syntax_retains_references_without_expanding_shapes() {
    let source = "---@alias JSONObject table<string,JSONValue>\n---@alias JSONArray JSONValue[]\n---@alias JSONContainer JSONObject|JSONArray\n---@alias JSONValue JSONContainer|number|string|boolean|nil";
    let file = checked(source);
    assert_eq!(file.type_declarations.len(), 4);
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[0].kind else {
        panic!("alias")
    };
    let TypeExprKind::Generic { args, .. } = &target.kind else {
        panic!("table")
    };
    assert!(matches!(&args[1].kind,TypeExprKind::Named(name) if name=="JSONValue"));
}

#[test]
fn named_alias_type_depth_remains_bounded_before_clone_and_drop() {
    let source = format!(
        "---@alias Deep {}number{}\nlocal recovery=1",
        "(".repeat(512),
        ")".repeat(512)
    );
    let file = parse(&source);
    assert!(!file.diagnostics.is_empty());
    let cloned = file.clone();
    drop(file);
    drop(cloned);
}

#[test]
fn readonly_modifies_a_field_only_before_a_key_and_type() {
    let source = "---@class Flags\n---@field readonly boolean\n---@field readonly readonly boolean\n---@field readonly ready boolean\n---@field private readonly hidden number\n---@field readonly string[]\n---@field readonly boolean # described\n---@field readonly fun(): boolean\n---@field readonly [string] number\n---@field readonly maybe? string\n---@field readonly string|nil\n---@field readonly table<string, number>";
    let file = checked(source);
    let (_, _, fields) = record(&file.type_declarations[0]);
    let named = |field: &FieldDeclaration| match &field.key {
        TypeFieldKey::Named(name) => name.text.clone(),
        TypeFieldKey::Indexed(_) => "[]".to_owned(),
        TypeFieldKey::Sequence { .. } => unreachable!(),
    };
    let summary: Vec<_> = fields.iter().map(|field| (named(field), field.readonly.is_some())).collect();
    assert_eq!(summary, [
        ("readonly", false), ("readonly", true), ("ready", true), ("hidden", true), ("readonly", false),
        ("readonly", false), ("readonly", false), ("[]", true), ("maybe", true), ("readonly", false), ("readonly", false),
    ].map(|(name, readonly)| (name.to_owned(), readonly)));
    let modifier = fields[1].readonly.unwrap();
    assert_eq!(&source[modifier.start..modifier.end], "readonly");
    let TypeFieldKey::Named(name) = &fields[1].key else { panic!("named") };
    assert!(name.span.start > modifier.end);
    assert_eq!(fields[3].visibility.unwrap().kind, FieldVisibility::Private);
    assert!(matches!(fields[4].ty.kind, TypeExprKind::Array { .. }));
    assert!(matches!(fields[6].ty.kind, TypeExprKind::Function(_)));
    assert!(matches!(fields[8].ty.kind, TypeExprKind::Union(_)));
    let described = checked("---@class Described\n---@field readonly boolean\n--- Whether it is locked.\n---@field readonly locked\n--- boolean");
    let (_, _, fields) = record(&described.type_declarations[0]);
    assert!(fields.iter().all(|field| field.readonly.is_none() && matches!(&field.key, TypeFieldKey::Named(name) if name.text == "readonly")));
}
