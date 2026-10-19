//! Editor outlines and folds use the current source and retain real source identities.
use luanalysis::language_service::{
    DocumentSymbol, FoldKind, SymbolKind, document_symbols, folding_ranges,
};
use luanalysis_syntax::Span;

fn names(symbols: &[DocumentSymbol]) -> Vec<(&str, SymbolKind)> {
    symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.kind))
        .collect()
}

fn selected<'a>(source: &'a str, symbol: &DocumentSymbol) -> &'a str {
    &source[symbol.selection.start..symbol.selection.end]
}

fn validate(source: &str, symbols: &[DocumentSymbol], parent: Option<Span>) {
    for symbol in symbols {
        assert!(
            symbol.range.start <= symbol.selection.start
                && symbol.selection.end <= symbol.range.end,
            "{symbol:?}"
        );
        assert!(symbol.selection.start < symbol.selection.end, "{symbol:?}");
        assert!(
            source.get(symbol.range.start..symbol.range.end).is_some(),
            "{symbol:?}"
        );
        assert!(
            source
                .get(symbol.selection.start..symbol.selection.end)
                .is_some(),
            "{symbol:?}"
        );
        if let Some(parent) = parent {
            assert!(
                parent.start <= symbol.range.start && symbol.range.end <= parent.end,
                "{symbol:?} outside {parent:?}"
            );
        }
        validate(source, &symbol.children, Some(symbol.range));
    }
}

#[test]
fn original_structure_view_fixture_has_the_original_class_method_hierarchy() {
    let source = include_str!(
        "../../luanalysis-fixtures/tests/legacy/embedded/StructureViewTest/testStructureViewBase/main.lua"
    );
    let symbols = document_symbols(source);
    assert_eq!(names(&symbols), [("A", SymbolKind::Class)]);
    assert_eq!(
        names(&symbols[0].children),
        [("method", SymbolKind::Method)]
    );
    assert_eq!(selected(source, &symbols[0]), "A");
    assert_eq!(selected(source, &symbols[0].children[0]), "method");
    validate(source, &symbols, None);
}

#[test]
fn a_class_collects_annotated_constructor_and_later_members_under_its_runtime_alias() {
    let source = "-- 😀 current unsaved outline\n---@class Widget\n---@field size number\nlocal widget = { name = 'x', reset = function() end }\nfunction widget:run() local result = 1 return result end\nwidget.count = 2\nwidget.execute = function(value) return value end\n";
    let symbols = document_symbols(source);
    assert_eq!(names(&symbols), [("Widget", SymbolKind::Class)]);
    assert_eq!(
        names(&symbols[0].children),
        [
            ("size", SymbolKind::Field),
            ("name", SymbolKind::Field),
            ("reset", SymbolKind::Method),
            ("run", SymbolKind::Method),
            ("count", SymbolKind::Field),
            ("execute", SymbolKind::Method),
        ]
    );
    let run = &symbols[0].children[3];
    assert_eq!(names(&run.children), [("result", SymbolKind::Variable)]);
    assert_eq!(run.detail.as_deref(), Some("()"));
    assert_eq!(selected(source, run), "run");
    assert_eq!(selected(source, &symbols[0]), "Widget");
    validate(source, &symbols, None);
}

#[test]
fn function_outlines_include_direct_locals_nested_functions_and_callbacks_without_loop_local_noise()
{
    let source = "local function outer()\n local count <const> = 1\n local function inner() local value = 2 end\n local closure = function(a) local result = a end\n for i = 1, 2 do local hidden = i local function loop_function() end end\n consume(function() local callback_local = 1 end)\nend\n";
    let symbols = document_symbols(source);
    assert_eq!(names(&symbols), [("outer", SymbolKind::Function)]);
    assert_eq!(
        names(&symbols[0].children),
        [
            ("count", SymbolKind::Constant),
            ("inner", SymbolKind::Function),
            ("closure", SymbolKind::Function),
            ("loop_function", SymbolKind::Function),
            ("<anonymous>", SymbolKind::Function),
        ]
    );
    assert_eq!(
        names(&symbols[0].children[1].children),
        [("value", SymbolKind::Variable)]
    );
    assert_eq!(
        names(&symbols[0].children[2].children),
        [("result", SymbolKind::Variable)]
    );
    assert_eq!(
        names(&symbols[0].children[4].children),
        [("callback_local", SymbolKind::Variable)]
    );
    assert_eq!(selected(source, &symbols[0].children[4]), "function");
    validate(source, &symbols, None);
}

#[test]
fn lexical_shadowing_does_not_attach_members_to_an_outer_class() {
    let source = "---@class Outer\nlocal object = {}\nlocal function build()\n local object = {}\n function object:inside() end\nend\ndo\n local object = {}\n function object:hidden() end\nend\nfunction object:outside() end\n";
    let symbols = document_symbols(source);
    assert_eq!(
        names(&symbols),
        [
            ("Outer", SymbolKind::Class),
            ("build", SymbolKind::Function)
        ]
    );
    assert_eq!(
        names(&symbols[0].children),
        [("outside", SymbolKind::Method)]
    );
    let local_object = &symbols[1].children[0];
    assert_eq!(local_object.name, "object");
    assert_eq!(
        names(&local_object.children),
        [("inside", SymbolKind::Method)]
    );
    validate(source, &symbols, None);
}

#[test]
fn methods_added_from_a_nested_function_still_belong_to_the_resolved_outer_class() {
    let source = "---@class Outer\nlocal object = {}\nlocal function install()\n object.added = function() local state = 1 end\nend\n";
    let symbols = document_symbols(source);
    assert_eq!(names(&symbols[0].children), [("added", SymbolKind::Method)]);
    assert_eq!(
        names(&symbols[0].children[0].children),
        [("state", SymbolKind::Variable)]
    );
    assert!(symbols[1].children.is_empty());
    validate(source, &symbols, None);
}

#[test]
fn namespaces_group_compound_members_and_reassignment_does_not_duplicate_locals() {
    let source = "function A.B.C.first() end\nA.B.C.second = function() local value = 1 end\nA.B.C.value = 3\nlocal declared\ndeclared = 1\n";
    let symbols = document_symbols(source);
    assert_eq!(
        names(&symbols),
        [
            ("A", SymbolKind::Variable),
            ("declared", SymbolKind::Variable)
        ]
    );
    assert_eq!(names(&symbols[0].children), [("B", SymbolKind::Field)]);
    assert_eq!(
        names(&symbols[0].children[0].children),
        [("C", SymbolKind::Field)]
    );
    let members = &symbols[0].children[0].children[0].children;
    assert_eq!(
        names(members),
        [
            ("first", SymbolKind::Method),
            ("second", SymbolKind::Method),
            ("value", SymbolKind::Field)
        ]
    );
    assert_eq!(selected(source, &members[0]), "first");
    validate(source, &symbols, None);
}

#[test]
fn standalone_and_nested_annotation_symbols_keep_their_lexical_scope() {
    let source = "---@alias NumberAlias number\n\nlocal function make()\n ---@class Nested\n local value = {}\n function value:method() end\n ---@alias Inner string\nend\n---@class Standalone\n";
    let symbols = document_symbols(source);
    assert_eq!(
        names(&symbols),
        [
            ("NumberAlias", SymbolKind::Alias),
            ("make", SymbolKind::Function),
            ("Standalone", SymbolKind::Class)
        ]
    );
    assert_eq!(
        names(&symbols[1].children),
        [("Nested", SymbolKind::Class), ("Inner", SymbolKind::Alias)]
    );
    assert_eq!(
        names(&symbols[1].children[0].children),
        [("method", SymbolKind::Method)]
    );
    validate(source, &symbols, None);
}

#[test]
fn annotated_fields_and_runtime_implementations_share_one_member() {
    let source = "---@class Widget\n---@field run fun(value: number): number\nlocal widget = { run = function(value) return value end }\n";
    let symbols = document_symbols(source);
    assert_eq!(names(&symbols[0].children), [("run", SymbolKind::Method)]);
    validate(source, &symbols, None);
}

#[test]
fn long_strings_fold_at_each_supported_delimiter_depth_and_during_incomplete_typing() {
    for source in [
        "local value = [[one\ntwo]]\n",
        "local value = [=[one\ntwo]=]\n",
        "consume [==[one\ntwo]==]\n",
        "local value = [==[unfinished\nstill typing",
        "local value = 'escaped\\\nnewline'\n",
    ] {
        let strings: Vec<_> = folding_ranges(source)
            .into_iter()
            .filter(|fold| {
                let text = &source[fold.start..fold.end];
                text.starts_with('[') || text.starts_with('\'')
            })
            .collect();
        assert_eq!(strings.len(), 1, "{source:?}: {strings:?}");
        assert_eq!(strings[0].kind, None);
        assert!(
            !strings[0].keep_last_line_visible,
            "strings fold through their closing line"
        );
        assert!(source[strings[0].start..strings[0].end].contains('\n'));
    }
}

#[test]
fn whitespace_regions_nest_and_do_not_recognize_string_contents_or_word_prefixes() {
    let source = "-- {{{ outer\n-- #region inner\nlocal value = [[\n-- }}} fake\n-- endregion fake\n]]\n-- #endregion inner\n-- }}} outer\n-- regional word\nlocal value = 1\n-- endregional word\n";
    let regions: Vec<_> = folding_ranges(source)
        .into_iter()
        .filter(|fold| fold.kind == Some(FoldKind::Region))
        .map(|fold| &source[fold.start..fold.end])
        .collect();
    assert_eq!(regions.len(), 2);
    assert!(regions[0].starts_with("-- {{{ outer\n"));
    assert!(regions[0].ends_with("-- }}} outer"));
    assert!(regions[1].starts_with("-- #region inner\n"));
    assert!(regions[1].ends_with("-- #endregion inner"));
}

#[test]
fn comment_runs_stop_at_region_boundaries_and_accept_each_lua_newline() {
    for newline in ["\n", "\r\n", "\r"] {
        let source = [
            "-- before one",
            "-- before two",
            "-- {{{ title",
            "-- inside one",
            "-- inside two",
            "-- }}}",
            "-- after one",
            "-- after two",
        ]
        .join(newline);
        let comments: Vec<_> = folding_ranges(&source)
            .into_iter()
            .filter(|fold| fold.kind == Some(FoldKind::Comment))
            .map(|fold| source[fold.start..fold.end].to_owned())
            .collect();
        assert_eq!(
            comments,
            [
                format!("-- before one{newline}-- before two"),
                format!("-- inside one{newline}-- inside two"),
                format!("-- after one{newline}-- after two"),
            ]
        );
    }
}

#[test]
fn fake_long_comment_prefixes_and_single_line_strings_do_not_create_invalid_folds() {
    let source = "--[ ordinary line comment\n-- a second line\nlocal s = [[one line]]\n-- {{{ unmatched\nlocal n = 1\n";
    let folds = folding_ranges(source);
    assert_eq!(folds.len(), 1);
    assert_eq!(folds[0].kind, Some(FoldKind::Comment));
    assert_eq!(
        &source[folds[0].start..folds[0].end],
        "--[ ordinary line comment\n-- a second line"
    );
}

#[test]
fn sibling_initializers_and_explicit_self_parameters_use_lua_lexical_scope() {
    let source = "---@class Outer\nlocal object = {}\nlocal function factory()\n local object, setup = {}, function() object.old = 1 end\n function object:method(self) self.hidden = 1 end\nend\nfunction object:method() self.visible = 1 end\n";
    let symbols = document_symbols(source);
    assert_eq!(
        names(&symbols[0].children),
        [
            ("old", SymbolKind::Field),
            ("method", SymbolKind::Method),
            ("visible", SymbolKind::Field)
        ]
    );
    let inner_object = &symbols[1].children[0];
    assert_eq!(
        names(&inner_object.children),
        [("method", SymbolKind::Method)]
    );
    assert!(inner_object.children[0].children.is_empty());
    validate(source, &symbols, None);
}

#[test]
fn parentheses_do_not_hide_named_function_and_table_initializers() {
    let source = "local callback = (function(value) local result = value end)\nlocal config = ({ run = function() end })\n";
    let symbols = document_symbols(source);
    assert_eq!(
        names(&symbols),
        [
            ("callback", SymbolKind::Function),
            ("config", SymbolKind::Variable)
        ]
    );
    assert_eq!(
        names(&symbols[0].children),
        [("result", SymbolKind::Variable)]
    );
    assert_eq!(names(&symbols[1].children), [("run", SymbolKind::Method)]);
    validate(source, &symbols, None);
}

#[test]
fn anonymous_callback_bodies_fold_their_control_flow_blocks() {
    let source =
        "consume(function()\n while ready do\n  if enabled then\n   consume()\n  end\n end\nend)\n";
    let starts: Vec<_> = folding_ranges(source)
        .into_iter()
        .filter(|fold| fold.kind.is_none())
        .map(|fold| source[fold.start..fold.end].lines().next().unwrap())
        .collect();
    assert_eq!(starts, ["function()", "while ready do", "if enabled then"]);
}

#[test]
fn existing_compact_region_spellings_keep_working() {
    for (start, end) in [
        ("--region title", "--endregion"),
        ("---region title", "---endregion"),
        ("--#region title", "--#endregion"),
        ("--{{{ title", "--}}}"),
    ] {
        let source = format!("{start}\nlocal n = 1\n{end}\n");
        let regions: Vec<_> = folding_ranges(&source)
            .into_iter()
            .filter(|fold| fold.kind == Some(FoldKind::Region))
            .collect();
        assert_eq!(regions.len(), 1, "{source:?}");
        assert_eq!(
            source[regions[0].start..regions[0].end].lines().last(),
            Some(end)
        );
    }
}

#[test]
fn literal_keyed_members_and_sequence_callbacks_have_useful_source_owned_structure() {
    let source = "local table = { ['run'] = function() local result = 1 end, [1] = 'first', function() local callback = 1 end }\ntable['later'] = function() end\n";
    let symbols = document_symbols(source);
    assert_eq!(names(&symbols), [("table", SymbolKind::Variable)]);
    assert_eq!(
        names(&symbols[0].children),
        [
            ("run", SymbolKind::Method),
            ("[1]", SymbolKind::Field),
            ("<anonymous>", SymbolKind::Function),
            ("later", SymbolKind::Method)
        ]
    );
    assert_eq!(selected(source, &symbols[0].children[0]), "'run'");
    assert_eq!(selected(source, &symbols[0].children[3]), "'later'");
    assert_eq!(
        names(&symbols[0].children[2].children),
        [("callback", SymbolKind::Variable)]
    );
    validate(source, &symbols, None);
}

#[test]
fn strings_fold_their_closing_line_while_code_blocks_keep_theirs_visible() {
    let source = "local text = [[first\nlast]]\nlocal function f()\n return 1\nend\n";
    let folds = folding_ranges(source);
    assert_eq!(folds.len(), 2);
    assert!(!folds[0].keep_last_line_visible);
    assert_eq!(&source[folds[0].start..folds[0].end], "[[first\nlast]]");
    assert!(folds[1].keep_last_line_visible);
    assert_eq!(
        &source[folds[1].start..folds[1].end],
        "local function f()\n return 1\nend"
    );
}
