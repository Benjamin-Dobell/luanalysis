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
fn return_predicate_parameter_occurrence_is_bound_for_references_and_rename() {
    let source = "local value = 1\n---@param value unknown\n---@return value is string\nlocal function isString(value)\nreturn type(value) == 'string'\nend";
    let graph = bound(source);
    let id = binding(source, &graph, "isString(value)", 9);
    let offset = source.find("@return value").unwrap() + 8;
    let occurrence = graph.occurrence_at(offset).unwrap();
    assert_eq!(occurrence.span, luanalysis_syntax::Span::new(offset, offset + "value".len()));
    assert_eq!(occurrence.binding, Some(id));
    assert_eq!(occurrence.kind, OccurrenceKind::Annotation);
    assert_eq!(occurrence.scope, graph.declaration(id).scope);
    assert_eq!(graph.references(id).map(|reference| reference.kind).collect::<Vec<_>>(), [OccurrenceKind::Annotation, OccurrenceKind::Annotation, OccurrenceKind::Read]);
    assert_ne!(id, binding(source, &graph, "local value", 6));
    assert_eq!(graph.binding_at(source.find("is string").unwrap() + 3), None);
}

#[test]
fn unknown_return_predicate_parameter_does_not_bind_an_outer_variable() {
    let source = "local value = 1\n---@return value is string\nlocal function invalid(other)\nreturn true\nend";
    let graph = bound(source);
    let outer = binding(source, &graph, "local value", 6);
    let occurrence = graph.occurrence_at(source.find("@return value").unwrap() + 8).unwrap();
    assert_eq!(occurrence.kind, OccurrenceKind::Annotation);
    assert_eq!(occurrence.binding, None);
    assert_eq!(graph.scope(occurrence.scope).kind, ScopeKind::Function);
    assert!(graph.scope(occurrence.scope).captures.is_empty());
    assert_eq!(graph.references(outer).count(), 0);
}

#[test]
fn return_predicate_self_binds_the_current_method_receiver_or_explicit_parameter() {
    for parameters in ["value", "self"] {
        let source = format!("local self = 1\nlocal guards = {{}}\n---@return self is string\nfunction guards:valid({parameters})\nreturn true\nend");
        let graph = bound(&source);
        let id = binding(&source, &graph, "@return self", 8);
        let declaration = graph.declaration(id);
        assert_ne!(id, binding(&source, &graph, "local self", 6));
        if parameters == "self" {
            assert_eq!(declaration.kind, DeclarationKind::Parameter);
            assert_eq!(&source[declaration.span.start..declaration.span.end], "self");
        } else {
            assert_eq!(declaration.kind, DeclarationKind::ImplicitReceiver);
            assert_eq!(&source[declaration.span.start..declaration.span.end], ":");
        }
    }
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

#[test]
fn conditional_branch_scopes_do_not_leak_into_other_conditions_or_after_end() {
    let source = "local value = 1\nif value then\nlocal value = value\nelseif value then\nlocal value = value\nelse\nlocal value = value\nend\nlocal after = value";
    let graph = bound(source);
    let outer = binding(source, &graph, "local value = 1", 6);
    assert_eq!(binding(source, &graph, "if value then", 3), outer);
    assert_eq!(binding(source, &graph, "elseif value then", 7), outer);
    assert_eq!(binding(source, &graph, "after = value", 8), outer);
    let inner: Vec<_> = graph
        .declarations
        .iter()
        .filter(|declaration| declaration.name == "value" && declaration.id != outer)
        .collect();
    assert_eq!(inner.len(), 3);
    assert_ne!(inner[0].scope, inner[1].scope);
    assert_ne!(inner[1].scope, inner[2].scope);
    for declaration in inner {
        let initializer = graph
            .occurrences
            .iter()
            .find(|occurrence| {
                occurrence.span.start > declaration.span.end
                    && occurrence.kind == OccurrenceKind::Read
                    && occurrence.scope == declaration.scope
            })
            .unwrap();
        assert_eq!(initializer.binding, Some(outer));
    }
}

#[test]
fn repeat_conditions_keep_body_locals_active_then_restore_the_outer_scope() {
    let source = "local value = 1\nrepeat\nlocal value = 2\nuntil value\nlocal after = value";
    let graph = bound(source);
    let outer = binding(source, &graph, "local value = 1", 6);
    let inner = binding(source, &graph, "local value = 2", 6);
    assert_eq!(binding(source, &graph, "until value", 6), inner);
    assert_eq!(binding(source, &graph, "after = value", 8), outer);
    assert_eq!(
        graph.scope_at(source.find("until value").unwrap() + 6),
        Some(graph.declaration(inner).scope)
    );
    assert_eq!(
        graph.scope(graph.declaration(inner).scope).kind,
        ScopeKind::Loop
    );
}

#[test]
fn numeric_loop_headers_resolve_before_the_loop_variable_is_declared() {
    let source = "local i = 1\nfor i = i, i, i do\nlocal value = i\nend\nlocal after = i";
    let graph = bound(source);
    let outer = binding(source, &graph, "local i = 1", 6);
    let counter = binding(source, &graph, "for i = i", 4);
    assert_eq!(
        graph.declaration(counter).kind,
        DeclarationKind::LoopVariable
    );
    assert_ne!(outer, counter);
    let header_start = source.find("for i").unwrap();
    let header_end = source.find(" do").unwrap();
    let reads: Vec<_> = graph
        .occurrences
        .iter()
        .filter(|occurrence| {
            occurrence.kind == OccurrenceKind::Read
                && occurrence.span.start > header_start
                && occurrence.span.end <= header_end
        })
        .collect();
    assert_eq!(reads.len(), 3);
    assert!(
        reads
            .iter()
            .all(|occurrence| occurrence.binding == Some(outer))
    );
    assert_eq!(binding(source, &graph, "value = i", 8), counter);
    assert_eq!(binding(source, &graph, "after = i", 8), outer);
}

#[test]
fn generic_loop_values_resolve_before_all_iteration_variables() {
    let source = "local key = 1\nfor key, value in factory(key) do\nlocal first = key\nlocal second = value\nend\nlocal after = key\nlocal missing = value";
    let graph = bound(source);
    let outer = binding(source, &graph, "local key = 1", 6);
    let key = binding(source, &graph, "for key, value", 4);
    let value = binding(source, &graph, "for key, value", 9);
    assert_ne!(key, outer);
    assert_eq!(binding(source, &graph, "factory(key)", 8), outer);
    assert_eq!(binding(source, &graph, "first = key", 8), key);
    assert_eq!(binding(source, &graph, "second = value", 9), value);
    assert_eq!(binding(source, &graph, "after = key", 8), outer);
    assert_eq!(
        graph.binding_at(source.find("missing = value").unwrap() + 10),
        None
    );
}
