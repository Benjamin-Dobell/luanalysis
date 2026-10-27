use luanalysis_syntax::{NamedTypeDeclarationKind, StmtKind, TypeExpr, TypeExprKind, parse};

fn alias<'a>(file: &'a luanalysis_syntax::File, index: usize) -> &'a TypeExpr {
    let NamedTypeDeclarationKind::Alias { target } = &file.type_declarations[index].kind else { panic!("alias") };
    target
}

fn text<'a>(source: &'a str, ty: &TypeExpr) -> &'a str {
    &source[ty.span.start..ty.span.end]
}

#[test]
fn keyof_typeof_and_indexed_access_compose_as_in_typescript() {
    let source = "---@alias Value typeof Color[keyof typeof Color]\n---@alias Keys keyof Point | nil\n---@alias Nested Point[\"origin\"][\"x\"]\n---@alias List keyof Point[]\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);

    let TypeExprKind::IndexedAccess { object, index } = &alias(&file, 0).kind else { panic!("indexed access") };
    let TypeExprKind::TypeQuery(path) = &object.kind else { panic!("type query") };
    assert_eq!(path.iter().map(|name| name.text.as_str()).collect::<Vec<_>>(), ["Color"]);
    assert_eq!(text(source, object), "typeof Color");
    let TypeExprKind::KeyOf(operand) = &index.kind else { panic!("keyof") };
    assert_eq!(text(source, operand), "typeof Color");

    // `keyof` binds more tightly than `|`.
    let TypeExprKind::Union(parts) = &alias(&file, 1).kind else { panic!("union") };
    assert!(matches!(&parts[0].kind, TypeExprKind::KeyOf(_)));

    let TypeExprKind::IndexedAccess { object, index } = &alias(&file, 2).kind else { panic!("indexed access") };
    assert_eq!(text(source, object), "Point[\"origin\"]");
    assert_eq!(text(source, index), "\"x\"");

    // And less tightly than an array suffix.
    let TypeExprKind::KeyOf(operand) = &alias(&file, 3).kind else { panic!("keyof") };
    assert!(matches!(operand.kind, TypeExprKind::Array { .. }));
}

#[test]
fn type_queries_name_members_that_touch() {
    let source = "---@type typeof M.config.window\nlocal window\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let StmtKind::Local { annotation: Some(annotation), .. } = &file.statements[0].kind else { panic!("annotated local") };
    let TypeExprKind::TypeQuery(path) = &annotation.types[0].kind else { panic!("type query") };
    assert_eq!(path.iter().map(|name| name.text.as_str()).collect::<Vec<_>>(), ["M", "config", "window"]);
    assert_eq!(text(source, &annotation.types[0]), "typeof M.config.window");
}

#[test]
fn contextual_operators_without_operands_are_names() {
    let source = "---@alias A keyof\n---@alias B typeof\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    assert!(matches!(&alias(&file, 0).kind, TypeExprKind::Named(name) if name == "keyof"));
    assert!(matches!(&alias(&file, 1).kind, TypeExprKind::Named(name) if name == "typeof"));
}

#[test]
fn an_index_touches_its_type_and_arrays_still_parse() {
    let source = "---@param a Point[\"x\"]\n---@param b string[][]\n---@param c Point[] [desc]\nlocal function f(a, b, c) end\n";
    let file = parse(source);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else { panic!("function") };
    let params = &function.annotations.params;
    assert!(matches!(params[0].ty.kind, TypeExprKind::IndexedAccess { .. }));
    assert!(matches!(&params[1].ty.kind, TypeExprKind::Array { element, .. } if matches!(element.kind, TypeExprKind::Array { .. })));
    assert!(matches!(params[2].ty.kind, TypeExprKind::Array { .. }));
    assert_eq!(text(source, &params[2].ty), "Point[]");
}

#[test]
fn an_enum_is_derived_from_the_table_its_statement_assigns() {
    let source = "---@enum Mode\nlocal Mode = { Read = 1 }\n---@enum (key) Side\nM.Side = { left = true }\n---@enum Loose\nlocal Loose = make()\n---@enum (key) Alone\n\nlocal after = {}\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let path = |ty: &TypeExpr| {
        let TypeExprKind::TypeQuery(path) = &ty.kind else { panic!("type query: {ty:#?}") };
        path.iter().map(|name| &source[name.span.start..name.span.end]).collect::<Vec<_>>()
    };
    // `typeof Mode[keyof typeof Mode]`, naming the local where it is declared.
    let TypeExprKind::IndexedAccess { object, index } = &alias(&file, 0).kind else { panic!("indexed access") };
    assert_eq!(path(object), ["Mode"]);
    assert_eq!(object.span.start, source.find("Mode =").unwrap());
    let TypeExprKind::KeyOf(table) = &index.kind else { panic!("keyof") };
    assert_eq!(path(table), ["Mode"]);
    // `keyof typeof M.Side`.
    let TypeExprKind::KeyOf(table) = &alias(&file, 1).kind else { panic!("keyof") };
    assert_eq!(path(table), ["M", "Side"]);
    // Without a table constructor, or a statement, it is still the alias of any value or name.
    assert!(matches!(&alias(&file, 2).kind, TypeExprKind::Named(name) if name == "any"));
    assert!(matches!(&alias(&file, 3).kind, TypeExprKind::Named(name) if name == "string"));
}
