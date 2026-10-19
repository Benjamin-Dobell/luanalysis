//! The same overlay/native-session action calculation as the LSP query worker. Assertions apply
//! the returned original-source edits and parse/check the resulting current buffer.
use luanalysis::incremental::ProjectWorkspace;
use luanalysis::project_check::ProjectCheckOptions;
use luanalysis::semantic_actions::{self, Action};
use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};

fn actions(text: &str, needle: &str) -> Vec<Action> {
    let root = std::env::temp_dir().join(format!("luanalysis semantic actions {}", std::process::id()));
    let path = root.join("unsaved.ttslua");
    let overlays = std::collections::HashMap::from([(path.clone(), text.to_owned())]);
    let plan = ProjectLoadPlan { entries: vec![path.clone()], libraries: Vec::new() };
    let resolver = Resolver::new(ProjectConfig::new([root])).unwrap();
    let mut workspace = ProjectWorkspace::new();
    let project = Project::load_plan_with_overlays(&resolver, plan, &overlays, |source| workspace.static_require_requests(source));
    let file = project.file_by_path(&path).unwrap().id;
    let offset = text.find(needle).expect("a selected site");
    let mut result = None;
    semantic_actions::actions(project, ProjectCheckOptions::default(), &mut workspace, file, offset..offset, |actions| result = Some(actions));
    result.expect("an action response")
}

fn apply(text: &str, action: &Action) -> String {
    assert_eq!(action.kind, "refactor.rewrite");
    let mut text = text.to_owned();
    let mut edits = action.edits.clone();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));
    for edit in edits { text.replace_range(edit.span.start..edit.span.end, &edit.replacement); }
    assert!(luanalysis_syntax::parse(&text).diagnostics.is_empty(), "{text}");
    text
}

fn rewrite(text: &str, needle: &str, title: &str) -> String {
    let action = actions(text, needle).into_iter().find(|action| action.title == title).unwrap_or_else(|| panic!("missing {title} at {needle} in {text}"));
    apply(text, &action)
}

#[test]
fn documentation_uses_native_parameter_and_result_types_and_does_not_repeat() {
    let text = "local function greet(name)\n    return 'hello'\nend\n";
    let generated = rewrite(text, "greet", "Create LuaDoc");
    assert!(generated.starts_with("--- greet\n---@param name"), "{generated}");
    assert!(generated.contains(" any\n"), "{generated}");
    assert!(generated.contains("---@return "), "{generated}");
    assert!(!generated.contains("---@return table"), "{generated}");
    assert!(!actions(&generated, "local function").iter().any(|action| action.title == "Create LuaDoc" || action.title == "Create return annotation"));
    assert!(luanalysis::check(&generated).diagnostics.is_empty(), "{generated}");
}

#[test]
fn individual_parameter_and_return_annotations_preserve_existing_documentation() {
    let text = "--- A description.\n---@param x number # first value\nlocal function choose(x, label)\n    return x, label\nend\n";
    let generated = rewrite(text, "label)", "Create parameter annotation");
    assert!(generated.contains("--- A description.\n---@param x number # first value\n---@param label"), "{generated}");
    assert_eq!(generated.matches("---@param x").count(), 1);
    assert!(!actions(&generated, "label)").iter().any(|action| action.title == "Create parameter annotation"));
    let generated = rewrite(&generated, "choose(", "Create return annotation");
    assert_eq!(generated.matches("---@return").count(), 1);
    assert!(luanalysis::check(&generated).diagnostics.is_empty(), "{generated}");
}

#[test]
fn local_annotations_describe_all_native_constraints_without_freezing_evolving_nil() {
    let text = "local amount, label = 42, 'ok'\nreturn amount, label\n";
    let generated = rewrite(text, "amount", "Create type annotation");
    assert!(generated.starts_with("---@type number, string\n"), "{generated}");
    assert!(luanalysis::check(&generated).diagnostics.is_empty());
    assert!(!actions(&generated, "local amount").iter().any(|action| action.title == "Create type annotation"));
    for text in ["local amount\namount = 1\n", "local amount = nil\namount = 1\n", "local amount = (nil)\namount = 1\n"] {
        assert!(!actions(text, "amount").iter().any(|action| action.title == "Create type annotation"));
    }
}

#[test]
fn annotation_generation_preserves_crlf_indentation_and_blank_line_attachment() {
    let text = "do\r\n\tlocal function answer() return 42 end\r\nend\r\n";
    let generated = rewrite(text, "answer", "Create LuaDoc");
    assert!(generated.contains("\r\n\t--- answer\r\n\t---@return "), "{generated}");
    assert!(!generated.replace("\r\n", "").contains('\n'));
    let text = "--- A separate comment.\n\nlocal function answer() return 42 end\n";
    assert!(actions(text, "answer").iter().any(|action| action.title == "Create LuaDoc"));
    assert!(!actions("do local function answer() end end", "answer").iter().any(|action| action.title == "Create LuaDoc"));
}

#[test]
fn optional_parameters_keep_their_type_and_description() {
    let text = "---@param label string # a label\nlocal function greet(label) end\n";
    let generated = rewrite(text, "label string", "Make parameter optional");
    assert_eq!(generated, "---@param label? string # a label\nlocal function greet(label) end\n");
    assert!(!actions(&generated, "label? string").iter().any(|action| action.title == "Make parameter optional"));
    let text = "---@alias Callback fun(value: number): number\n";
    let generated = rewrite(text, "value:", "Make parameter optional");
    assert_eq!(generated, "---@alias Callback fun(value?: number): number\n");
}

#[test]
fn parentheses_rewrites_preserve_long_strings_comments_and_nested_calls() {
    let text = "local result = outer(inner -- keep\n [=[text ) -- not a comment\nmore]=])\n";
    let generated = rewrite(text, "inner", "Add call parentheses");
    assert_eq!(generated, "local result = outer(inner -- keep\n ([=[text ) -- not a comment\nmore]=]))\n");
    let restored = rewrite(&generated, "inner", "Remove call parentheses");
    assert!(restored.contains("inner -- keep\n  [=[text ) -- not a comment\nmore]=]"), "{restored}");
    let text = "consume(-- before\n {value = 1} -- after\n)\n";
    let generated = rewrite(text, "consume", "Remove call parentheses");
    assert_eq!(generated, "consume -- before\n {value = 1} -- after\n\n");
    assert!(!actions("consume(1)\n", "consume").iter().any(|action| action.title == "Remove call parentheses"));
    assert!(!actions("consume(('hello'))\n", "consume").iter().any(|action| action.title == "Remove call parentheses"));
}

#[test]
fn boolean_and_parameter_field_intentions_are_bounded_and_repeat_safe() {
    let text = "local enabled = true -- keep\n";
    let generated = rewrite(text, "true", "Invert boolean value");
    assert_eq!(generated, "local enabled = false -- keep\n");
    let text = "local Object = {}\nfunction Object:set(value)end\n";
    let generated = rewrite(text, "value)", "Create field for parameter");
    assert!(generated.contains("self.value = value\n"), "{generated}");
    assert!(!actions(&generated, "value)").iter().any(|action| action.title == "Create field for parameter"));
    assert!(!actions("local function f(value) end\n", "value").iter().any(|action| action.title == "Create field for parameter"));
}

#[test]
fn invalid_current_buffers_and_nonmatching_sites_do_not_offer_speculative_rewrites() {
    assert!(actions("local function broken(value\n", "value").is_empty());
    assert!(actions("local function f()\n    return 1\nend\n", "return").is_empty());
    let text = "local description = 'local function fake(value) end'\n";
    assert!(!actions(text, "fake").iter().any(|action| action.title == "Create LuaDoc"));
}

#[test]
fn constant_actions_use_native_values_only_when_evaluation_has_no_effects_or_numeric_loss() {
    let text = "local text = false or (true and 'a\\2552')\n";
    let generated = rewrite(text, "false", "Compute constant value");
    assert!(generated.contains("\"a\\2552\""), "{generated}");
    assert!(luanalysis::check(&generated).diagnostics.is_empty());
    for (text, needle) in [
        ("local value = false and print('side effect')\n", "false"),
        ("local value = true and 9223372036854775807\n", "true"),
        ("local value = false or (1 // 0)\n", "false"),
        ("local value = false -- keep this comment\n or true\n", "false"),
    ] {
        assert!(!actions(text, needle).iter().any(|action| action.title == "Compute constant value"), "{text}");
    }
}
