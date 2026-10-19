//! Focused native editor regressions. The server stdio suite verifies the transport too.
use luanalysis::{incremental::ProjectWorkspace, language_service::{self, InlayHint, InlayOptions, SignatureSite}, project_check::{ProjectCheckOptions, static_require_requests}};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
use std::{collections::HashMap, fs, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn site(marked: &str) -> SignatureSite {
    let offset = marked.find('|').unwrap();
    language_service::signature_site(&marked.replacen('|', "", 1), offset).expect(marked)
}

fn inlays(text: &str, preferences: InlayOptions) -> Vec<InlayHint> {
    let root = std::env::temp_dir().join(format!("luanalysis inlay parity {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("unsaved.ttslua");
    fs::write(&path, "return nil\n").unwrap();
    let path = fs::canonicalize(path).unwrap();
    let resolver = Resolver::new(ProjectConfig::new([root.clone()])).unwrap();
    let overlays = HashMap::from([(path.clone(), text.to_owned())]);
    let project = Project::load_plan_with_overlays(&resolver, ProjectLoadPlan::new([path]), &overlays, static_require_requests);
    let file = project.entries[0];
    let mut hints = Vec::new();
    language_service::inlay_hints(project, ProjectCheckOptions::default(), &mut ProjectWorkspace::default(), file, 0..text.len(), preferences, |result| hints = result);
    fs::remove_dir_all(root).unwrap();
    hints
}

#[test]
fn quoted_and_long_string_arguments_keep_their_enclosing_call() {
    for marked in ["move(1, '|a')", "move(1, 'a|')", r#"move(1, 'a\\\'|, fake(1, 2)')"#, "move(1, [=[a,\n fake(|]=])", "move(1, [==[a ]=] fake(|]==])"] {
        let site = site(marked);
        assert_eq!((site.open, site.argument), (4, 1), "{marked}");
        assert_eq!(site.text, marked.replacen('|', "", 1), "a valid buffer is unchanged");
    }
    assert_eq!((site("move '|a'").open, site("move '|a'").argument), (5, 0));
    assert_eq!((site("outer(move [=[a|]=])").open, site("outer(move [=[a|]=])").argument), (11, 0));
    let nested = site("outer('text', move(1, 'a|, )'))");
    assert_eq!((nested.open, nested.argument), (18, 1));
    assert_eq!((site("outer(move(1, 'a'), '|b')").open, site("outer(move(1, 'a'), '|b')").argument), (5, 1));
}

#[test]
fn unfinished_strings_close_the_exact_delimiter_before_the_call() {
    for marked in ["move(1, 'a|", "move(1, 'a\\|", "move(1, [=[a,\n fake(|", "move(1, [==[a ]=] fake(|"] {
        let site = site(marked);
        assert_eq!((site.open, site.argument), (4, 1), "{marked}");
        let parsed = luanalysis_syntax::parse(&site.text);
        assert!(parsed.diagnostics.is_empty(), "{marked}: {} {:#?}", site.text, parsed.diagnostics);
    }
    for marked in ["move(1, -- text|", "move(1, --[[ text| ]])", "local function move(x, |label)"] {
        let offset = marked.find('|').unwrap();
        assert!(language_service::signature_site(&marked.replacen('|', "", 1), offset).is_none(), "{marked}");
    }
}

#[test]
fn optional_parameter_and_return_type_inlays_use_contextual_native_types() {
    let text = "---@param callback fun(value: number): string\nlocal function use(callback) return callback(1) end\nuse(function(value) return 'yes' end)\nlocal function inferred() return 1 end\nlocal result = inferred()\n";
    let hints = inlays(text, InlayOptions { parameter_types: true, return_types: true, ..InlayOptions::default() });
    let parameter = text.find("function(value)").unwrap() + "function(value".len();
    assert!(hints.iter().any(|hint| hint.offset == parameter && hint.label == ": number" && !hint.parameter), "{hints:#?}");
    let result = text.find("inferred()").unwrap() + "inferred()".len();
    assert!(hints.iter().any(|hint| hint.offset == result && hint.label == ": number" && !hint.parameter), "{hints:#?}");
    // A written parameter annotation should not be repeated beside its declaration.
    let written = text.find("use(callback)").unwrap() + "use(callback".len();
    assert!(!hints.iter().any(|hint| hint.offset == written), "{hints:#?}");
    let defaults = inlays(text, InlayOptions::default());
    assert!(!defaults.iter().any(|hint| hint.offset == parameter || hint.offset == result), "{defaults:#?}");
    let disabled = inlays(text, InlayOptions { variable_types: false, parameter_names: false, expression_parameters: false, parameter_types: false, return_types: false });
    assert!(disabled.is_empty(), "{disabled:#?}");
}

#[test]
fn expression_argument_names_are_opt_in_and_choose_the_applicable_overload() {
    let text = "---@overload fun(label: string, handler: fun(): number): number\n---@overload fun(amount: number, callback: fun(): number): number\nlocal function apply(value, fn) return fn() end\napply(1 + 2, function() return 3 end)\n";
    let defaults = inlays(text, InlayOptions::default());
    assert!(defaults.iter().all(|hint| !hint.parameter), "{defaults:#?}");
    let hints = inlays(text, InlayOptions { expression_parameters: true, ..InlayOptions::default() });
    assert!(hints.iter().any(|hint| hint.offset == text.find("1 + 2").unwrap() && hint.label == "amount:" && hint.parameter), "{hints:#?}");
    assert!(hints.iter().any(|hint| hint.offset == text.find("function() return 3").unwrap() && hint.label == "callback:" && hint.parameter), "{hints:#?}");
}
