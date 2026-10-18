use luanalysis::{CheckReport, check};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}
fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .expect(name)
        .ty
}
fn rejects_at(source: &str, code: &str, snippet: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == code
                && &source[diagnostic.span.start..diagnostic.span.end] == snippet
        }),
        "expected {code} at {snippet:?}\n{source}\n{:#?}",
        report.diagnostics
    );
    report
}

#[test]
fn structural_shapes_inherit_required_fields_and_initializer_context() {
    let source = r#"---@shape Base
---@field value number

---@shape Derived:Base
---@field label string

---@type Derived
local value={value=42,label="text"}
local inherited=value.value
local own=value.label
"#;
    let report = clean(source);
    assert_eq!(ty(&report, "inherited"), "number");
    assert_eq!(ty(&report, "own"), "string");
    rejects_at(
        &source.replace("value=42,", ""),
        "TS2322",
        "{label=\"text\"}",
    );
    rejects_at(
        &source.replace("value=42", "value=\"wrong\""),
        "TS2322",
        "{value=\"wrong\",label=\"text\"}",
    );
}

#[test]
fn inherited_generic_members_compose_mappers_across_three_levels() {
    let report = clean(
        r#"---@shape Box<T>
---@field value T

---@class Base<T>
---@field inherited T

---@class Middle<U>:Base<Box<U>>
---@field middle U

---@class Leaf<V>:Middle<V>
---@field leaf V

---@param value Leaf<number>
local function read(value)
local inherited=value.inherited.value
local middle=value.middle
local leaf=value.leaf
end
"#,
    );
    for name in ["inherited", "middle", "leaf"] {
        assert_eq!(ty(&report, name), "number");
    }
}

#[test]
fn nominal_descendants_satisfy_mapped_ancestors_but_ancestors_do_not_become_descendants() {
    clean(
        r#"---@class Base<T>
---@field value T

---@alias InheritedBase<T> Base<T>

---@class Child<U>:InheritedBase<U>

---@class Leaf:Child<number>

---@alias AliasBase Base<number>

---@param value Leaf
---@return AliasBase
local function upcast(value) return value end
"#,
    );
    let wrong = "---@class Base\n\n---@class Child:Base\n\n---@param value Base\n---@return Child\nlocal function downcast(value) return value end";
    rejects_at(wrong, "TS2322", "value");
    let different = wrong.replace(
        "---@param value Base",
        "---@class Other\n\n---@param value Other",
    );
    rejects_at(&different, "TS2322", "value");
}

#[test]
fn generic_ancestor_relations_still_check_inherited_member_types() {
    let source = "---@class Base<T>\n---@field value T\n\n---@class Child<U>:Base<U>\n\n---@param value Child<number>\n---@return Base<string>\nlocal function wrong(value) return value end";
    rejects_at(source, "TS2322", "value");
    clean(&source.replace("Base<string>", "Base<number>"));
}

#[test]
fn classes_can_extend_generic_shapes_with_numeric_and_named_lua_keys() {
    let report = clean(
        r#"---@shape VectorShape<T>
---@field x T
---@field [1] T

---@class Vector:VectorShape<number>

---@param vector Vector
---@return VectorShape<number>
local function asShape(vector)
local named=vector.x
local numeric=vector[1]
return vector
end
"#,
    );
    assert_eq!(ty(&report, "named"), "number");
    assert_eq!(ty(&report, "numeric"), "number");
    let wrong = "---@shape VectorShape<T>\n---@field x T\n\n---@class Vector:VectorShape<number>\n\n---@param value VectorShape<number>\n---@return Vector\nlocal function wrong(value) return value end";
    rejects_at(wrong, "TS2322", "value");
}

#[test]
fn structural_inheritance_does_not_grant_a_nominal_class_origin() {
    let source = "---@class Base\n---@field value number\n\n---@shape Derived:Base\n\n---@param value Derived\n---@return Base\nlocal function wrong(value) return value end";
    rejects_at(source, "TS2322", "value");
}

#[test]
fn transparent_record_and_generic_alias_bases_keep_their_original_member_mappers() {
    let report = clean(
        r#"---@alias Record<T> {value:T}
---@alias Alias<T> Record<T>

---@shape Derived:Alias<"literal">

---@type Derived
local value={value="literal"}
local inherited=value.value
---@type "literal"
local exact=value.value
"#,
    );
    assert_eq!(ty(&report, "inherited"), "string");
    assert_eq!(ty(&report, "exact"), "\"literal\"");
    rejects_at(
        "---@alias Record<T> {value:T}\n---@alias Alias<T> Record<T>\n\n---@shape Derived:Alias<\"literal\">\n\n---@param value Derived\nlocal function wrong(value) value.value=\"other\" end",
        "TS2322",
        "\"other\"",
    );
    let wrong = "---@alias Record<T> {value:T}\n\n---@shape Derived:Record<number>\n\n---@param value Derived\nlocal function wrong(value) value.value=\"wrong\" end";
    rejects_at(wrong, "TS2322", "\"wrong\"");
}

#[test]
fn own_field_overrides_use_native_assignability_and_original_annotation_ranges() {
    let wrong = "---@shape Base\n---@field value number\n\n---@shape Derived:Base\n---@field value string\n";
    rejects_at(wrong, "TS2416", "string");
    clean(&wrong.replace("---@field value number", "---@field value number|string"));
    let inherited = clean(
        "---@shape Base\n---@field value number|string\n\n---@shape Middle:Base\n---@field value string\n\n---@shape Leaf:Middle\n\n---@param leaf Leaf\nlocal function read(leaf) local inherited=leaf.value end",
    );
    assert_eq!(ty(&inherited, "inherited"), "string");
}

#[test]
fn inherited_generic_indexers_retain_their_key_and_value_mappers() {
    let report = clean(
        "---@shape Dictionary<K,V>\n---@field [K] V\n\n---@shape Derived<T>:Dictionary<string,T>\n\n---@param value Derived<number>\nlocal function read(value) local inherited=value.answer end",
    );
    assert_eq!(ty(&report, "inherited"), "number");
}

#[test]
fn own_indexer_overrides_must_satisfy_the_inherited_indexer_contract() {
    let wrong = "---@shape Base\n---@field [string] number\n\n---@shape Derived:Base\n---@field [string] string\n";
    rejects_at(wrong, "TS2416", "string");
    clean(&wrong.replace(
        "---@field [string] number",
        "---@field [string] number|string",
    ));
}

#[test]
fn a_valid_indexer_override_remains_effective_in_the_grandchild() {
    let source = "---@shape Base\n---@field [string] number|string\n\n---@shape Middle:Base\n---@field [string] string\n\n---@shape Leaf:Middle\n\n---@param leaf Leaf\nlocal function read(leaf) local inherited=leaf.answer end";
    let report = clean(source);
    assert_eq!(ty(&report, "inherited"), "string");
    rejects_at(
        &format!("{source}\n---@param leaf Leaf\nlocal function wrong(leaf) leaf.answer=42 end"),
        "TS2322",
        "42",
    );
}

#[test]
fn inherited_partial_indexer_overlaps_keep_the_overlap_and_both_outer_domains() {
    let source = "---@shape Base\n---@field ['a'|'b'] number|string\n\n---@shape Middle:Base\n---@field ['b'|'c'] string\n\n---@shape Leaf:Middle\n\n---@param leaf Leaf\nlocal function read(leaf)\nlocal broad=leaf.a\nlocal overlap=leaf.b\nlocal extra=leaf.c\nleaf.a=42\nleaf.b=\"text\"\nleaf.c=\"text\"\nend";
    let report = clean(source);
    assert!(ty(&report, "broad").contains("number") && ty(&report, "broad").contains("string"));
    assert_eq!(ty(&report, "overlap"), "string");
    assert_eq!(ty(&report, "extra"), "string");
    rejects_at(
        &source.replace("leaf.b=\"text\"", "leaf.b=42"),
        "TS2322",
        "42",
    );
    rejects_at(
        &source.replace("leaf.c=\"text\"", "leaf.c=42"),
        "TS2322",
        "42",
    );
}

#[test]
fn own_fixed_fields_must_satisfy_applicable_inherited_indexers() {
    let wrong = "---@shape Base\n---@field [string] number\n\n---@shape Derived:Base\n---@field value string\n";
    rejects_at(wrong, "TS2416", "string");
    clean(&wrong.replace("---@field value string", "---@field value number"));
}

#[test]
fn inherited_function_overrides_use_signature_variance_and_result_pack_relations() {
    let wrong = "---@class Base\n---@field method fun(value:number|string):number\n\n---@class Derived:Base\n---@field method fun(value:number):number\n";
    rejects_at(wrong, "TS2416", "fun(value:number):number");
    clean(
        "---@class Base\n---@field method fun(value:number):number\n\n---@class Derived:Base\n---@field method fun(value:number|string):number\n",
    );
    let packs = "---@shape Base\n---@field method fun():number,boolean\n\n---@shape Derived:Base\n---@field method fun():string,boolean\n";
    rejects_at(packs, "TS2416", "fun():string,boolean");
}

#[test]
fn inherited_call_heads_retain_original_signature_scopes_and_generic_mappers() {
    let report = clean(
        r#"---@shape Base<T>
---@overload fun(value:T):T

---@shape Derived<U>:Base<U>
---@overload fun(value:boolean):boolean

---@param callable Derived<number>
local function read(callable)
local inherited=callable(42)
local own=callable(true)
end
"#,
    );
    assert_eq!(ty(&report, "inherited"), "number");
    assert_eq!(ty(&report, "own"), "boolean");
}

#[test]
fn productive_recursive_member_types_are_not_base_cycles() {
    let report = clean(
        r#"---@shape Node
---@field value number
---@field next Node|nil

---@shape Tagged:Node
---@field tag string

---@param node Tagged
local function read(node)
if node.next then local value=node.next.value end
end
"#,
    );
    assert_eq!(ty(&report, "value"), "number");
}

#[test]
fn direct_indirect_alias_and_growing_generic_base_cycles_are_bounded() {
    for (source, snippets) in [
        ("---@class Self:Self\n", vec!["Self"]),
        ("---@class A:B\n\n---@class B:A\n", vec!["A", "B"]),
        ("---@alias Alias A\n\n---@class A:Alias\n", vec!["Alias"]),
        ("---@class Growing<T>:Growing<T[]>\n", vec!["Growing<T[]>"]),
    ] {
        let report = check(source);
        assert!(
            report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "TS2310"
                    && snippets.contains(&&source[diagnostic.span.start..diagnostic.span.end])
            }),
            "{source}\n{:#?}",
            report.diagnostics
        );
        assert!(report.diagnostics.len() < 10, "{:#?}", report.diagnostics);
    }
}

#[test]
fn invalid_base_categories_and_unknown_names_have_original_base_spans() {
    for base in [
        "number",
        "string",
        "function",
        "table",
        "unknown",
        "number|string",
        "number[]",
        "table<string,number>",
    ] {
        let source = format!("---@class Invalid:{base}\n");
        rejects_at(&source, "TS2312", base);
    }
    let missing = rejects_at("---@shape Invalid:Missing\n", "TS2304", "Missing");
    assert_eq!(missing.diagnostics.len(), 1, "{:#?}", missing.diagnostics);
    rejects_at("---@class Invalid<T>:T\n", "TS2312", "T");
}

#[test]
fn named_base_arguments_still_check_native_constraints_and_arity_at_the_application() {
    let prefix = "---@class Base<T:number>\n\n";
    let wrong_argument = rejects_at(
        &format!("{prefix}---@class Invalid:Base<string>\n"),
        "TS2344",
        "string",
    );
    assert_eq!(
        wrong_argument.diagnostics.len(),
        1,
        "{:#?}",
        wrong_argument.diagnostics
    );
    let wrong_arity = rejects_at(
        &format!("{prefix}---@class Invalid:Base<number,string>\n"),
        "TS2314",
        "Base<number,string>",
    );
    assert_eq!(
        wrong_arity.diagnostics.len(),
        1,
        "{:#?}",
        wrong_arity.diagnostics
    );
    let unconstrained_argument = rejects_at(
        &format!("{prefix}---@class Invalid<T>:Base<T>\n"),
        "TS2344",
        "T",
    );
    assert_eq!(
        unconstrained_argument.diagnostics.len(),
        1,
        "{:#?}",
        unconstrained_argument.diagnostics
    );
    clean(&format!("{prefix}---@class Valid<T:number>:Base<T>\n"));
}

#[test]
fn partial_class_definitions_do_not_fabricate_initialized_inherited_members() {
    let source = "---@class Base\n---@field inherited number\n\n---@class Derived:Base\n---@field own string\nlocal definition={own=\"text\"}\nlocal before=definition.inherited\ndefinition.inherited=42\nlocal after=definition.inherited";
    // An inherited member not yet given reads as the class declares it.
    let report = clean(source);
    assert_eq!(ty(&report, "before"), "number");
    assert_eq!(ty(&report, "after"), "number");
    rejects_at(
        &source.replace("definition.inherited=42", "definition.inherited=\"wrong\""),
        "TS2322",
        "\"wrong\"",
    );
    rejects_at(
        &source.replace("definition.inherited=42", "definition.inherited=nil"),
        "TS2322",
        "nil",
    );
}

#[test]
fn returned_definition_inherited_members_retain_outer_function_type_parameters() {
    let report = clean(
        r#"---@shape Base<T>
---@field value T

---@generic T
---@param value T
local function make(value)
---@class Derived:Base<T>
local definition={value=value}
return definition
end
local number=make(42).value
local string=make("text").value
"#,
    );
    assert_eq!(ty(&report, "number"), "number");
    assert_eq!(ty(&report, "string"), "string");
}

#[test]
fn complete_definition_values_satisfy_ancestors_while_partial_and_unrelated_values_do_not() {
    let source = "---@class Base\n---@field value number\n\n---@class Child:Base\nlocal child={}\n---@param value Base\nlocal function consume(value) end\nconsume(child)";
    // A class table is of its class, and so of its ancestors, before it has every member.
    clean(source);
    clean(&source.replace("local child={}", "local child={value=42}"));
    let unrelated = source
        .replace(
            "---@class Child:Base",
            "---@class Child\n---@field value number",
        )
        .replace("local child={}", "local child={value=42}");
    rejects_at(&unrelated, "TS2345", "child");
}

#[test]
fn private_and_protected_members_are_inherited() {
    // As in Luanalysis for IntelliJ, visibility chooses completions; it does not hide members.
    for visibility in ["private", "protected", "package"] {
        let source = format!(
            "---@class Base\n---@field {visibility} value number\n\n---@class Child:Base\n\n---@param child Child\n---@return number\nlocal function read(child) return child.value end\n"
        );
        let report = check(&source);
        assert!(report.diagnostics.is_empty(), "{visibility}: {:#?}", report.diagnostics);
    }
}

#[test]
fn project_inheritance_keeps_foreign_base_mappers_and_original_override_ownership() {
    use luanalysis::project_check::check_project;
    use luanalysis_project::{
        Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole,
    };
    use std::fs;
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-inheritance-project-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let library = directory.join("base.def.lua");
    let entry = directory.join("entry.ttslua");
    fs::write(&library, "---@class Base<T>\n---@field value T\n").unwrap();
    let run = |source: &str| {
        fs::write(&entry, source).unwrap();
        let resolver = Resolver::new(ProjectConfig::new([directory.clone()])).unwrap();
        let mut plan = ProjectLoadPlan::new(vec![entry.clone()]);
        plan.libraries = vec![ProjectInput::new(library.clone(), SourceRole::Declaration)];
        check_project(Project::load_plan(&resolver, plan, |_| vec![]))
    };
    let report = run(
        "---@class Child<U>:Base<U>\n\n---@param child Child<number>\nlocal function read(child) local inherited=child.value end",
    );
    for file in &report.files {
        assert!(
            file.report.diagnostics.is_empty(),
            "{:#?}",
            file.report.diagnostics
        );
    }
    assert!(
        report
            .files
            .iter()
            .flat_map(|file| &file.report.bindings)
            .any(|binding| binding.name == "inherited" && binding.ty == "number")
    );
    let wrong = "---@class Child:Base<number>\n---@field value string\n";
    let report = run(wrong);
    let canonical_entry = fs::canonicalize(&entry).unwrap();
    let entry_report = report.files.iter().find(|file| {
        report
            .source(file.file)
            .is_some_and(|source| source.path == canonical_entry)
    });
    let diagnostics = &entry_report.expect("entry report").report.diagnostics;
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2416"
                && &wrong[diagnostic.span.start..diagnostic.span.end] == "string"),
        "{diagnostics:#?}"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn actual_cli_reports_the_inherited_field_at_original_source_then_accepts_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis-inheritance-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("inherited.ttslua");
    let source = "---@shape Base\n---@field value number\n\n---@shape Child:Base\n\n---@type Child\nlocal child={value=42}\nchild.value=\"wrong\"\n";
    let run = |source: &str| {
        fs::write(&path, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--dump-types"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let rejected = run(source);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("inherited.ttslua:8:13: error[TS2322]"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    let accepted = run(&source.replace("\"wrong\"", "84"));
    assert_eq!(
        accepted.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}
