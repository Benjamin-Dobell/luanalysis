use luanalysis_syntax::{NamedTypeDeclarationKind, TypeExprKind, parse};

#[test]
fn inheritance_retains_the_entire_original_type_expression_for_semantic_validation() {
    for base in [
        "number|string",
        "Base<number>|nil",
        "table<string,number>",
        "number[]",
    ] {
        let source = format!("---@class Derived:{base}\n");
        let file = parse(&source);
        assert!(
            file.diagnostics.is_empty(),
            "{source}\n{:#?}",
            file.diagnostics
        );
        let NamedTypeDeclarationKind::Record {
            base: Some(parsed), ..
        } = &file.type_declarations[0].kind
        else {
            panic!("original base expression missing")
        };
        assert_eq!(&source[parsed.span.start..parsed.span.end], base);
        if base.contains('|') {
            assert!(matches!(parsed.kind, TypeExprKind::Union(_)));
        }
    }
}

#[test]
fn named_generic_base_keeps_its_own_argument_and_full_expression_spans() {
    let source = "---@shape Derived<T>:Base<Box<T>>\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let NamedTypeDeclarationKind::Record {
        base: Some(base), ..
    } = &file.type_declarations[0].kind
    else {
        panic!("base")
    };
    let TypeExprKind::Generic { args, .. } = &base.kind else {
        panic!("generic base")
    };
    assert_eq!(&source[base.span.start..base.span.end], "Base<Box<T>>");
    assert_eq!(&source[args[0].span.start..args[0].span.end], "Box<T>");
}
