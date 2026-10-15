use luanalysis::{CheckReport, check};

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}
fn ty(report: &CheckReport, name: &str) -> String {
    report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
        .clone()
}
#[test]
fn generic_dictionary_assertions_use_possible_key_overlap_and_keep_value_contracts() {
    let source = r#"---@shape Packed<T>
---@field [number] T
---@field n number

---@generic K,V
---@param tab table<K,V>
---@return V[]
local function array(tab) return --[[---@type V[] ]] tab end

---@generic K,V
---@param tab table<K,V>
---@return Packed<V>
local function packed(tab) return --[[---@type Packed<V>]] tab end

---@generic K,V
---@param tab V[]
---@return table<K,V>
local function dictionary(tab) return --[[---@type table<K,V>]] tab end
"#;
    accepted(source);

    let wrong = r#"---@generic K
---@param tab table<K,number>
local function wrong(tab) return --[[---@type string[] ]] tab end
"#;
    let report = check(wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2352");
    assert_eq!(&wrong[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "--[[---@type string[] ]] tab");
}

#[test]
fn dictionary_assertion_overlap_does_not_coerce_distinct_lua_key_categories() {
    for generic in ["", "---@generic K:string\n", "---@generic K:boolean\n"] {
        let key = if generic.is_empty() { "string" } else { "K" };
        let source = format!("{generic}---@param tab table<{key},number>\nlocal function wrong(tab) return --[[---@type number[] ]] tab end\n");
        let report = check(&source);
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, "TS2352");
    }
}

#[test]
fn possible_generic_dictionary_key_overlap_does_not_allow_ordinary_assignment() {
    let source = "---@generic K,V\n---@param tab table<K,V>\nlocal function wrong(tab)\n---@type V[]\nlocal values=tab\nend\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(&source[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "tab");
}

#[test]
fn scalar_type_assertion_and_not_preserve_operand_checking() {
    let source = "---@param value number|string\nlocal function read(value)\n---@type number\nlocal cast=--[[---@type number]] value\n---@type number\nlocal excluded=--[[---@not string]] value\nreturn cast,excluded\nend\nread(42)";
    accepted(source);
    let bad = source.replace("---@not string", "---@not number");
    let report = check(&bad);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let overlap = check("local result=--[[---@type string]] 42");
    assert_eq!(overlap.diagnostics.len(), 1, "{:#?}", overlap.diagnostics);
    assert_eq!(overlap.diagnostics[0].code, "TS2352");
    let invalid = check(
        "---@param value number\nlocal function use(value) end\nlocal result=--[[---@type number]] use(\"wrong\")",
    );
    assert!(
        invalid
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        invalid.diagnostics
    );
}
#[test]
fn whole_pack_assertions_and_exclusions_keep_slot_order_and_parentheses() {
    let source = "---@return number|string,number|string\nlocal function pair() return 1,\"value\" end\nlocal first,second=--[[---@type number,string]] pair()\nlocal notstring,notnumber=--[[---@not string,number]] pair()\nlocal single,absent=(--[[---@type number,string]] pair())";
    let report = accepted(source);
    for name in ["first", "notstring", "single"] {
        assert_eq!(ty(&report, name), "number");
    }
    for name in ["second", "notnumber"] {
        assert_eq!(ty(&report, name), "string");
    }
    assert_eq!(ty(&report, "absent"), "nil");
}
#[test]
fn assertion_does_not_rebind_or_globally_narrow_the_original_local() {
    let source = "---@param value number|nil\nlocal function read(value)\n---@type number\nlocal nonnil=--[[---@not nil]] value\n---@type number|nil\nlocal unchanged=value\nreturn unchanged\nend";
    accepted(source);
    let bad = source.replace(
        "---@type number|nil\nlocal unchanged",
        "---@type number\nlocal unchanged",
    );
    let report = check(&bad);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}
#[test]
fn inline_assertion_retains_original_field_receiver_for_assignment() {
    accepted(
        "---@param object {value:number}|nil\nlocal function write(object)\n;(--[[---@not nil]] object).value=42\nend",
    );
    let bad = check(
        "---@param object {value:number}|nil\nlocal function write(object)\n;(--[[---@not nil]] object).value=\"wrong\"\nend",
    );
    assert_eq!(bad.diagnostics.len(), 1, "{:#?}", bad.diagnostics);
    assert_eq!(bad.diagnostics[0].code, "TS2322");
}
#[test]
fn assertion_only_outer_type_parameter_is_captured_and_mapped() {
    let report = accepted(
        "---@generic T\n---@param value T\nlocal function outer(value)\n---@type any\nlocal hidden=value\nlocal function inner() return --[[---@type T]] hidden end\nreturn inner\nend\nlocal numeric=outer(42)\nlocal text=outer(\"word\")\nlocal a=numeric()\nlocal b=text()\nlocal again=numeric()",
    );
    assert_eq!(ty(&report, "a"), "number");
    assert_eq!(ty(&report, "b"), "string");
    assert_eq!(ty(&report, "again"), "number");
}

#[test]
fn fixed_and_variadic_exclusions_preserve_open_tail_and_zero_result_counts() {
    let report = accepted(
        "---@param flag boolean\n---@vararg number|string|nil\nlocal function forward(flag,...) return ... end\nlocal a,b,c=--[[---@not nil|string...]] forward(true,1,2,3)\n---@type number|nil\nlocal first=a\n---@type number|nil\nlocal second=b\n---@type number|nil\nlocal third=c",
    );
    assert_eq!(ty(&report, "first"), "number | nil");
    accepted(
        "local function none() end\nlocal function zero() end\nzero(--[[---@not nil]] none())",
    );
    accepted(
        "---@vararg nil\nlocal function onlynil(...) return ... end\nlocal function zero() end\nzero(--[[---@not nil...]] onlynil())",
    );
}

#[test]
fn typed_inline_function_receives_its_annotation_context() {
    accepted(
        "local callback=--[[---@type fun(value:number):number]] function(value) return value end\n---@type number\nlocal result=callback(42)",
    );
}

#[test]
fn cli_assertion_error_uses_original_unicode_crlf_source_and_correction() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis inline assertions {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("assertions.lua");
    let source = "---@param value number|string\r\nlocal function read(value)\r\n---@type number\r\nlocal é=--[[---@not number]] value\r\nreturn é\r\nend";
    fs::write(&path, source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .arg("check")
            .arg("--dump-types")
            .arg(&path)
            .output()
            .unwrap()
    };
    let failed = run();
    assert_eq!(failed.status.code(), Some(1));
    let errors = String::from_utf8_lossy(&failed.stderr);
    assert!(
        errors.contains("assertions.lua:4:30") && errors.contains("TS2322"),
        "{errors}"
    );
    fs::write(&path, source.replace("@not number", "@not string")).unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("é: number"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn asserted_table_context_types_nested_callbacks_and_preserves_body_errors() {
    let source = "local object=--[[---@type {callback:fun(value:number):number}]] {callback=function(value) return value end}\n---@type number\nlocal result=object.callback(42)";
    let report = accepted(source);
    assert_eq!(ty(&report, "value"), "number");
    assert_eq!(ty(&report, "result"), "number");
    let bad = check(&source.replace("return value", "return \"wrong\""));
    assert!(
        bad.diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2322" | "TS2352")),
        "{:#?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics.iter().all(|error| error.code != "TS7006"),
        "{:#?}",
        bad.diagnostics
    );
}

#[test]
fn narrower_exclusions_keep_any_in_scalar_and_result_pack_positions() {
    let report = accepted(
        "---@param value any\nlocal function scalar(value)\nlocal text=--[[---@not string]] value\nlocal falsy=--[[---@not false]] value\nlocal primitive=--[[---@not number|string]] value\nreturn text,falsy,primitive\nend\nlocal text,falsy,primitive=scalar(42)\n---@return any,any\nlocal function pair() return 42,true end\nlocal first,second=--[[---@not string,false]] pair()",
    );
    for name in ["text", "falsy", "primitive", "first", "second"] {
        assert_eq!(ty(&report, name), "any", "{name}");
    }
}

#[test]
fn top_type_exclusions_remove_any_in_scalar_and_result_pack_positions() {
    let report = accepted(
        "---@param value any\nlocal function scalar(value)\nlocal all=--[[---@not any]] value\nlocal unknown=--[[---@not unknown]] value\nreturn all,unknown\nend\nlocal all,unknown=scalar(42)\n---@return any,any\nlocal function pair() return 42,true end\nlocal first,second=--[[---@not unknown,any]] pair()",
    );
    for name in ["all", "unknown", "first", "second"] {
        assert_eq!(ty(&report, name), "never", "{name}");
    }
}

#[test]
fn fixed_nil_exclusion_agrees_with_scalar_projection_and_guarantees_slots() {
    let report = accepted(
        "---@vararg number|nil\nlocal function forward(...) return ... end\nlocal bare=--[[---@not nil]] forward(42)\nlocal parenthesized=(--[[---@not nil]] forward(42))\nlocal first,second,third=--[[---@not nil,nil,nil]] forward(1,2,3)\n---@type number\nlocal readfirst=first\n---@type number\nlocal readsecond=second\n---@type number\nlocal readthird=third",
    );
    for name in ["bare", "parenthesized", "first", "second", "third"] {
        assert_eq!(ty(&report, name), "number", "{name}");
    }
}

#[test]
fn fixed_nil_exclusion_promises_minimum_count_without_closing_the_tail() {
    let source = "---@vararg number|nil\nlocal function forward(...) return ... end\n---@param first number\n---@param second number\n---@param third number\n---@vararg number|nil\nlocal function three(first,second,third,...) end\nthree(--[[---@not nil,nil,nil]] forward(1,2,3))";
    accepted(source);
    let bad = check(&source.replace("@not nil,nil,nil", "@not nil,nil"));
    assert_eq!(bad.diagnostics.len(), 1, "{:#?}", bad.diagnostics);
    assert_eq!(bad.diagnostics[0].code, "TS2554");
    assert!(
        bad.diagnostics[0].message.contains("got at least 2"),
        "{}",
        bad.diagnostics[0].message
    );
}

#[test]
fn fixed_absence_exclusion_is_impossible_while_variadic_exclusion_can_be_empty() {
    let report = accepted(
        "local function zero() end\nlocal fixed=--[[---@not nil]] zero()\nlocal scalar=(--[[---@not nil]] zero())\n---@vararg nil\nlocal function nils(...) return ... end\nlocal impossible=--[[---@not nil,nil]] nils(nil,nil)\nlocal empty=--[[---@not nil...]] nils()\nzero(--[[---@not nil...]] nils())",
    );
    for name in ["fixed", "scalar", "impossible"] {
        assert_eq!(ty(&report, name), "never", "{name}");
    }
    assert_eq!(ty(&report, "empty"), "nil");
}

#[test]
fn first_type_assertion_has_legacy_precedence_over_exclusions() {
    // The legacy implementation returns the first @type immediately. It does
    // not apply @not tags on that expression afterward.
    let report = accepted(
        "---@param value number|string\nlocal function read(value)\nlocal cast=--[[---@type number|string]] --[[---@not string]] value\nreturn cast\nend\nlocal result=read(42)",
    );
    let displayed = ty(&report, "cast");
    assert!(displayed.contains("number") && displayed.contains("string"));
}
