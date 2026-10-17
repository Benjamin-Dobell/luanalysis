use luanalysis::bind::{BoundFile, BuiltinIntrinsic, OccurrenceKind, bind};

fn bound(source: &str) -> BoundFile {
    let syntax = luanalysis_syntax::parse(source);
    assert!(syntax.diagnostics.is_empty(), "{:#?}", syntax.diagnostics);
    bind(&syntax)
}

fn intrinsic(source: &str, graph: &BoundFile, needle: &str) -> Option<BuiltinIntrinsic> {
    graph.intrinsic_at(source.find(needle).unwrap())
}

#[test]
fn global_require_has_an_intrinsic_identity_without_a_local_binding() {
    let source = "local module = require('module')\nlocal saved = require";
    let graph = bound(source);
    let offset = source.find("require").unwrap();
    assert_eq!(
        graph.intrinsic_at(offset),
        Some(BuiltinIntrinsic::LuaRequire)
    );
    assert_eq!(
        graph.intrinsic_at(offset + 6),
        Some(BuiltinIntrinsic::LuaRequire)
    );
    assert_eq!(graph.intrinsic_at(offset + 7), None);
    let occurrence = graph.occurrence_at(offset).unwrap();
    assert_eq!(occurrence.kind, OccurrenceKind::Read);
    assert_eq!(occurrence.binding, None);
    assert_eq!(
        graph.intrinsic_at(source.rfind("require").unwrap()),
        Some(BuiltinIntrinsic::LuaRequire)
    );
    assert!(!graph.has_modified_module_environment());
}

#[test]
fn local_initializer_observes_the_outer_require_before_shadow_activation() {
    let source = "local require = require('outer')\nrequire('local')";
    let graph = bound(source);
    assert_eq!(intrinsic(source, &graph, "require ="), None);
    assert_eq!(
        intrinsic(source, &graph, "require('outer')"),
        Some(BuiltinIntrinsic::LuaRequire)
    );
    assert_eq!(intrinsic(source, &graph, "require('local')"), None);
    assert!(!graph.has_modified_module_environment());
}

#[test]
fn local_functions_parameters_loop_variables_and_captures_shadow_require() {
    for source in [
        "local function require() return require('recursive') end",
        "local function f(require) return function() return require('captured') end end",
        "for require = 1, 2 do require('numeric') end",
        "for require in iterator do require('generic') end",
        "local require = custom\nlocal function f() return require('captured') end",
    ] {
        let graph = bound(source);
        assert!(
            graph
                .occurrences
                .iter()
                .filter(|occurrence| occurrence.name == "require")
                .all(|occurrence| graph.intrinsic_at(occurrence.span.start).is_none()),
            "{source}"
        );
        assert!(!graph.has_modified_module_environment(), "{source}");
    }
}

#[test]
fn global_require_remains_available_in_an_unmodified_nested_function() {
    let source = "local function outer() return function() return require('nested') end end";
    let graph = bound(source);
    assert_eq!(
        intrinsic(source, &graph, "require('nested')"),
        Some(BuiltinIntrinsic::LuaRequire)
    );
}

#[test]
fn lexical_environments_redirect_global_reads_and_nested_captures() {
    let source = "local first = require('first')\ndo\nlocal _ENV = require('environment')\nlocal next = require('redirected')\nlocal function nested() return require('nested') end\nend\nlocal last = require('last')";
    let graph = bound(source);
    for needle in [
        "require('first')",
        "require('environment')",
        "require('last')",
    ] {
        assert_eq!(
            intrinsic(source, &graph, needle),
            Some(BuiltinIntrinsic::LuaRequire),
            "{needle}"
        );
    }
    for needle in ["require('redirected')", "require('nested')"] {
        assert_eq!(intrinsic(source, &graph, needle), None, "{needle}");
    }
    assert!(!graph.has_modified_module_environment());
    let parameter = "local function f(_ENV) return require('redirected') end";
    assert_eq!(
        intrinsic(parameter, &bound(parameter), "require('redirected')"),
        None
    );
}

#[test]
fn direct_global_require_and_environment_writes_invalidate_the_whole_file() {
    for mutation in [
        "require = custom",
        "function require() end",
        "_G.require = custom",
        "_G['require'] = custom",
        "_ENV.require = custom",
        "(_ENV)['require'] = custom",
        "((_G)).require = custom",
        "((_ENV))[('package')][('loaded')].module = custom",
        "_ENV = custom",
        "local function mutate() require = custom end",
        "local key = 'require'\n_G[key] = custom",
    ] {
        // A parenthesized next statement otherwise continues the previous call.
        let source = format!("require('before');\n{mutation}\nrequire('after')");
        let graph = bound(&source);
        assert!(graph.has_modified_module_environment(), "{mutation}");
        for needle in ["require('before')", "require('after')"] {
            assert_eq!(intrinsic(&source, &graph, needle), None, "{mutation}");
        }
    }
}

#[test]
fn a_parenthesized_call_continuation_does_not_become_an_environment_assignment() {
    let source = "require('before')\n(_ENV)['require'] = custom\nrequire('after')";
    let graph = bound(source);
    assert!(!graph.has_modified_module_environment());
    for needle in ["require('before')", "require('after')"] {
        assert_eq!(
            intrinsic(source, &graph, needle),
            Some(BuiltinIntrinsic::LuaRequire)
        );
        assert_eq!(
            graph
                .occurrence_at(source.find(needle).unwrap())
                .unwrap()
                .span
                .start,
            source.find(needle).unwrap()
        );
    }
    assert_eq!(intrinsic(source, &graph, "_ENV"), None);
}

#[test]
fn package_loader_mutations_invalidate_require_including_nested_or_computed_writes() {
    for mutation in [
        "package = {}",
        "package.loaded.module = {}",
        "package.preload['module'] = custom",
        "package.searchers[1] = custom",
        "package.loaders = {}",
        "package['lo\\097ded'].module = {}",
        "package[key] = custom",
        "_G.package.searchers = custom",
        "_ENV['package']['searchers'][1] = custom",
        "_G[key].path = custom",
    ] {
        let source = format!("require('before')\n{mutation}\nrequire('after')");
        let graph = bound(&source);
        assert!(graph.has_modified_module_environment(), "{mutation}");
        assert_eq!(
            intrinsic(&source, &graph, "require('before')"),
            None,
            "{mutation}"
        );
    }
}

#[test]
fn unmodified_loader_reads_unrelated_fields_and_shadowed_writes_preserve_identity() {
    for statement in [
        "local path = package.path",
        // Search paths are configuration that the project's package path describes.
        "package.path = 'custom'",
        "package.cpath = 'custom'",
        "_G.package.path = custom",
        "local searcher = package.searchers[1]",
        "package.config = 'custom'",
        "local package = {}\npackage.path = 'local'",
        "local function f(package) package.loaded.module = {} end",
        "local _G = {}\n_G.require = custom",
        "do local _ENV = {}\nrequire = custom\npackage.path = custom end",
        "do local require = custom\nrequire = other end",
    ] {
        let source = format!("{statement}\nrequire('standard')");
        let graph = bound(&source);
        assert!(!graph.has_modified_module_environment(), "{statement}");
        assert_eq!(
            intrinsic(&source, &graph, "require('standard')"),
            Some(BuiltinIntrinsic::LuaRequire),
            "{statement}"
        );
    }
}

#[test]
fn explicit_standard_mutator_calls_invalidate_the_module_environment() {
    for mutation in [
        "rawset(_G, 'require', custom)",
        "_G.rawset(_ENV, 'require', custom)",
        "rawset(package, key, custom)",
        "rawset(package.loaded, 'module', {})",
        "table.insert(package.searchers, custom)",
        "table.remove(package.loaders, 1)",
        "table.move(custom, 1, 2, 1, package.searchers)",
        "_ENV.table.move(package.searchers, 1, 2, 1)",
    ] {
        let source = format!("{mutation}\nrequire('module')");
        let graph = bound(&source);
        assert!(graph.has_modified_module_environment(), "{mutation}");
        assert_eq!(
            intrinsic(&source, &graph, "require('module')"),
            None,
            "{mutation}"
        );
    }
}

#[test]
fn shadowed_mutators_and_unrelated_mutator_targets_do_not_invalidate_require() {
    for mutation in [
        "local rawset = custom\nrawset(package, 'loaded', {})",
        "local table = custom\ntable.insert(package.searchers, custom)",
        "local package = {}\nrawset(package, 'loaded', {})",
        "local _G = {}\nrawset(_G, 'require', custom)",
        "rawset(unrelated, 'require', custom)",
        "table.insert(unrelated, custom)",
        "table.move(package.searchers, 1, 2, 1, unrelated)",
    ] {
        let source = format!("{mutation}\nrequire('module')");
        let graph = bound(&source);
        assert!(!graph.has_modified_module_environment(), "{mutation}");
        assert_eq!(
            intrinsic(&source, &graph, "require('module')"),
            Some(BuiltinIntrinsic::LuaRequire),
            "{mutation}"
        );
    }
}

#[test]
fn comments_strings_and_member_names_are_not_global_require_occurrences() {
    let source = "-- require = custom\nlocal text = 'package.path = custom; require(\"fake\")'\nlocal object = {require = custom}\nobject.require('member')\nobject:require('method')\nrequire('actual')";
    let graph = bound(source);
    assert!(!graph.has_modified_module_environment());
    for needle in [
        "require = custom",
        "require(\"fake\")",
        "require('member')",
        "require('method')",
    ] {
        assert_eq!(intrinsic(source, &graph, needle), None, "{needle}");
    }
    assert_eq!(
        intrinsic(source, &graph, "require('actual')"),
        Some(BuiltinIntrinsic::LuaRequire)
    );
}
