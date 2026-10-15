use luanalysis::bind::{DeclarationKind, OccurrenceKind, ScopeKind, bind};
use luanalysis_syntax::{StmtKind, parse};

#[test]
fn colon_receiver_is_an_original_semantic_binding_without_changing_written_parameters() {
    let source = "-- λ\r\nlocal object={}\r\n---@param self table\r\nfunction object:method(value,...) self=value; return self end";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let StmtKind::FunctionDeclaration {
        function,
        method_span,
        ..
    } = &file.statements[1].kind
    else {
        panic!("method")
    };
    assert_eq!(function.params.len(), 1);
    assert_eq!(function.params[0].text, "value");
    assert!(function.vararg.is_some());
    let bound = bind(&file);
    let receiver = bound
        .implicit_receiver(function.span)
        .expect("Lua receiver");
    let declaration = bound.declaration(receiver);
    assert_eq!(declaration.kind, DeclarationKind::ImplicitReceiver);
    assert_eq!(declaration.span, method_span.unwrap());
    assert_eq!(&source[declaration.span.start..declaration.span.end], ":");
    assert_eq!(declaration.activation, function.body_span.start);
    for occurrence in bound
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.name == "self")
    {
        assert_eq!(occurrence.binding, Some(receiver), "{occurrence:#?}");
    }
    assert!(
        bound
            .references(receiver)
            .any(|occurrence| occurrence.kind == OccurrenceKind::Write)
    );
    assert!(
        bound
            .references(receiver)
            .any(|occurrence| occurrence.kind == OccurrenceKind::Annotation)
    );
}

#[test]
fn an_explicit_self_parameter_shadows_the_real_implicit_receiver() {
    let file = parse("local object={}\nfunction object:method(self) return self end");
    let StmtKind::FunctionDeclaration { function, .. } = &file.statements[1].kind else {
        panic!("method")
    };
    let bound = bind(&file);
    let implicit = bound.implicit_receiver(function.span).unwrap();
    let explicit = bound.binding_at(function.params[0].span.start).unwrap();
    assert_ne!(implicit, explicit);
    assert_eq!(bound.declaration(explicit).kind, DeclarationKind::Parameter);
    let read = bound
        .occurrences
        .iter()
        .find(|occurrence| occurrence.name == "self" && occurrence.kind == OccurrenceKind::Read)
        .unwrap();
    assert_eq!(read.binding, Some(explicit));
}

#[test]
fn receiver_captures_writes_and_local_initializer_shadowing_use_existing_lexical_rules() {
    let source = "local object={}\nfunction object:method()\nlocal function captured() self=object; return self end\nlocal self=self\nreturn self\nend";
    let file = parse(source);
    let StmtKind::FunctionDeclaration { function, .. } = &file.statements[1].kind else {
        panic!("method")
    };
    let bound = bind(&file);
    let receiver = bound.implicit_receiver(function.span).unwrap();
    let nested = bound
        .scopes
        .iter()
        .find(|scope| scope.kind == ScopeKind::Function && scope.span != function.span)
        .unwrap();
    assert!(nested.captures.contains(&receiver));
    assert!(
        bound
            .references(receiver)
            .any(|occurrence| occurrence.kind == OccurrenceKind::Write)
    );
    let local = bound
        .declarations
        .iter()
        .find(|declaration| {
            declaration.name == "self" && declaration.kind == DeclarationKind::Local
        })
        .unwrap();
    let initializer = source.find("local self=self").unwrap() + "local self=".len();
    assert_eq!(bound.binding_at(initializer), Some(receiver));
    let final_read = source.rfind("return self").unwrap() + "return ".len();
    assert_eq!(bound.binding_at(final_read), Some(local.id));
}

#[test]
fn dot_definitions_and_anonymous_functions_do_not_create_an_implicit_self() {
    let source = "local object={}\nfunction object.method() return self end\nlocal anonymous=function() return self end";
    let file = parse(source);
    let bound = bind(&file);
    assert!(
        !bound
            .declarations
            .iter()
            .any(|declaration| declaration.kind == DeclarationKind::ImplicitReceiver)
    );
    assert!(
        bound
            .occurrences
            .iter()
            .filter(|occurrence| occurrence.name == "self")
            .all(|occurrence| occurrence.binding.is_none())
    );
}

#[test]
fn each_colon_declaration_has_its_own_receiver_identity() {
    let file = parse(
        "local object={}\nfunction object:first() return self end\nfunction object:second() return self end",
    );
    let bound = bind(&file);
    let receivers: Vec<_> = file
        .statements
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::FunctionDeclaration { function, .. } => {
                bound.implicit_receiver(function.span)
            }
            _ => None,
        })
        .collect();
    assert_eq!(receivers.len(), 2);
    assert_ne!(receivers[0], receivers[1]);
}
