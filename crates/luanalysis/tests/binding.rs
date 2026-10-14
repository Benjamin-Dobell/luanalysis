use luanalysis::bind::{BindingId, BoundFile, DeclarationKind, OccurrenceKind, ScopeKind, bind};

fn bound(source: &str) -> BoundFile {
    let syntax = luanalysis_syntax::parse(source);
    assert!(syntax.diagnostics.is_empty(), "{:#?}", syntax.diagnostics);
    bind(&syntax)
}

fn binding(source: &str, graph: &BoundFile, needle: &str, offset: usize) -> BindingId {
    graph
        .binding_at(source.find(needle).unwrap() + offset)
        .unwrap()
}

#[test]
fn initializers_resolve_before_all_names_in_a_local_list_are_declared() {
    let source = "local a, b = 1, 2\nlocal a, b = b, a\nlocal c = a\nlocal d = b";
    let graph = bound(source);
    let initial_a = binding(source, &graph, "local a, b", 6);
    let initial_b = binding(source, &graph, "local a, b", 9);
    assert_eq!(binding(source, &graph, "= b, a", 2), initial_b);
    assert_eq!(binding(source, &graph, "= b, a", 5), initial_a);
    let shadow_a = binding(source, &graph, "local a, b = b", 6);
    let shadow_b = binding(source, &graph, "local a, b = b", 9);
    assert_ne!(initial_a, shadow_a);
    assert_ne!(initial_b, shadow_b);
    assert_eq!(binding(source, &graph, "c = a", 4), shadow_a);
    assert_eq!(binding(source, &graph, "d = b", 4), shadow_b);
    assert_eq!(
        graph.declaration(shadow_a).activation,
        source.find("\nlocal c").unwrap()
    );
}

#[test]
fn repeated_locals_in_the_same_declaration_have_distinct_identities() {
    let source = "local x = 1\nlocal x, x = x, x\nlocal y = x";
    let graph = bound(source);
    assert_eq!(graph.declarations.len(), 4);
    let previous = binding(source, &graph, "local x = 1", 6);
    assert_eq!(binding(source, &graph, "= x, x", 2), previous);
    assert_eq!(binding(source, &graph, "= x, x", 5), previous);
    let first = binding(source, &graph, "local x, x", 6);
    let last = binding(source, &graph, "local x, x", 9);
    assert_ne!(first, last);
    assert_eq!(binding(source, &graph, "y = x", 4), last);
}

#[test]
fn do_blocks_restore_enclosing_bindings_and_preserve_unresolved_globals() {
    let source = "local item = 1\ndo\nlocal item = item\nitem = outside\nend\nlocal after = item";
    let graph = bound(source);
    let outer = binding(source, &graph, "local item = 1", 6);
    let inner = binding(source, &graph, "local item = item", 6);
    assert_ne!(outer, inner);
    assert_eq!(binding(source, &graph, "local item = item", 13), outer);
    assert_eq!(binding(source, &graph, "item = outside", 0), inner);
    assert_eq!(binding(source, &graph, "after = item", 8), outer);
    let global = graph
        .occurrence_at(source.find("outside").unwrap())
        .unwrap();
    assert_eq!(global.kind, OccurrenceKind::Read);
    assert_eq!(global.binding, None);
    assert_eq!(graph.scope(global.scope).kind, ScopeKind::Block);
    assert_eq!(
        graph.scope(graph.declaration(inner).scope).parent,
        Some(graph.declaration(outer).scope)
    );
}

#[test]
fn function_parameters_and_closure_captures_bind_to_the_right_declarations() {
    let source = "local x = 1\nlocal f = function(x)\nlocal g = function()\nreturn x\nend\nreturn g\nend\nlocal after = x";
    let graph = bound(source);
    let outer = binding(source, &graph, "local x = 1", 6);
    let parameter = binding(source, &graph, "function(x)", 9);
    assert_eq!(
        graph.declaration(parameter).kind,
        DeclarationKind::Parameter
    );
    assert_eq!(binding(source, &graph, "return x", 7), parameter);
    assert_eq!(binding(source, &graph, "after = x", 8), outer);
    let functions: Vec<_> = graph
        .scopes
        .iter()
        .filter(|scope| scope.kind == ScopeKind::Function)
        .collect();
    assert_eq!(functions.len(), 2);
    assert!(functions[0].captures.is_empty());
    assert_eq!(functions[1].captures, [parameter]);
}

#[test]
fn local_function_recursion_activates_before_the_body_but_an_initializer_does_not() {
    let source = "local function recursive(value)\nreturn recursive(value)\nend\nlocal ordinary = function()\nreturn ordinary()\nend\nrecursive(1)";
    let graph = bound(source);
    let recursive = binding(source, &graph, "function recursive", 9);
    assert_eq!(
        graph.declaration(recursive).kind,
        DeclarationKind::LocalFunction
    );
    assert_eq!(binding(source, &graph, "return recursive", 7), recursive);
    assert_eq!(binding(source, &graph, "end\nrecursive", 4), recursive);
    let unresolved = graph
        .occurrence_at(source.find("return ordinary").unwrap() + 7)
        .unwrap();
    assert_eq!(unresolved.binding, None);
    let function_scope = graph.scope(
        graph
            .scope_at(source.find("return recursive").unwrap())
            .unwrap(),
    );
    assert_eq!(function_scope.captures, [recursive]);
}

#[test]
fn nested_functions_forward_outer_captures_and_writes_are_occurrences() {
    let source = "local shared = 1\nlocal outer = function()\nlocal inner = function()\nshared = 2\nreturn shared\nend\nreturn inner\nend";
    let graph = bound(source);
    let shared = binding(source, &graph, "local shared", 6);
    let functions: Vec<_> = graph
        .scopes
        .iter()
        .filter(|scope| scope.kind == ScopeKind::Function)
        .collect();
    assert_eq!(functions.len(), 2);
    assert_eq!(functions[0].captures, [shared]);
    assert_eq!(functions[1].captures, [shared]);
    assert_eq!(
        graph
            .references(shared)
            .map(|reference| reference.kind)
            .collect::<Vec<_>>(),
        [OccurrenceKind::Write, OccurrenceKind::Read]
    );
}

#[test]
fn source_queries_distinguish_variables_from_member_and_table_field_names() {
    let source = "local 日本語 = 1\nlocal table = {日本語 = 日本語}\nlocal value = table.日本語";
    let graph = bound(source);
    let id = binding(source, &graph, "local 日本語", 6);
    let declaration = graph.declaration(id);
    assert_eq!(
        &source[declaration.span.start..declaration.span.end],
        "日本語"
    );
    assert_eq!(graph.binding_at(declaration.span.start + 2), Some(id));
    assert_eq!(graph.binding_at(declaration.span.end), None);
    assert_eq!(graph.binding_at(source.find("{日本語").unwrap() + 1), None);
    assert_eq!(binding(source, &graph, "= 日本語}", 2), id);
    assert_eq!(
        graph.binding_at(source.find("table.日本語").unwrap() + 6),
        None
    );
    assert_eq!(graph.references(id).count(), 1);
}

#[test]
fn parameter_annotations_are_source_references_to_parameter_declarations() {
    let source = "---@param value number\n---@return number\nlocal function identity(value)\nreturn value\nend";
    let graph = bound(source);
    let id = binding(source, &graph, "identity(value)", 9);
    assert_eq!(binding(source, &graph, "@param value", 7), id);
    assert_eq!(binding(source, &graph, "return value", 7), id);
    assert_eq!(
        graph
            .references(id)
            .map(|reference| reference.kind)
            .collect::<Vec<_>>(),
        [OccurrenceKind::Annotation, OccurrenceKind::Read]
    );
}

#[test]
fn operators_indices_and_method_calls_preserve_variable_occurrences() {
    let source = "local item = {x = 1}\nlocal index = \"x\"\nlocal argument = 2\nitem[index] = -argument + #index\nlocal result = item:method(argument and not index)";
    let graph = bound(source);
    let item = binding(source, &graph, "local item", 6);
    let index = binding(source, &graph, "local index", 6);
    let argument = binding(source, &graph, "local argument", 6);
    assert_eq!(graph.references(item).count(), 2);
    assert_eq!(graph.references(index).count(), 3);
    assert_eq!(graph.references(argument).count(), 2);
    assert!(
        graph
            .references(item)
            .all(|occurrence| occurrence.kind == OccurrenceKind::Read)
    );
    assert_eq!(graph.binding_at(source.find(":method").unwrap() + 1), None);
    assert_eq!(graph.binding_at(source.find("{x").unwrap() + 1), None);
}
