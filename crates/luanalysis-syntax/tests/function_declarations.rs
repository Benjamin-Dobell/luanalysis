use luanalysis_syntax::{
    DiagnosticOrigin, DocFunctionDeclarationId, DocFunctionOwner, FieldDeclaration, File,
    FunctionAnnotations, NamedTypeDeclarationKind, Span, StmtKind, TypeExprKind, TypeFieldKey,
    parse,
};

fn checked(source: &str) -> File {
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    file
}

fn text(source: &str, span: Span) -> &str {
    &source[span.start..span.end]
}

fn field_name(field: &FieldDeclaration) -> &str {
    let TypeFieldKey::Named(name) = &field.key else {
        panic!("named field")
    };
    &name.text
}

#[test]
fn qualified_fields_and_deprecated_angle_owners_share_original_owner_metadata() {
    let source = "-- α\r\n---@field Example.callback fun(value:string):number\r\n---@field <Example> legacy fun(value:number):string";
    let file = checked(source);
    assert!(file.type_declarations.is_empty());
    assert!(file.function_declarations.is_empty());
    assert_eq!(file.type_extensions.len(), 2);
    for (extension, expected) in file.type_extensions.iter().zip(["callback", "legacy"]) {
        let field = &extension.fields[0];
        assert_eq!(extension.owner.text, "Example");
        assert_eq!(text(source, extension.owner.span), "Example");
        assert_eq!(field.explicit_owner.as_ref(), Some(&extension.owner));
        assert_eq!(field_name(field), expected);
        assert!(matches!(&field.ty.kind, TypeExprKind::Function(_)));
    }
    assert_eq!(
        file.type_extensions[0].fields[0].deprecated_owner_span,
        None
    );
    assert_eq!(
        text(
            source,
            file.type_extensions[1].fields[0]
                .deprecated_owner_span
                .unwrap()
        ),
        "<Example>"
    );
}

#[test]
fn qualified_fields_choose_the_written_owner_over_a_documentation_owner() {
    let file = checked(
        "---@class Example\n---@field Example.callback fun():number\n---@field Other.callback fun():string",
    );
    let NamedTypeDeclarationKind::Record { fields, .. } = &file.type_declarations[0].kind else {
        panic!("record")
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].explicit_owner.as_ref().unwrap().text, "Example");
    assert_eq!(file.type_extensions[0].owner.text, "Other");
}

#[test]
fn bodyless_globals_have_declaration_identity_and_no_runtime_body() {
    let source = "---@function callback(value:string):number";
    let file = checked(source);
    assert!(file.statements.is_empty());
    assert_eq!(file.function_declarations.len(), 1);
    let declaration = &file.function_declarations[0];
    assert_eq!(declaration.id, DocFunctionDeclarationId(0));
    assert_eq!(declaration.owner, None);
    assert!(!declaration.invalid_owner);
    assert_eq!(text(source, declaration.name.span), "callback");
    assert_eq!(text(source, declaration.tag_span), "@function");
    assert_eq!(
        text(source, declaration.signature.span),
        "callback(value:string):number"
    );
    assert_eq!(
        text(source, declaration.signature.parameters_span.unwrap()),
        "(value:string)"
    );
    assert_eq!(declaration.signature.params.len(), 1);
    assert_eq!(declaration.signature.returns.len(), 1);
    assert_eq!(declaration.annotations, FunctionAnnotations::default());
    assert_eq!(declaration.group_span, file.comments[0].span);
}

#[test]
fn explicit_dot_and_colon_members_preserve_only_original_parameters() {
    let source = "---@function Example.callback(value:string):number\n---@function Example:callback(value:number):string";
    let file = checked(source);
    for (index, declaration) in file.function_declarations.iter().enumerate() {
        assert_eq!(declaration.id, DocFunctionDeclarationId(index as u32));
        let Some(DocFunctionOwner::Explicit(owner)) = &declaration.owner else {
            panic!("explicit owner")
        };
        assert_eq!(owner.text, "Example");
        assert!(!declaration.invalid_owner);
        assert_eq!(text(source, owner.span), "Example");
        assert_eq!(declaration.signature.params.len(), 1);
        assert_eq!(declaration.signature.params[0].name.text, "value");
        assert_eq!(
            declaration.method_colon.map(|span| text(source, span)),
            (index == 1).then_some(":")
        );
    }
    assert_eq!(
        file.function_declarations[0].group_span,
        file.function_declarations[1].group_span
    );
}

#[test]
fn contextual_owners_use_the_complete_same_class_or_shape_documentation_block() {
    for source in [
        "---@class Example\n---@function callback(value:string):number\n---@function :callback(value:number):string",
        "---@function callback(value:string):number\n---@function :callback(value:number):string\n---@shape Example",
    ] {
        let file = checked(source);
        for declaration in &file.function_declarations {
            assert!(!declaration.invalid_owner);
            assert_eq!(
                declaration.owner,
                Some(DocFunctionOwner::Contextual(file.type_declarations[0].id))
            );
            assert_eq!(declaration.group_span, file.type_declarations[0].group_span);
            assert_eq!(declaration.signature.params.len(), 1);
        }
        assert_eq!(file.function_declarations[0].method_colon, None);
        assert_eq!(
            file.function_declarations[1]
                .method_colon
                .map(|span| text(source, span)),
            Some(":")
        );
    }
}

#[test]
fn ambiguous_contextual_owners_are_diagnosed_but_explicit_owners_win() {
    let source = "---@class First\n---@function callback():number\n---@function Explicit:callback():string\n---@shape Second\nlocal recovery=42";
    let file = parse(source);
    assert_eq!(file.diagnostics.len(), 1, "{:#?}", file.diagnostics);
    let diagnostic = &file.diagnostics[0];
    assert_eq!(diagnostic.origin, DiagnosticOrigin::Annotation);
    assert!(
        diagnostic
            .message
            .contains("multiple class or shape owners")
    );
    assert_eq!(text(source, diagnostic.span), "callback");
    assert_eq!(file.function_declarations[0].owner, None);
    assert!(file.function_declarations[0].invalid_owner);
    assert!(!file.function_declarations[1].invalid_owner);
    assert!(
        matches!(&file.function_declarations[1].owner, Some(DocFunctionOwner::Explicit(owner)) if owner.text == "Explicit")
    );
    assert!(
        matches!(&file.statements[0].kind, StmtKind::Local { names, .. } if names[0].text == "recovery")
    );
    assert!(!file.has_lua_syntax_errors());
}

#[test]
fn blank_lines_ordinary_comments_and_runtime_statements_end_owner_context() {
    for separator in ["\n\n", "\r\n\r\n", "\n-- ordinary\n", "\nlocal value={}\n"] {
        let source = format!("---@class Example{separator}---@function callback():number");
        let file = checked(&source);
        assert_eq!(file.function_declarations[0].owner, None, "{separator:?}");
        assert_ne!(
            file.type_declarations[0].group_span,
            file.function_declarations[0].group_span
        );
    }
}

#[test]
fn colon_without_an_owner_is_an_annotation_error_and_does_not_consume_a_tag() {
    let source = "---@function :callback():number\n---@function next(value:string):string";
    let file = parse(source);
    assert_eq!(file.function_declarations.len(), 2);
    assert!(file.function_declarations[0].invalid_owner);
    assert!(!file.function_declarations[1].invalid_owner);
    assert_eq!(file.diagnostics.len(), 1, "{:#?}", file.diagnostics);
    assert!(
        file.diagnostics[0]
            .message
            .contains("requires an explicit or documentation-group owner")
    );
    assert_eq!(text(source, file.diagnostics[0].span), "callback");
    assert_eq!(file.function_declarations[1].name.text, "next");
}

#[test]
fn expanded_annotations_belong_to_the_bodyless_head_and_do_not_leak_to_runtime() {
    let source = "---@generic T\n---@function identity(value)\n---@param value T\n---@return T\n\nlocal function physical(value) return value end";
    let file = checked(source);
    let declaration = &file.function_declarations[0];
    assert_eq!(declaration.signature.params[0].ty, None);
    assert_eq!(declaration.annotations.generics[0].name.text, "T");
    assert_eq!(declaration.annotations.params[0].name.text, "value");
    assert_eq!(
        declaration
            .annotations
            .returns
            .as_ref()
            .unwrap()
            .types
            .len(),
        1
    );
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("runtime function")
    };
    assert_eq!(function.annotations, FunctionAnnotations::default());
}

#[test]
fn long_documentation_keeps_each_expanded_signature_metadata_separate() {
    let source = "--[[---\n@generic T\n@function first(value)\n@param value T\n@return T\n@function second(value)\n@param value number\n@return string\n]]";
    let file = checked(source);
    assert_eq!(file.function_declarations.len(), 2);
    let first = &file.function_declarations[0];
    let second = &file.function_declarations[1];
    assert_eq!(first.annotations.generics.len(), 1);
    assert!(second.annotations.generics.is_empty());
    assert!(
        matches!(&first.annotations.params[0].ty.kind, TypeExprKind::Named(name) if name == "T")
    );
    assert!(
        matches!(&second.annotations.params[0].ty.kind, TypeExprKind::Named(name) if name == "number")
    );
    assert!(
        matches!(&first.annotations.returns.as_ref().unwrap().types[0].kind, TypeExprKind::Named(name) if name == "T")
    );
    assert!(
        matches!(&second.annotations.returns.as_ref().unwrap().types[0].kind, TypeExprKind::Named(name) if name == "string")
    );
}

#[test]
fn repeated_headers_retain_separate_generic_scopes_varargs_and_correlated_return_packs() {
    let source = "---@function choose<T>(value:T,...:T):false|(true,T)\n---@function choose<T:number>(value:T):T,T";
    let file = checked(source);
    let first = &file.function_declarations[0];
    let second = &file.function_declarations[1];
    assert_eq!(first.name.text, second.name.text);
    assert_ne!(first.id, second.id);
    assert_ne!(
        first.signature.generics[0].name.span,
        second.signature.generics[0].name.span
    );
    assert_eq!(
        text(
            source,
            first
                .signature
                .vararg
                .as_ref()
                .unwrap()
                .ellipsis_span
                .unwrap()
        ),
        "..."
    );
    let packs = first.signature.packs.as_ref().unwrap();
    assert_eq!(packs.alternatives[0].fixed, 0..1);
    assert_eq!(packs.alternatives[1].fixed, 1..3);
    assert_eq!(first.signature.returns.len(), 3);
    assert_eq!(second.signature.returns.len(), 2);
    assert!(second.signature.generics[0].constraint.is_some());
}

#[test]
fn unicode_crlf_origins_remain_original_at_eof() {
    let source = "-- α\r\n---@class Résumé\r\n---@function :répondre(entrée:string):number";
    let file = checked(source);
    let declaration = &file.function_declarations[0];
    assert_eq!(text(source, declaration.name.span), "répondre");
    assert_eq!(
        text(source, declaration.signature.params[0].name.span),
        "entrée"
    );
    assert_eq!(text(source, declaration.method_colon.unwrap()), ":");
    assert_eq!(
        text(source, declaration.signature.returns[0].span),
        "number"
    );
    assert_eq!(declaration.group_span.end, source.len());
}

#[test]
fn malformed_headers_recover_at_the_next_valid_tag_and_runtime_declaration() {
    for malformed in [
        "---@function",
        "---@function Example.",
        "---@function Example:",
        "---@function callback",
        "---@function callback(value:",
        "---@function fun Example.callback(value:string):number",
        "---@field Example. fun():number",
    ] {
        let source =
            format!("{malformed}\n---@function healthy(value:string):number\nlocal recovery=42");
        let file = parse(&source);
        assert!(!file.diagnostics.is_empty(), "{malformed}");
        assert!(
            !file.has_lua_syntax_errors(),
            "{malformed}: {:#?}",
            file.diagnostics
        );
        assert!(
            file.function_declarations
                .iter()
                .any(|declaration| declaration.name.text == "healthy"
                    && declaration.signature.returns.len() == 1),
            "{malformed}"
        );
        assert!(
            matches!(&file.statements[0].kind, StmtKind::Local { names, .. } if names[0].text == "recovery")
        );
    }
}

#[test]
fn malformed_long_documentation_head_keeps_the_following_original_head() {
    let source = "--[[---\n@function Broken:missing(value:\n@function healthy(value:string):number\n]]\nlocal recovery=42";
    let file = parse(source);
    assert!(!file.diagnostics.is_empty());
    assert!(!file.has_lua_syntax_errors());
    let healthy = file
        .function_declarations
        .iter()
        .find(|declaration| declaration.name.text == "healthy")
        .unwrap();
    assert_eq!(
        text(
            source,
            healthy.signature.params[0].ty.as_ref().unwrap().span
        ),
        "string"
    );
    assert_eq!(text(source, healthy.signature.returns[0].span), "number");
    assert!(
        matches!(&file.statements[0].kind, StmtKind::Local { names, .. } if names[0].text == "recovery")
    );
}

#[test]
fn overload_metadata_keeps_one_canonical_head_and_the_bodyless_owner() {
    let source = "---@function choose(value:string):string\n---@overload fun(value:number):number\n---@param value string|number\n---@return string|number\n\nlocal function physical(value) return value end";
    let file = checked(source);
    let declaration = &file.function_declarations[0];
    assert_eq!(declaration.annotations.overloads.len(), 1);
    let overload = &file.overload_annotations[declaration.annotations.overloads[0].id.0 as usize];
    assert_eq!(
        text(source, overload.signature.span),
        "fun(value:number):number"
    );
    assert_eq!(overload.group_span, declaration.group_span);
    assert_eq!(declaration.annotations.params.len(), 1);
    assert!(declaration.annotations.returns.is_some());
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("runtime function")
    };
    assert_eq!(function.annotations, FunctionAnnotations::default());
}
