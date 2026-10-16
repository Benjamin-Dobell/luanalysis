use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis definition interactions {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn check(&self, libraries: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis"));
        command.current_dir(&self.0).args(["check", "--dump-types"]);
        for library in libraries {
            command.args(["--library", library]);
        }
        command.arg("main.lua").output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn pass(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
}
fn fail(output: &Output, code: &str, owner: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    let text = text(output);
    assert!(text.contains(code) && text.contains(owner), "{text}");
}
const FACTORY: &str =
    "---@class Factory\n---@overload fun(value:number):number\nlocal factory={}\nreturn factory\n";

#[test]
fn required_definition_keeps_its_original_heads_and_runtime_table_domain() {
    let fixture = Fixture::new();
    fixture.file("factory.lua", FACTORY);
    fixture.file("main.lua", "local factory=require('factory')\nlocal result=factory(42)\n---@type number\nlocal accepted=result\n---@type table\nlocal as_table=factory\nlocal length=#factory\n");
    pass(&fixture.check(&[]));
    fixture.file(
        "main.lua",
        "local factory=require('factory')\nlocal rejected=factory('wrong')\n",
    );
    fail(&fixture.check(&[]), "TS2345", "main.lua:2:");
    fixture.file(
        "main.lua",
        "local factory=require('factory')\n---@type function\nlocal rejected=factory\n",
    );
    fail(&fixture.check(&[]), "TS2322", "main.lua:3:");
    fixture.file("main.lua", "local factory=require('factory')\n---@type table\nlocal corrected=factory\nlocal result=factory(42)\n");
    pass(&fixture.check(&[]));
}

#[test]
fn ambient_complete_class_contracts_and_checked_partial_definitions_keep_separate_roles() {
    let fixture = Fixture::new();
    fixture.file(
        "api.def.lua",
        "---@class External\n---@field count number\nexternal={}\n",
    );
    let valid = "---@class Local\n---@field count number\nlocal definition={}\n---@type number\nlocal trusted=external.count\n---@type number|nil\nlocal absent=definition.count\ndefinition.count=42\n---@type number\nlocal present=definition.count\n";
    fixture.file("main.lua", valid);
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file(
        "main.lua",
        &format!("{valid}definition.count='wrong'\nexternal.count='wrong'\n"),
    );
    let invalid = fixture.check(&["api.def.lua"]);
    fail(&invalid, "TS2322", "main.lua:11:");
    assert!(
        text(&invalid).contains("main.lua:12:"),
        "{}",
        text(&invalid)
    );
    assert!(
        !text(&invalid).contains("api.def.lua:"),
        "{}",
        text(&invalid)
    );
    fixture.file("main.lua", valid);
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn module_definitions_require_actual_members_and_original_nominal_origin_for_instances() {
    let fixture = Fixture::new();
    let definition = "---@class Point\n---@field x number\nlocal value={}\nreturn value\n";
    fixture.file("factory.lua", definition);
    fixture.file("main.lua", "local value=require('factory')\n---@param point Point\n---@return number\nlocal function read(point) return point.x end\nlocal result=read(value)\n");
    fail(&fixture.check(&[]), "TS2345", "main.lua:5:");
    fixture.file(
        "factory.lua",
        &definition.replace("value={}", "value={x=42}"),
    );
    pass(&fixture.check(&[]));
    fixture.file(
        "factory.lua",
        "local value={x=42}\nreturn value\n---@class Point\n---@field x number\n",
    );
    fail(&fixture.check(&[]), "TS2345", "main.lua:5:");
    fixture.file(
        "factory.lua",
        &definition.replace("value={}", "value={x=42}"),
    );
    pass(&fixture.check(&[]));
}

#[test]
fn imported_definition_templates_keep_independent_original_captured_mappers() {
    let fixture = Fixture::new();
    fixture.file("maker.lua", "---@generic T\n---@param value T\nlocal function make(value)\n---@class Factory\n---@field value T\n---@overload fun():T\nlocal factory={value=value}\nreturn factory\nend\nreturn make\n");
    let valid = "local make=require('maker')\nlocal numbers=make(42)\nlocal strings=make('text')\n---@type number\nlocal first=numbers()\n---@type string\nlocal second=strings()\n---@type number\nlocal stored=numbers.value\n";
    fixture.file("main.lua", valid);
    pass(&fixture.check(&[]));
    fixture.file(
        "main.lua",
        &format!("{valid}---@type number\nlocal rejected=strings()\n"),
    );
    fail(&fixture.check(&[]), "TS2322", "main.lua:11:");
    fixture.file("main.lua", valid);
    pass(&fixture.check(&[]));
}

#[test]
fn global_definition_aliases_use_original_lazy_initializers_and_do_not_contextualize_shadowing_auto_locals()
 {
    let fixture = Fixture::new();
    let valid = "---@class Counter\n---@field value number\n---@overload fun(amount:number):number\nlocal definition={value=1}\nshared=definition\nlocal result=shared(42)\n---@type number\nlocal stored=shared.value\nlocal shared=nil\nshared='local'\n---@type string\nlocal shadow=shared\n";
    fixture.file("main.lua", valid);
    pass(&fixture.check(&[]));
    fixture.file("main.lua", &valid.replace("shared(42)", "shared('wrong')"));
    fail(&fixture.check(&[]), "TS2345", "main.lua:6:");
    fixture.file("main.lua", valid);
    pass(&fixture.check(&[]));
}

#[test]
fn completion_hints_preserve_definition_literal_priority_and_signature_local_generics() {
    let fixture = Fixture::new();
    fixture.file("factory.lua", "---@class Factory\n---@overload fun(value:('key'|`Suggested.key`)):string\n---@overload fun(value:'key'|`Suggested.key`):number\n---@overload fun<T:boolean>(value:T):T\nlocal factory={}\nreturn factory\n");
    fixture.file("main.lua", "local factory=require('factory')\n---@type number\nlocal literal=factory('key')\n---@type boolean\nlocal generic=factory(false)\n");
    pass(&fixture.check(&[]));
    fixture.file(
        "main.lua",
        "local factory=require('factory')\nlocal rejected=factory(42)\n",
    );
    let output = fixture.check(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
    assert!(text(&output).contains("main.lua:2:"), "{}", text(&output));
}

#[test]
fn ambient_callable_class_placeholders_never_run_checked_definition_initializers() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@class Widget\n---@field value number\n---@overload fun():number\nwidget=unresolved_placeholder()\n");
    fixture.file("main.lua", "---@type number\nlocal field=widget.value\n---@type number\nlocal result=widget()\n---@type table\nlocal table_value=widget\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "---@type function\nlocal rejected=widget\n");
    let output = fixture.check(&["api.def.lua"]);
    fail(&output, "TS2322", "main.lua:2:");
    assert!(
        !text(&output).contains("unresolved_placeholder"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("LUA2031"), "{}", text(&output));
}

#[test]
fn ambient_generic_table_identity_retains_imported_definition_heads_without_function_branding() {
    let fixture = Fixture::new();
    fixture.file("factory.lua", FACTORY);
    fixture.file("api.def.lua", "---@generic T:table\n---@param value T\n---@return T\nfunction keep_table(value) end\n---@generic F:function\n---@param value F\n---@return F\nfunction keep_function(value) end\n");
    fixture.file("main.lua", "local factory=keep_table((require('factory')))\n---@type number\nlocal result=factory(42)\nlocal length=#factory\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file(
        "main.lua",
        "local factory=keep_table((require('factory')))\nlocal rejected=keep_function(factory)\n",
    );
    fail(&fixture.check(&["api.def.lua"]), "TS2345", "main.lua:2:");
    fixture.file("main.lua", "local factory=keep_table((require('factory')))\n---@type number\nlocal corrected=factory(42)\n");
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn immutable_assignment_context_checks_the_original_custom_environment_property() {
    let source = "local _ENV={}\nmissing=42\n";
    let report = luanalysis::check(source);
    let gates: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|error| error.code == "TS2339")
        .collect();
    assert_eq!(gates.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(&source[gates[0].span.start..gates[0].span.end], "missing");
    let fixed = "local _ENV={missing=0}\nmissing=42\n";
    let rejected = luanalysis::check(fixed);
    assert_eq!(rejected.diagnostics.len(), 1, "{:#?}", rejected.diagnostics);
    assert_eq!(rejected.diagnostics[0].code, "TS2322");
    let span = rejected.diagnostics[0].span;
    assert_eq!(&fixed[span.start..span.end], "42");
    let corrected = luanalysis::check(
        "---@type {missing:number}\nlocal _ENV={missing=0}\nmissing=42\n",
    );
    assert!(corrected.diagnostics.is_empty(), "{:#?}", corrected.diagnostics);
}

#[test]
fn inherited_partial_module_members_keep_write_constraints_and_actual_instance_requirements() {
    let fixture = Fixture::new();
    fixture.file("base.def.lua", "---@class Base\n---@field value number\n");
    let derived = "---@class Derived:Base\nlocal prototype={}\nreturn prototype\n";
    fixture.file("derived.lua", derived);
    fixture.file("main.lua", "local prototype=require('derived')\n---@type number|nil\nlocal before=prototype.value\nprototype.value=42\n---@type number\nlocal after=prototype.value\n");
    pass(&fixture.check(&["base.def.lua"]));
    fixture.file(
        "main.lua",
        "local prototype=require('derived')\nprototype.value='wrong'\n",
    );
    fail(&fixture.check(&["base.def.lua"]), "TS2322", "main.lua:2:");
    fixture.file("main.lua", "local prototype=require('derived')\n---@param value Base\n---@return number\nlocal function read(value) return value.value end\nlocal result=read(prototype)\n");
    fail(&fixture.check(&["base.def.lua"]), "TS2345", "main.lua:5:");
    fixture.file(
        "derived.lua",
        &derived.replace("prototype={}", "prototype={value=42}"),
    );
    pass(&fixture.check(&["base.def.lua"]));
}

#[test]
fn ambient_inherited_class_globals_keep_complete_external_schema_and_nominal_ancestry() {
    let fixture = Fixture::new();
    fixture.file("base.def.lua", "---@class Base\n---@field value number\n");
    fixture.file(
        "derived.def.lua",
        "---@class Derived:Base\n---@overload fun():number\nderived=unresolved_placeholder()\n",
    );
    fixture.file("main.lua", "---@param value Base\n---@return number\nlocal function read(value) return value.value end\n---@type number\nlocal field=read(derived)\n---@type number\nlocal called=derived()\n---@type table\nlocal category=derived\n");
    pass(&fixture.check(&["base.def.lua", "derived.def.lua"]));
    fixture.file(
        "main.lua",
        "---@param value Base\nlocal function read(value) end\nread({value=42})\n",
    );
    fail(
        &fixture.check(&["base.def.lua", "derived.def.lua"]),
        "TS2345",
        "main.lua:3:",
    );
}

#[test]
fn global_alias_of_imported_derived_definition_contextualizes_inherited_callbacks() {
    let fixture = Fixture::new();
    fixture.file(
        "base.def.lua",
        "---@class Base\n---@field callback fun(value:number):number\n",
    );
    fixture.file("derived.lua", "---@class Derived:Base\nlocal definition={callback=function(value) return value+1 end}\nreturn definition\n");
    let valid = "shared=require('derived')\n---@type number\nlocal result=shared.callback(42)\n";
    fixture.file("main.lua", valid);
    pass(&fixture.check(&["base.def.lua"]));
    fixture.file(
        "main.lua",
        &format!("{valid}shared.callback=function(value) return 'wrong' end\n"),
    );
    fail(&fixture.check(&["base.def.lua"]), "TS2322", "main.lua:4:");
    fixture.file("main.lua", valid);
    pass(&fixture.check(&["base.def.lua"]));
}

#[test]
fn mapped_generic_ancestor_definitions_keep_foreign_outer_parameter_identity() {
    let fixture = Fixture::new();
    fixture.file("base.def.lua", "---@class Box<T>\n---@field value T\n");
    fixture.file("maker.lua", "---@generic T\n---@param value T\nlocal function make(value)\n---@class Derived:Box<T>\nlocal definition={value=value}\nreturn definition\nend\nreturn make\n");
    let valid = "local make=require('maker')\nlocal numbers=make(42)\nlocal strings=make('text')\n---@type number\nlocal number=numbers.value\n---@type string\nlocal string=strings.value\n---@param value Box<number>\n---@return number\nlocal function read(value) return value.value end\nlocal accepted=read(numbers)\n";
    fixture.file("main.lua", valid);
    pass(&fixture.check(&["base.def.lua"]));
    fixture.file(
        "main.lua",
        &format!("{valid}local rejected=read(strings)\n"),
    );
    fail(&fixture.check(&["base.def.lua"]), "TS2345", "main.lua:12:");
    fixture.file("main.lua", valid);
    pass(&fixture.check(&["base.def.lua"]));
}

#[test]
fn completion_only_base_hints_preserve_record_identity_and_real_unions_stay_invalid() {
    let fixture = Fixture::new();
    fixture.file("base.def.lua", "---@class Base\n---@field value number\n");
    let valid = "---@class Derived:(Base|`Suggested.Base`)\nlocal definition={value=42}\nreturn definition\n";
    fixture.file("derived.lua", valid);
    fixture.file("main.lua", "local value=require('derived')\n---@param value Base\n---@return number\nlocal function read(value) return value.value end\nlocal result=read(value)\n");
    pass(&fixture.check(&["base.def.lua"]));
    fixture.file(
        "derived.lua",
        &valid.replace("Base|`Suggested.Base`", "Base|number"),
    );
    fail(
        &fixture.check(&["base.def.lua"]),
        "TS2312",
        "derived.lua:1:",
    );
    fixture.file("derived.lua", valid);
    pass(&fixture.check(&["base.def.lua"]));
}

#[test]
fn inherited_base_argument_errors_keep_one_original_diagnostic_and_accept_correction() {
    let fixture = Fixture::new();
    fixture.file(
        "base.def.lua",
        "---@class Base<T:number>\n---@field value T\n",
    );
    let invalid = "---@class Derived:Base<string>\nlocal definition={}\nreturn definition\n";
    fixture.file("derived.lua", invalid);
    fixture.file("main.lua", "local value=require('derived')\n");
    let output = fixture.check(&["base.def.lua"]);
    fail(&output, "TS2344", "derived.lua:1:");
    assert_eq!(
        text(&output).matches("error[TS2344]").count(),
        1,
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("TS2312"), "{}", text(&output));
    fixture.file(
        "derived.lua",
        &invalid.replace("Base<string>", "Base<number>"),
    );
    pass(&fixture.check(&["base.def.lua"]));
    fixture.file(
        "derived.lua",
        &invalid
            .replace("Base<string>", "Base<number>")
            .replace("definition={}", "definition={value='wrong'}"),
    );
    let output = fixture.check(&["base.def.lua"]);
    fail(&output, "TS2416", "derived.lua:2:");
    assert_eq!(text(&output).matches("error[TS2416]").count(), 1, "{}", text(&output));
    assert!(!text(&output).contains("LUA2031"), "{}", text(&output));
}

#[test]
fn trusted_grandchild_global_keeps_valid_parent_indexer_refinement() {
    let fixture = Fixture::new();
    fixture.file(
        "base.def.lua",
        "---@shape Base\n---@field [string] number|string\n",
    );
    fixture.file(
        "middle.def.lua",
        "---@shape Middle:Base\n---@field [string] string\n",
    );
    fixture.file("leaf.def.lua", "---@class Leaf:Middle\nleaf={}\n");
    let libraries = ["base.def.lua", "middle.def.lua", "leaf.def.lua"];
    fixture.file(
        "main.lua",
        "---@type string\nlocal inherited=leaf.answer\nleaf.answer='text'\n",
    );
    pass(&fixture.check(&libraries));
    fixture.file("main.lua", "leaf.answer=42\n");
    fail(&fixture.check(&libraries), "TS2322", "main.lua:1:");
}

#[test]
fn inherited_callback_override_checks_complete_native_result_packs_before_module_publication() {
    let fixture = Fixture::new();
    fixture.file(
        "base.def.lua",
        "---@class Base\n---@field read fun():number,string\n",
    );
    let invalid = "---@class Derived:Base\n---@field read fun():number\nlocal definition={}\nreturn definition\n";
    fixture.file("derived.lua", invalid);
    fixture.file("main.lua", "local value=require('derived')\n");
    fail(
        &fixture.check(&["base.def.lua"]),
        "TS2416",
        "derived.lua:2:",
    );
    fixture.file("derived.lua", "---@class Derived:Base\n---@field read fun():number,string\nlocal definition={read=function() return 42,'text' end}\nreturn definition\n");
    fixture.file("main.lua", "local value=require('derived')\nlocal number,string=value.read()\n---@type number\nlocal n=number\n---@type string\nlocal s=string\n");
    pass(&fixture.check(&["base.def.lua"]));
}
