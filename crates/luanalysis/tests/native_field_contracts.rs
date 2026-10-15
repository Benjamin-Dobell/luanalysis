use luanalysis::{check, CheckReport};

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}
fn binding<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report.bindings.iter().find(|binding| binding.name == name).unwrap().ty
}

#[test]
fn scalar_field_contracts_check_initializers_and_subsequent_mutable_writes() {
    let source = r#"local record = {
---@type number
value = 1,
---@type 1
one = 1,
inferred = 1,
}
record.value = 2
record.one = 1
record.inferred = 2
local copied = record.one
"#;
    let report = accepted(source);
    assert_eq!(binding(&report, "copied"), "1");
    for (suffix, rejected) in [("record.value = \"wrong\"", "\"wrong\""), ("record.one = 2", "2"), ("copied = 2", "2")] {
        let bad = format!("{source}{suffix}\n");
        let report = check(&bad);
        let diagnostic = report.diagnostics.iter().find(|error| error.code == "TS2322").unwrap_or_else(|| panic!("{suffix}: {:#?}", report.diagnostics));
        assert_eq!(&bad[diagnostic.span.start..diagnostic.span.end], rejected);
    }
    let bad = source.replacen("value = 1", "value = \"wrong\"", 1);
    let report = check(&bad);
    let diagnostic = report.diagnostics.iter().find(|error| error.code == "TS2322").unwrap();
    assert_eq!(&bad[diagnostic.span.start..diagnostic.span.end], "\"wrong\"");
    accepted(&bad.replace("\"wrong\"", "1"));
}

#[test]
fn a_written_broad_union_narrows_to_its_primitive_arm_not_a_fresh_literal() {
    let source = "local record={\n---@type number|string\nvalue=1,\n}\nrecord.value=1\nlocal copied=record.value\n";
    let report = accepted(source);
    assert_eq!(binding(&report, "copied"), "number");
    let wrong = format!("{source}---@type 1\nlocal singleton=record.value\n");
    let report = check(&wrong);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"), "{:#?}", report.diagnostics);
    accepted(&wrong.replace("---@type 1", "---@type number"));
}

#[test]
fn function_field_contracts_type_the_actual_body_and_later_contextual_callbacks() {
    let source = r#"local callbacks = {
---@type fun(value: number): number
map = function(value) return value end,
}
callbacks.map = function(replacement) return replacement + 1 end
local result = callbacks.map(2)
"#;
    let report = accepted(source);
    assert_eq!(binding(&report, "result"), "number");
    assert_eq!(report.type_at(source.find("function(value)").unwrap() + "function(".len()), Some("number"));
    assert_eq!(report.type_at(source.find("function(replacement)").unwrap() + "function(".len()), Some("number"));
    let bad = source.replacen("return value end", "return \"wrong\" end", 1);
    let report = check(&bad);
    let diagnostic = report.diagnostics.iter().find(|error| error.code == "TS2322").unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&bad[diagnostic.span.start..diagnostic.span.end], "\"wrong\"");
    assert!(report.diagnostics.iter().all(|error| error.code != "TS7006"));
    accepted(&bad.replace("\"wrong\"", "value"));
}

#[test]
fn generic_field_heads_keep_original_signature_scopes_and_sibling_instantiations() {
    let source = r#"local functions = {
---@type fun<T>(value: T): T
identity = function(value)
---@type T
local copy = value
return copy
end,
}
local numeric = functions.identity(42)
local textual = functions.identity("word")
local numeric_again = functions.identity(7)
"#;
    let report = accepted(source);
    assert_eq!(binding(&report, "numeric"), "number");
    assert_eq!(binding(&report, "textual"), "string");
    assert_eq!(binding(&report, "numeric_again"), "number");
}

#[test]
fn declared_field_values_preserve_outer_generic_and_immutable_callback_captures() {
    let source = r#"---@generic T
---@param input T
local function wrap(input)
    local fields = {
    ---@type T
    value = input,
    ---@type fun(): T
    read = function() return input end,
    }
    return fields
end
local numeric = wrap(42)
local textual = wrap("word")
---@type number
local numeric_value = numeric.value
---@type number
local numeric_call = numeric.read()
---@type string
local textual_value = textual.value
---@type string
local textual_call = textual.read()
"#;
    accepted(source);
    let bad = source.replace("local textual_value = textual.value", "local textual_value = numeric.value");
    assert!(check(&bad).diagnostics.iter().any(|error| error.code == "TS2322"));
}

#[test]
fn original_constructor_members_are_visible_on_class_instances_with_written_priority() {
    // Original overrides.lua TableExplicitField/TableImplicitField reductions.
    let source = r#"---@class Fields
local Fields = {
---@type 1
one = 1,
inferred = 1,
---@type fun(): number
read = function() return 1 end,
}
---@type Fields
local instance
---@type 1
local one = instance.one
---@type number
local ordinary = instance.inferred
---@type number
local call = instance.read()
instance.inferred = 2
instance.one = 1
local copied = instance.one
"#;
    let report = accepted(source);
    assert_eq!(binding(&report, "copied"), "1");
    let bad = format!("{source}instance.one = 2\n");
    assert!(check(&bad).diagnostics.iter().any(|error| error.code == "TS2322"));
    let forged = format!("{source}---@type Fields\nlocal fake = {{one=1,inferred=1,read=function() return 1 end}}\n");
    assert!(check(&forged).diagnostics.iter().any(|error| error.code == "TS2322"));
    accepted(r#"---@class Written
---@field value number
local Written = {
---@type 1
value = 1,
}
---@type Written
local instance
instance.value = 2
"#);
}

#[test]
fn exact_key_domains_and_sequence_pack_contracts_retain_declared_singletons() {
    let source = r#"local keys = {
---@type 1
[1] = 1,
---@type "word"
["1"] = "word",
}
keys[1] = 1
keys["1"] = "word"
---@return 1, "tail"
local function values() return 1, "tail" end
local list = {
---@type 1, "tail"
values()
}
---@type 1
local first = list[1]
---@type "tail"
local last = list[2]
"#;
    accepted(source);
    assert!(check(&format!("{source}keys[1] = 2\n")).diagnostics.iter().any(|error| error.code == "TS2322"));
    assert!(check(&format!("{source}list[2] = \"wrong\"\n")).diagnostics.iter().any(|error| error.code == "TS2322"));
}

#[test]
fn inferred_function_fields_keep_their_public_overload_heads() {
    let source = r#"local functions = {
---@overload fun(value: number): number
---@overload fun(value: string): string
---@param value any
---@return any
identity = function(value) return value end,
}

local numeric = functions.identity(1)
local textual = functions.identity("word")
"#;
    let report = accepted(source);
    assert_eq!(binding(&report, "numeric"), "number");
    assert_eq!(binding(&report, "textual"), "string");
}

#[test]
fn inherited_constructor_function_contexts_keep_sibling_generic_base_mappers() {
    let source = r#"---@alias Base<T> {callback:fun(value:T):T}
---@class Numeric:Base<number>
local Numeric={callback=function(value) return value+1 end}
---@class Textual:Base<string>
local Textual={callback=function(value) return value.."!" end}
---@type Numeric
local numeric
---@type Textual
local textual
---@type number
local number=numeric.callback(1)
---@type string
local string=textual.callback("word")
"#;
    let report = accepted(source);
    assert_eq!(report.type_at(source.find("function(value)").unwrap() + "function(".len()), Some("number"));
    assert_eq!(report.type_at(source.rfind("function(value)").unwrap() + "function(".len()), Some("string"));
    let bad = source.replace("return value+1", "return 'wrong'");
    let report = check(&bad);
    let failure = report.diagnostics.iter().find(|error| error.code == "TS2416").unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&bad[failure.span.start..failure.span.end], "function(value) return 'wrong' end");
    assert!(report.diagnostics.iter().all(|error| error.code != "TS7006"), "{:#?}", report.diagnostics);
    assert_eq!(report.type_at(bad.find("function(value)").unwrap() + "function(".len()), Some("number"));
    accepted(&bad.replace("return 'wrong'", "return value+1"));
}

#[test]
fn inferred_constructor_override_reports_the_original_value_once() {
    let source = "---@alias Base {value:number}\n---@class Derived:Base\nlocal Derived={value='wrong'}\n";
    let report = check(source);
    let failures: Vec<_> = report.diagnostics.iter().filter(|error| error.code == "TS2416").collect();
    assert_eq!(failures.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(&source[failures[0].span.start..failures[0].span.end], "'wrong'");
    assert!(report.diagnostics.iter().all(|error| error.code != "LUA2031"), "{:#?}", report.diagnostics);
    accepted(&source.replace("'wrong'", "42"));
}
#[test]
fn cli_checks_original_field_initializer_and_accepts_its_source_correction() {
    let directory = std::env::temp_dir().join(format!("luanalysis original field contracts {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("original fields.lua");
    let source = "local fields={\n---@type number\nvalue=\"wrong\",\n}\n";
    std::fs::write(&path, source).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis")).arg("check").arg(&path).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains(":3:7: error[TS2322]"), "{}", String::from_utf8_lossy(&output.stderr));
    std::fs::write(&path, source.replace("\"wrong\"", "1")).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis")).arg("check").arg(&path).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn alias_record_updates_keep_existing_numeric_and_named_contracts() {
    for (record, access) in [("{[1]:1,[2]:string[],[3]:number}", "[3]"), ("{count:number}", ".count"), ("number[]", "[3]")] {
        let source = format!("---@alias Record {record}\n---@param original Record\nlocal function update(original)\n local view=--[[---@type Record]] original\n view{access}=view{access}+1\n ---@type number\n local checked=view{access}\nend\n");
        accepted(&source);
        let wrong = source.replace(&format!("view{access}=view{access}+1"), &format!("view{access}='wrong'"));
        let report = check(&wrong);
        let errors: Vec<_> = report.diagnostics.iter().filter(|error| error.code == "TS2322").collect();
        assert_eq!(errors.len(), 1, "{wrong}: {:#?}", report.diagnostics);
        assert_eq!(&wrong[errors[0].span.start..errors[0].span.end], "'wrong'");
        assert!(report.diagnostics.iter().all(|error| !matches!(error.code, "TS2589" | "TS2300" | "LUA2032")), "{:#?}", report.diagnostics);
        accepted(&wrong.replace("'wrong'", "42"));
    }
}

#[test]
fn recursive_json_array_node_updates_validate_the_original_length_and_element_writes() {
    let source = "---@class JsonNull\nlocal NULL={}\n---@alias JsonObject table<string,JsonValue>\n---@alias JsonArray JsonValue[]\n---@alias JsonContainer JsonObject|JsonArray\n---@alias JsonValue JsonContainer|number|string|boolean|nil|JsonNull\n---@alias ArrayNode {[1]:1,[2]:JsonArray,[3]:number}\n---@param currentNode ArrayNode\n---@param value JsonValue\nlocal function addValue(currentNode,value)\n local arrayNode=--[[---@type ArrayNode]] currentNode\n local array=arrayNode[2]\n arrayNode[3]=arrayNode[3]+1\n array[arrayNode[3]]=value\nend\n";
    accepted(source);
    let wrong = source.replace("arrayNode[3]+1", "'wrong'");
    let report = check(&wrong);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"
        && &wrong[error.span.start..error.span.end] == "'wrong'"), "{:#?}", report.diagnostics);
    assert!(report.diagnostics.iter().all(|error| error.code != "TS2589"), "{:#?}", report.diagnostics);
    accepted(&wrong.replace("'wrong'", "42"));
}

#[test]
fn direct_declared_duplicates_still_fail_and_genuinely_new_definition_fields_are_inferred() {
    let duplicate = "---@alias Record {[3]:number,[3]:string}\n---@param value Record\nlocal function read(value) return value[3] end\n";
    let report = check(duplicate);
    assert!(report.diagnostics.iter().any(|error| matches!(error.code, "LUA2032" | "TS2300")
        && error.message.contains("Duplicate declared property 3")), "{:#?}", report.diagnostics);
    accepted(&duplicate.replacen("[3]:string", "[4]:string", 1));

    let constructor = "---@class Built\nlocal Built={}\nBuilt.count=1\n---@type number\nlocal checked=Built.count\n";
    accepted(constructor);
    let wrong = constructor.replace("---@type number\nlocal checked", "---@type string\nlocal checked");
    let report = check(&wrong);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"
        && &wrong[error.span.start..error.span.end] == "Built.count"), "{:#?}", report.diagnostics);
}

#[test]
fn cli_checks_an_alias_node_update_and_its_wrong_length_correction() {
    let directory = std::env::temp_dir().join(format!("luanalysis alias node update {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("node.lua");
    let source = "---@alias ArrayNode {[1]:1,[2]:string[],[3]:number}\n---@param currentNode ArrayNode\nlocal function addValue(currentNode)\n local arrayNode=--[[---@type ArrayNode]] currentNode\n arrayNode[3]=arrayNode[3]+1\n arrayNode[2][arrayNode[3]]='value'\nend\n";
    let run = || std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--format", "json", "--lua-version", "5.2"]).arg(&path).output().unwrap();
    std::fs::write(&path, source).unwrap();
    let output = run();
    assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let wrong = source.replace("arrayNode[3]+1", "'wrong'");
    std::fs::write(&path, &wrong).unwrap();
    let output = run();
    let report = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    let start = wrong.find("'wrong'").unwrap();
    assert_eq!(report.matches("\"code\":").count(), 1, "{report}");
    assert!(report.contains("\"code\":\"TS2322\"")
        && report.contains(&format!("\"range\":{{\"start\":{start},\"end\":{}}}", start + "'wrong'".len())), "{report}");
    std::fs::write(&path, source).unwrap();
    let output = run();
    assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    std::fs::remove_dir_all(directory).unwrap();
}
