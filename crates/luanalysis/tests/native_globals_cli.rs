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
            "luanalysis globals {} {}",
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
        self.check_files(libraries, &["main.lua"])
    }
    fn check_files(&self, libraries: &[&str], files: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"));
        command.current_dir(&self.0).args(["check", "--dump-types"]);
        for library in libraries {
            command.args(["--library", library]);
        }
        command.args(files).output().unwrap()
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
fn fail(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(text(output).contains(code), "{}", text(output));
}
const API: &str = "---@param text string\n---@return number\nfunction parse(text) end\n---@type number\nanswer = nil\n";

#[test]
fn ambient_annotated_functions_and_values_use_contracts_without_placeholder_inference() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", API);
    fixture.file("main.lua", "local parsed = parse('42')\n---@type number\nlocal value = answer\n---@type number\nlocal result = parsed\n");
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    assert!(
        text(&output).contains("parsed: number"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("TS2355"), "{}", text(&output));
}

#[test]
fn actual_original_crlf_argument_and_result_errors_accept_correction() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", API);
    fixture.file(
        "main.lua",
        "-- café\r\nlocal result = parse(42)\r\n---@type string\r\nlocal bad = answer\r\n",
    );
    let invalid = fixture.check(&["api.def.lua"]);
    fail(&invalid, "main.lua:2:22: error[TS2345]");
    assert!(
        text(&invalid).contains("main.lua:4:13: error[TS2322]"),
        "{}",
        text(&invalid)
    );
    assert!(!text(&invalid).contains("TS2304"), "{}", text(&invalid));
    assert!(
        !text(&invalid).contains("api.def.lua:"),
        "{}",
        text(&invalid)
    );
    fixture.file(
        "main.lua",
        "-- café\r\nlocal result = parse('42')\r\n---@type number\r\nlocal good = answer\r\n",
    );
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn ambient_generics_keep_original_parameter_owners_and_native_call_inference() {
    let fixture = Fixture::new();
    fixture.file(
        "api.def.lua",
        "---@generic T\n---@param value T\n---@return T\nfunction identity(value) end\n",
    );
    fixture.file(
        "main.lua",
        "local result = identity(42)\n---@type number\nlocal valid = result\n",
    );
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    assert!(
        text(&output).contains("result: number"),
        "{}",
        text(&output)
    );
    fixture.file("main.lua", "---@type string\nlocal bad = identity(42)\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2322");
}

#[test]
fn ambient_multi_results_and_variadics_preserve_written_physical_pack_contracts() {
    let fixture = Fixture::new();
    fixture.file(
        "api.def.lua",
        "---@vararg number\n---@return number, string\nfunction split(...) end\n",
    );
    fixture.file("main.lua", "local count, label = split(1, 2, 3)\n---@type number\nlocal n = count\n---@type string\nlocal s = label\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "local count, label = split('wrong')\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2345");
}

#[test]
fn generic_contract_identity_compares_distinct_original_parameter_owners() {
    let fixture = Fixture::new();
    let contract = "---@generic T\n---@param value T\n---@return T\nfunction identity(value) end\n";
    fixture.file("one.def.lua", contract);
    fixture.file("two.def.lua", contract);
    fixture.file("main.lua", "---@type number\nlocal value = identity(42)\n");
    pass(&fixture.check(&["one.def.lua", "two.def.lua"]));
}

#[test]
fn function_type_annotation_contracts_validate_metadata_and_keep_original_generics() {
    let fixture = Fixture::new();
    fixture.file(
        "api.def.lua",
        "---@generic T\n---@type fun(value:T):T\nfunction identity(value) end\n",
    );
    fixture.file("main.lua", "---@type number\nlocal value = identity(42)\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("api.def.lua", "---@type number\nfunction invalid() end\n");
    fixture.file("main.lua", "local value = 42\n");
    fail(&fixture.check(&["api.def.lua"]), "LUA2010");
}

#[test]
fn implementation_writes_check_the_selected_library_contract_at_the_original_rhs() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@type number\nanswer = nil\n");
    fixture.file("main.lua", "answer = 42\nanswer = 43\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "answer = 'wrong'\n");
    let output = fixture.check(&["api.def.lua"]);
    fail(&output, "main.lua:1:10: error[TS2322]");
    assert!(!text(&output).contains("TS2403"), "{}", text(&output));
    assert!(!text(&output).contains("api.def.lua:"), "{}", text(&output));
}

#[test]
fn ambient_nil_return_contract_does_not_invent_a_guaranteed_physical_result() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@return nil\nfunction finish() end\n---@param value nil\n---@return void\nfunction consume(value) end\n");
    fixture.file("main.lua", "consume(finish())\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2554");
    fixture.file("main.lua", "consume(nil)\n");
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn complete_implementation_function_contracts_keep_actual_body_result_cardinality() {
    let fixture = Fixture::new();
    fixture.file(
        "api.def.lua",
        "---@param value nil\n---@return void\nfunction consume(value) end\n",
    );
    fixture.file(
        "main.lua",
        "---@return nil\nfunction finish() return nil end\nconsume(finish())\n",
    );
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file(
        "main.lua",
        "---@return nil\nfunction finish() end\nconsume(finish())\n",
    );
    fail(&fixture.check(&["api.def.lua"]), "TS2554");
}

#[test]
fn independent_chunks_merge_equivalent_introductions_and_report_incompatible_owners() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "answer = 1\nanswer = 2\n");
    fixture.file("other.lua", "answer = 3\nanswer = 4\n");
    for files in [["main.lua", "other.lua"], ["other.lua", "main.lua"]] {
        pass(&fixture.check_files(&[], &files));
    }
    fixture.file("other.lua", "answer = 'wrong'\n");
    for files in [["main.lua", "other.lua"], ["other.lua", "main.lua"]] {
        let output = fixture.check_files(&[], &files);
        fail(&output, "TS2403");
        assert!(
            text(&output).contains("main.lua:1:1: error[TS2403]"),
            "{}",
            text(&output)
        );
        assert!(
            text(&output).contains("other.lua:1:1: error[TS2403]"),
            "{}",
            text(&output)
        );
        assert_eq!(
            text(&output).matches("error[TS2403]").count(),
            2,
            "{}",
            text(&output)
        );
    }
}

#[test]
fn written_union_contract_owns_initialization_and_later_writes() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@type number|string\nanswer = nil\n");
    fixture.file("main.lua", "answer = 42\nanswer = 'valid'\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "answer = false\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2322");
}

#[test]
fn conflicting_global_contracts_keep_the_first_and_report_the_later_in_either_library_order() {
    let fixture = Fixture::new();
    fixture.file("one.def.lua", "---@type number\nanswer = nil\n");
    fixture.file("two.def.lua", "---@type string\nanswer = nil\n");
    fixture.file("main.lua", "local result = answer\n");
    for (libraries, first) in [
        (["one.def.lua", "two.def.lua"], "number"),
        (["two.def.lua", "one.def.lua"], "string"),
    ] {
        let output = fixture.check(&libraries);
        fail(&output, "TS2403");
        assert!(text(&output).contains(&format!("{}:2:1: error[TS2403]", libraries[1])), "{}", text(&output));
        assert_eq!(text(&output).matches("error[TS2403]").count(), 1, "{}", text(&output));
        // The global is usable, of its first declaration's type.
        assert!(text(&output).contains(&format!("result: {first}")), "{}", text(&output));
    }
}

#[test]
fn equivalent_written_alias_contracts_merge_but_any_and_number_conflict() {
    let fixture = Fixture::new();
    fixture.file(
        "one.def.lua",
        "---@alias Count number\n---@type Count\nanswer = nil\n",
    );
    fixture.file("two.def.lua", "---@type number\nanswer = nil\n");
    fixture.file("main.lua", "---@type number\nlocal value = answer\n");
    pass(&fixture.check(&["one.def.lua", "two.def.lua"]));
    fixture.file("two.def.lua", "---@type any\nanswer = nil\n");
    fail(&fixture.check(&["one.def.lua", "two.def.lua"]), "TS2403");
}

#[test]
fn local_names_parameters_and_initializer_activation_take_precedence_over_globals() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", API);
    fixture.file("main.lua", "local answer = answer\n---@param value boolean\n---@return boolean\nlocal parse = function(value) return value end\nlocal local_result = parse(false)\n---@type boolean\nlocal valid = local_result\n---@param answer string\n---@return string\nlocal function local_name(answer) return answer end\n");
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    assert!(
        text(&output).contains("answer: number"),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains("local_result: boolean"),
        "{}",
        text(&output)
    );
}

#[test]
fn declaration_placeholders_never_supply_missing_contracts_and_metadata_is_validated() {
    let fixture = Fixture::new();
    fixture.file(
        "api.def.lua",
        "untyped = unresolved_placeholder()\n---@param value Missing\n---@return number\nfunction parse(value) end\n",
    );
    fixture.file("main.lua", "local value = 1\n");
    let output = fixture.check(&["api.def.lua"]);
    fail(&output, "api.def.lua:1:1: error[LUA2035]");
    assert!(
        text(&output).contains("api.def.lua:2:17: error[TS2304]"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("TS2355"), "{}", text(&output));
}

#[test]
fn unsupported_ambient_execution_and_environment_lookup_have_original_diagnostics() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "local placeholder = {}\n");
    fixture.file("main.lua", "local value = _G.answer\n");
    let output = fixture.check(&["api.def.lua"]);
    fail(&output, "api.def.lua:1:1: error[LUA2034]");
    assert!(
        text(&output).contains("main.lua:1:15: error[TS2304]"),
        "{}",
        text(&output)
    );
    fixture.file("api.def.lua", API);
    fixture.file("main.lua", "local _ENV = {}\nlocal value = answer\n");
    fail(
        &fixture.check(&["api.def.lua"]),
        "main.lua:2:15: error[TS2339]",
    );
}

#[test]
fn global_table_and_class_contracts_keep_required_members_despite_empty_placeholders() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@class Settings\n---@field count number\nsettings = {}\n---@type {name:string}\nrecord = {}\n");
    fixture.file("main.lua", "---@type number\nlocal count = settings.count\n---@type string\nlocal name = record.name\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file(
        "main.lua",
        "---@type string\nlocal wrong = settings.count\nlocal missing = record.absent\n",
    );
    let output = fixture.check(&["api.def.lua"]);
    fail(&output, "TS2322");
    assert!(text(&output).contains("TS2339"), "{}", text(&output));
}

#[test]
fn ambient_overload_heads_select_results_without_an_implementation_or_body_check() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@overload fun(value:number):number\n---@overload fun(value:string):string\nfunction echo(value) return unresolved_placeholder(value) end\n");
    fixture.file(
        "main.lua",
        "---@type number\nlocal number = echo(42)\n---@type string\nlocal string = echo('value')\n",
    );
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "local rejected = echo(true)\n");
    let output = fixture.check(&["api.def.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
    for private_error in ["TS2394", "TS2355", "TS7006", "TS2304", "LUA2035"] {
        assert!(
            !text(&output).contains(private_error),
            "{private_error}: {}",
            text(&output)
        );
    }
    fixture.file("main.lua", "local corrected = echo(42)\n");
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn ambient_signature_local_generics_shadow_function_tags_with_original_owners() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@overload fun<T:number>(value:T):T\n---@overload fun<T:string>(value:T):T\n---@generic T:boolean\nfunction identity(value) return unresolved_placeholder(value) end\n");
    fixture.file("main.lua", "---@type number\nlocal number = identity(42)\n---@type string\nlocal string = identity('value')\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "local rejected = identity(false)\n");
    let output = fixture.check(&["api.def.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("TS2394"), "{}", text(&output));
}

#[test]
fn ambient_public_heads_require_explicit_signatures_and_keep_the_main_contract_private() {
    let fixture = Fixture::new();
    let private = "---@overload fun(value:number):number\n---@overload fun(value:string):string\n---@param value number|string|boolean\n---@return number|string|boolean\nfunction echo(value) return unresolved_placeholder(value) end\n";
    fixture.file("api.def.lua", private);
    fixture.file("main.lua", "---@type boolean\nlocal accepted = echo(true)\n");
    let private_only = fixture.check(&["api.def.lua"]);
    assert_eq!(private_only.status.code(), Some(1), "{}", text(&private_only));
    assert!(
        text(&private_only).contains("TS2345") || text(&private_only).contains("TS2769"),
        "{}",
        text(&private_only)
    );
    assert!(!text(&private_only).contains("TS2394"), "{}", text(&private_only));
    fixture.file(
        "api.def.lua",
        &private.replace(
            "---@param",
            "---@overload fun(value:boolean):boolean\n---@param",
        ),
    );
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "local rejected = echo({})\n");
    let output = fixture.check(&["api.def.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("TS2394"), "{}", text(&output));
    assert!(!text(&output).contains("TS2304"), "{}", text(&output));
    fixture.file("main.lua", "---@type number\nlocal rejected = echo(true)\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2322");
    fixture.file("main.lua", "---@type number\nlocal corrected = echo(42)\n");
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn ambient_original_signature_heads_keep_multi_results_and_implicit_empty_nil_contracts() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@overload fun(count:1):number\n---@overload fun(count:2):number,string\nfunction split(count) end\n---@overload fun():nil\nfunction finish() end\n---@param value nil\n---@return void\nfunction consume(value) end\n");
    fixture.file("main.lua", "local count, label = split(2)\n---@type number\nlocal number = count\n---@type string\nlocal string = label\nconsume(finish())\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2554");
    fixture.file("main.lua", "local count, label = split(2)\n---@type number\nlocal number = count\n---@type string\nlocal string = label\nconsume(nil)\n");
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn named_callable_global_library_contracts_resolve_heads_from_another_original_file() {
    let fixture = Fixture::new();
    fixture.file("types.def.lua", "---@shape Callable\n---@overload fun(value:number):number\n---@overload fun(value:string):string\n");
    fixture.file("api.def.lua", "---@type Callable\necho = {}\n");
    fixture.file(
        "main.lua",
        "---@type number\nlocal number = echo(42)\n---@type string\nlocal string = echo('value')\n",
    );
    pass(&fixture.check(&["types.def.lua", "api.def.lua"]));
    fixture.file("main.lua", "local rejected = echo(false)\n");
    let output = fixture.check(&["types.def.lua", "api.def.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
}

#[test]
fn unused_ambient_heads_still_validate_original_constraints_and_written_tags() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@overload fun<T:Missing>(value:T):T\nfunction unused(value) return unresolved_placeholder(value) end\n");
    fixture.file("main.lua", "local value = 42\n");
    let output = fixture.check(&["api.def.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("Missing") && text(&output).contains("api.def.lua:1:"),
        "{}",
        text(&output)
    );
    assert!(
        !text(&output).contains("unresolved_placeholder"),
        "{}",
        text(&output)
    );
    fixture.file("api.def.lua", "---@overload fun(value:number):number\n---@param value Missing\nfunction unused(value) end\n");
    let output = fixture.check(&["api.def.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("Missing") && text(&output).contains("api.def.lua:2:"),
        "{}",
        text(&output)
    );
}

#[test]
fn ambient_annotation_signature_formals_keep_original_shadowed_generic_owners() {
    let fixture = Fixture::new();
    fixture.file(
        "api.def.lua",
        "---@generic T:boolean\n---@type fun<T:number>(value:T):T\nfunction identity(value) end\n",
    );
    fixture.file("main.lua", "---@type number\nlocal value = identity(42)\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "local rejected = identity('wrong')\n");
    let rejected = fixture.check(&["api.def.lua"]);
    assert_eq!(rejected.status.code(), Some(1), "{}", text(&rejected));
    assert!(
        text(&rejected).contains("TS2345") || text(&rejected).contains("TS2769"),
        "{}",
        text(&rejected)
    );
}

#[test]
fn ambient_raw_table_and_function_value_contracts_preserve_native_runtime_domains() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@type table\nobject = nil\n---@type function\ncallback = nil\n---@type fun\nbare = nil\n---@type fun()\nempty = nil\n");
    fixture.file("main.lua", "object.field = 42\nlocal field = object.field\nlocal first, second = callback(42, 'word')\nlocal other = bare(42, 'word')\nempty()\n");
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    for binding in ["field: any", "first: any", "second: any", "other: any"] {
        assert!(
            text(&output).contains(binding),
            "{binding}: {}",
            text(&output)
        );
    }
    fixture.file("main.lua", "object = 42\ncallback = {}\nempty(42)\n");
    let output = fixture.check(&["api.def.lua"]);
    fail(&output, "TS2322");
    assert!(text(&output).contains("TS2554"), "{}", text(&output));
}

#[test]
fn named_ambient_primitive_function_stubs_keep_runtime_top_and_ignore_placeholder_parameters() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@type function\nfunction raw(value) return unresolved_placeholder(value) end\n---@type fun\nfunction bare(value) end\n---@type fun:number\nfunction typed() end\n");
    fixture.file("main.lua", "local raw_value = raw\nlocal bare_value = bare\nlocal first, second = raw(42, 'word')\nlocal other = bare(42, 'word')\n---@type number\nlocal result = typed(42, 'word')\n");
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    for binding in [
        "raw_value: function",
        "bare_value: function",
        "first: any",
        "second: any",
        "other: any",
        "result: number",
    ] {
        assert!(
            text(&output).contains(binding),
            "{binding}: {}",
            text(&output)
        );
    }
}

#[test]
fn ambient_bare_fun_and_raw_function_declaration_owners_have_identical_runtime_contracts() {
    let fixture = Fixture::new();
    fixture.file(
        "one.def.lua",
        "---@type fun\nfunction dynamic(...) return unresolved_placeholder() end\n",
    );
    fixture.file("two.def.lua", "---@type function\ndynamic = nil\n");
    fixture.file(
        "main.lua",
        "local value = dynamic\nlocal first, second = dynamic(42, 'word')\n",
    );
    let output = fixture.check(&["one.def.lua", "two.def.lua"]);
    pass(&output);
    assert!(
        text(&output).contains("value: function"),
        "{}",
        text(&output)
    );
    assert!(text(&output).contains("second: any"), "{}", text(&output));
}

#[test]
fn ambient_ordinary_parameters_keep_raw_runtime_and_headerless_typed_contracts() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@param value table\n---@return number\nfunction useTable(value) end\n---@param callback function\n---@return any...\nfunction apply(callback) end\n---@param callback fun:number\n---@return number\nfunction typed(callback) end\n");
    fixture.file("main.lua", "---@type number\nlocal size = useTable({})\nlocal first, second = apply(function(value) return value end)\n---@type number\nlocal result = typed(function() return 42 end)\n");
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    assert!(text(&output).contains("second: any"), "{}", text(&output));
    fixture.file("main.lua", "useTable(function() end)\napply({})\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2345");
}

#[test]
fn ambient_callable_class_globals_remain_runtime_tables_when_used_as_public_heads() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@class Callable\n---@overload fun(value:number):number\ncallable = {}\n---@overload fun(value:table):'table'\n---@overload fun(value:function):'function'\nfunction classify(value) end\n---@param callback function\n---@return void\nfunction takeFunction(callback) end\n");
    fixture.file("main.lua", "---@type table\nlocal object = callable\n---@type 'table'\nlocal category = classify(callable)\n---@type number\nlocal result = callable(42)\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file("main.lua", "takeFunction(callable)\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2345");
    fixture.file("main.lua", "takeFunction(function() end)\n");
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn ambient_function_runtime_bounds_keep_the_original_specific_inferred_signatures() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@generic F:function\n---@param callback F\n---@return F\nfunction keep(callback) end\n---@overload fun<F:function>(callback:F):F\nfunction keepHead(callback) end\n---@generic F:function\n---@type fun(callback:F):F\nfunction keepTyped(callback) end\n");
    fixture.file("main.lua", "---@param value number\n---@return number\nlocal function numeric(value) return value end\nlocal specific = keep(numeric)\nlocal head = keepHead(numeric)\nlocal typed = keepTyped(numeric)\n---@type number\nlocal first = specific(42)\n---@type number\nlocal second = head(42)\n---@type number\nlocal third = typed(42)\n");
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    assert!(text(&output).contains("third: number"), "{}", text(&output));
    fixture.file("main.lua", "---@param value number\n---@return number\nlocal function numeric(value) return value end\nlocal specific = keepTyped(numeric)\nlocal rejected = specific('wrong')\n");
    fail(&fixture.check(&["api.def.lua"]), "TS2345");
}

#[test]
fn ambient_headerless_generic_and_typed_heads_keep_any_rest_and_physical_results() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@type fun:number\nfunction typed(...) end\n---@type fun<T:number>\nfunction generic(...) end\n---@overload fun\nfunction bare(...) end\n");
    fixture.file("main.lua", "---@type number\nlocal typed_result = typed(42, 'word')\nlocal first, second = generic(42, 'word')\nlocal third, fourth = bare(42, 'word')\n");
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    for binding in ["first: any", "second: any", "third: any", "fourth: any"] {
        assert!(
            text(&output).contains(binding),
            "{binding}: {}",
            text(&output)
        );
    }
}

#[test]
fn hinted_ambient_bare_function_contract_keeps_the_native_runtime_carrier() {
    let fixture = Fixture::new();
    fixture.file("one.def.lua", "---@type fun|`Suggested.call`\nfunction dynamic(...) return unresolved_placeholder() end\n");
    fixture.file("two.def.lua", "---@type function\ndynamic = nil\n");
    fixture.file(
        "main.lua",
        "local value = dynamic\nlocal first, second = dynamic(42, 'word')\n",
    );
    let output = fixture.check(&["one.def.lua", "two.def.lua"]);
    pass(&output);
    assert!(
        text(&output).contains("value: function"),
        "{}",
        text(&output)
    );
    assert!(text(&output).contains("second: any"), "{}", text(&output));
}

#[test]
fn hinted_ambient_heads_keep_generics_and_parenthesized_literal_priority() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@type (fun<T>(value:T):T)|`Suggested.identity`\nfunction identity(value) return unresolved_placeholder(value) end\n---@overload fun(value:string):number\n---@overload fun(value:'special'|`Suggested.special`):string\nfunction choose(value) end\n---@overload fun(value:string):number\n---@overload fun(value:('special')|`Suggested.special`):string\nfunction parenthesized(value) end\n");
    fixture.file("main.lua", "---@type number\nlocal numeric = identity(42)\n---@type string\nlocal textual = identity('word')\n---@type string\nlocal direct = choose('special')\n---@type number\nlocal wrapped = parenthesized('special')\n");
    let output = fixture.check(&["api.def.lua"]);
    pass(&output);
    assert!(!text(&output).contains("Suggested"), "{}", text(&output));
    fixture.file("main.lua", "local wrong = choose(false)\n");
    let output = fixture.check(&["api.def.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
}

#[test]
fn implementation_modules_export_original_ambient_overload_heads_to_require_importers() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@overload fun(value:number|`Suggested.numeric`):number\n---@overload fun(value:string|`Suggested.textual`):string\nfunction echo(value) return unresolved_placeholder(value) end\n");
    fixture.file("exporter.lua", "return echo\n");
    fixture.file("main.lua", "local imported = require('exporter')\n---@type number\nlocal numeric = imported(42)\n---@type string\nlocal textual = imported('word')\n");
    pass(&fixture.check(&["api.def.lua"]));
    fixture.file(
        "main.lua",
        "local imported = require('exporter')\nlocal wrong = imported(false)\n",
    );
    let output = fixture.check(&["api.def.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
    assert!(text(&output).contains("main.lua:2:"), "{}", text(&output));
    assert!(
        !text(&output).contains("unresolved_placeholder"),
        "{}",
        text(&output)
    );
    fixture.file(
        "main.lua",
        "local imported = require('exporter')\nlocal corrected = imported(42)\n",
    );
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn implementation_global_bodies_import_modules_without_losing_original_diagnostic_owners() {
    let fixture = Fixture::new();
    fixture.file("payload.lua", "return {value = 'wrong'}\n");
    fixture.file("reader.lua", "---@return number\nfunction loadPayload()\nlocal payload = require('payload')\nreturn payload.value\nend\n");
    fixture.file(
        "main.lua",
        "require('reader')\n---@type number\nlocal result = loadPayload()\n",
    );
    let output = fixture.check(&[]);
    fail(&output, "reader.lua:4:8: error[TS2322]");
    fixture.file("payload.lua", "return {value = 42}\n");
    pass(&fixture.check(&[]));
}

#[test]
fn declaration_module_exports_remain_gated_without_inferring_placeholder_execution() {
    let fixture = Fixture::new();
    fixture.file(
        "contract.lua",
        "---@type number\nanswer = nil\nreturn unresolved_placeholder()\n",
    );
    fixture.file("main.lua", "local value = require('contract')\n");
    let output = fixture.check(&["contract.lua"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("Declaration modules require an explicit export contract"),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains("contract.lua:3:1: error[LUA2034]"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("TS2304"), "{}", text(&output));
    fixture.file("contract.lua", "---@type number\nanswer = nil\n");
    fixture.file("main.lua", "---@type number\nlocal value = answer\n");
    pass(&fixture.check(&["contract.lua"]));
}

#[test]
fn local_require_and_type_bindings_do_not_load_phantoms_or_apply_runtime_intrinsic_narrowing() {
    let fixture = Fixture::new();
    fixture.file("main.lua", "---@param name string\n---@return number\nlocal function require(name) return 42 end\n---@param value any\n---@return string\nlocal function type(value) return 'function' end\nlocal value = require('phantom')\nif type(value) == 'function' then\n---@type function\nlocal wrong = value\nend\n");
    let output = fixture.check(&[]);
    fail(&output, "main.lua:10:15: error[TS2322]");
    assert!(!text(&output).contains("TS2307"), "{}", text(&output));
    fixture.file("main.lua", "---@param name string\n---@return number\nlocal function require(name) return 42 end\n---@param value any\n---@return string\nlocal function type(value) return 'function' end\nlocal value = require('phantom')\nif type(value) == 'function' then\n---@type number\nlocal corrected = value\nend\n");
    pass(&fixture.check(&[]));
}

#[test]
fn imported_captured_functions_keep_owner_mappers_under_ambient_generic_callback_contexts() {
    let fixture = Fixture::new();
    fixture.file("api.def.lua", "---@overload fun<T>(callback:fun():T|`Suggested.result`):T\nfunction invoke(callback) return unresolved_placeholder(callback) end\n");
    fixture.file("factory.lua", "---@generic T\n---@param seed T\nlocal function make(seed) return function() return seed end end\nreturn make\n");
    fixture.file("main.lua", "local make = require('factory')\nlocal numeric = make(42)\nlocal textual = make('word')\n---@type number\nlocal first = invoke(numeric)\n---@type string\nlocal second = invoke(textual)\n---@type number\nlocal again = invoke(numeric)\n");
    pass(&fixture.check(&["api.def.lua"]));
}

#[test]
fn a_source_redeclaring_a_standard_global_leaves_it_the_standard_librarys() {
    let fixture = Fixture::new();
    // As LuaLS's own meta templates redeclare the standard library.
    fixture.file("string.lua", "---@meta string\n\n---@class stringlib\nstring = {}\n\nreturn string\n");
    fixture.file("main.lua", "local length = string.len('x')\nlocal format = string.format\n");
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .current_dir(&fixture.0)
        .args(["check", "--dump-types", "--lua-version", "5.4", "string.lua", "main.lua"])
        .output()
        .unwrap();
    let report = text(&output);
    assert!(report.contains("string.lua:4:1: error[TS2403]: Subsequent variable declarations must have the same type. Variable 'string' must be of type 'string', but here has type 'stringlib'."), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
    assert!(report.contains("length: number") && report.contains("format: fun(formatstring: string"), "{report}");
}
