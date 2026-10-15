//! Original-source overload contracts routed through native lazy signatures
//! and the existing TypeScript chooser, relations and inference workers.

use luanalysis::check;

fn accepted(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn declaration_heads_select_return_types_and_hide_the_implementation_signature() {
    let source = "---@overload fun(value:number):number\n---@overload fun(value:string):string\n\
---@param value number|string|boolean\n---@return number|string|boolean\n\
local function echo(value) return value end\n\
---@type number\nlocal number=echo(1)\n---@type string\nlocal string=echo(\"text\")";
    let accepted_report = check(source);
    assert!(
        accepted_report.diagnostics.is_empty(),
        "{:#?}",
        accepted_report.diagnostics
    );
    let original_name = source.find("echo(value)").unwrap();
    let displayed = accepted_report.type_at(original_name).unwrap();
    assert!(
        displayed.contains("fun(value: number): number"),
        "{displayed}"
    );
    assert!(
        displayed.contains("fun(value: string): string"),
        "{displayed}"
    );
    assert!(
        !displayed.contains("boolean"),
        "private implementation leaked: {displayed}"
    );
    let wrong = format!("{source}\nlocal rejected=echo(true)");
    let report = check(&wrong);
    assert!(!report.diagnostics.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| matches!(error.code, "TS2345" | "TS2769")),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| &wrong[error.span.start..error.span.end] == "true")
    );
}

#[test]
fn annotation_order_and_direct_literal_priority_follow_the_existing_ts_chooser() {
    accepted(
        r#"---@overload fun(value:number):"first"
---@overload fun(value:number):"second"
---@param value number
---@return "first"|"second"
local function tied(value) return "first" end
---@type "first"
local first=tied(1)
---@overload fun(value:string):"general"
---@overload fun(value:"x"):"special"
---@param value string
---@return "general"|"special"
local function literal(value)
if value=="x" then return "special" end
return "general"
end
---@type "special"
local special=literal("x")
---@overload fun(value:string):"general"
---@overload fun(value:("x")):"parenthesized"
---@param value string
---@return "general"|"parenthesized"
local function parenthesized(value) return "general" end
---@type "general"
local general=parenthesized("x")"#,
    );
}

#[test]
fn sibling_signature_generics_shadow_body_generics_without_crossing_constraints() {
    accepted(
        "---@overload fun<T:number>(value:T):T\n---@overload fun<T:string>(value:T):T\n\
---@generic T:boolean\n---@param value any\n---@return any\n\
local function identity(value) return value end\n\
---@type number\nlocal number=identity(1)\n---@type string\nlocal string=identity(\"text\")\n\
---@type number\nlocal repeated=identity(2)",
    );
}

#[test]
fn legacy_fixed_return_counts_feed_the_existing_expanded_argument_checker() {
    // Reduction of inspections/overloads.lua:88-160. Implementation bodies
    // remain explicitly typed; declared overload packs determine public calls.
    accepted(
        "---@overload fun(count:1):number\n---@overload fun(count:2):number,number\n\
---@param count number\n---@return number...\n\
local function count(count) return 1,2 end\n\
---@overload fun(a:number):boolean\n---@overload fun(a:number,b:number):number\n\
---@param a number\n---@param b? number\n---@return boolean|number\n\
local function consume(a,b) if b==nil then return true end return b end\n\
---@type boolean\nlocal one=consume(count(1))\n\
---@type number\nlocal two=consume(count(2))",
    );
}

#[test]
fn tableutils_map_reduction_infers_callback_and_return_per_signature() {
    // TableUtils.ttslua:70: the public array head has its own V/MappedV,
    // while the implementation keeps independent K/V/MappedV source scopes.
    accepted(
        "---@overload fun<V,MappedV>(tab:V[],callback:fun(value:V,key:number):MappedV):MappedV[]\n\
---@generic K,V,MappedV\n---@param tab table<K,V>\n\
---@param callback fun(value:V,key:K):MappedV\n---@return table<K,MappedV>\n\
local function map(tab,callback) return {} end\n\
---@type string[]\nlocal mapped=map({1,2},function(value,key)\n\
---@type number\nlocal input=value\n---@type number\nlocal index=key\nreturn \"mapped\"\nend)",
    );
}

#[test]
fn vector_constructor_callable_shape_uses_explicit_declaration_heads() {
    // Vector3.ttslua:39-43: reduce the separate static/instance nominal
    // declarations to a structural callable shape returning a record.
    accepted(
        "---@param factory CallableVector\n\
local function construct(factory)\n\
---@type Vector\nlocal zero=factory()\n\
---@type Vector\nlocal vector=factory(1,2)\nend\n\n\
---@shape Vector\n---@field x number\n---@field y number\n\n\
---@shape CallableVector\n---@overload fun():Vector\n\
---@overload fun(x:number,y:number):Vector",
    );
}

#[test]
fn cli_overload_argument_error_disappears_after_original_call_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis overload CLI {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("overloads.lua");
    let source = "---@overload fun(value:number):number\n---@overload fun(value:string):string\n\
---@param value number|string|boolean\n---@return number|string|boolean\n\
local function echo(value) return value end\nlocal rejected=echo(true)";
    fs::write(&path, source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .arg("check")
            .arg("--dump-types")
            .arg(&path)
            .output()
            .unwrap()
    };
    let bad = run();
    assert_eq!(bad.status.code(), Some(1));
    let output = String::from_utf8_lossy(&bad.stdout);
    let declaration = output.lines().find(|line| line.contains("echo: ")).unwrap();
    assert!(
        declaration.contains("fun(value: number): number"),
        "{declaration}"
    );
    assert!(
        declaration.contains("fun(value: string): string"),
        "{declaration}"
    );
    assert!(
        !declaration.contains("boolean"),
        "private implementation leaked: {declaration}"
    );
    let errors = String::from_utf8_lossy(&bad.stderr);
    assert!(
        errors.contains("TS2345") || errors.contains("TS2769"),
        "{errors}"
    );
    assert!(
        !errors.contains("LUA1001") && !errors.contains("LUA201"),
        "{errors}"
    );
    fs::write(&path, source.replace("echo(true)", "echo(1)")).unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("rejected: number"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn implementation_incompatibility_uses_the_original_overload_head() {
    let source = "---@overload fun(value:string):string\n---@param value number\n---@return number\nlocal function only_number(value) return value end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2394");
    assert!(source[error.span.start..error.span.end].contains("fun(value:string):string"));
}

#[test]
fn unused_heads_are_checked_under_their_own_signature_scope() {
    let source = "---@overload fun<T:Missing>(value:T):T\n---@param value any\n---@return any\nlocal function unused(value) return value end";
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2304"
            && &source[error.span.start..error.span.end] == "Missing"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| error.code == "TS2304"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn standalone_and_nested_generic_type_heads_have_original_signature_scopes() {
    accepted(
        r#"---@param identity fun<T>(value:T):T
local function use(identity)
---@type number
local number=identity(1)
---@type string
local string=identity("text")
end
---@generic T:number
---@param callback fun<T:string>(value:T):T
---@param input T
---@return T
local function outer(callback,input)
---@type string
local result=callback("text")
return input
end

---@alias Identity fun<T>(value:T):T
---@param identity Identity
local function alias_use(identity)
---@type number
local number=identity(1)
---@type string
local string=identity("text")
end"#,
    );
}

#[test]
fn captured_outer_generic_maps_each_overload_head_without_sibling_contamination() {
    accepted(
        r#"---@generic T
---@param value T
local function make(value)
---@overload fun():T
---@return T
local function get() return value end
return get
end
local number_get=make(1)
local string_get=make("text")
---@type number
local number=number_get()
---@type string
local string=string_get()
---@type number
local repeated=make(2)()"#,
    );
}

#[test]
fn nil_is_a_direct_literal_but_literal_aliases_and_unions_are_not() {
    accepted(
        r#"---@overload fun(value:any):"general"
---@overload fun(value:nil):"nil"
---@param value any
---@return "general"|"nil"
local function nil_literal(value) return "general" end
---@type "nil"
local specific=nil_literal(nil)
---@overload fun(value:string):"general"
---@overload fun(value:X):"alias"
---@param value string
---@return "general"|"alias"
local function alias_literal(value) return "general" end
---@type "general"
local general=alias_literal("x")
---@overload fun(value:string):"general"
---@overload fun(value:"x"|"y"):"union"
---@param value string
---@return "general"|"union"
local function union_literal(value) return "general" end
---@type "general"
local union=union_literal("x")

---@alias X "x""#,
    );
}

#[test]
fn standalone_generic_local_function_annotation_infers_repeated_calls() {
    accepted(
        r#"---@type fun<T>(value:T):T
local identity=function(value)
---@type T
local copy=value
return copy
end
---@type number
local number=identity(1)
---@type string
local string=identity("text")
---@type number
local repeated=identity(2)"#,
    );
}

#[test]
fn missing_and_non_generic_names_report_distinct_original_annotation_errors() {
    for (annotation, code, name) in [
        ("Missing<string>", "TS2304", "Missing"),
        ("number<string>", "TS2315", "number"),
    ] {
        let source = format!("---@type {annotation}\nlocal value=nil");
        let report = check(&source);
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, code);
        assert_eq!(
            &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
            name
        );
    }
}

#[test]
fn named_callable_heads_keep_their_own_generics_in_array_and_callback_annotations() {
    accepted(
        r#"---@param factory Factory
local function use(factory)
---@type number[]
local numbers=factory({1,2},function(value) return value end)
---@type string[]
local strings=factory({"text"},function(value) return value end)
end

---@shape Factory
---@overload fun<T>(values:T[],callback:fun(value:T):T):T[]"#,
    );
}

#[test]
fn generic_callable_fields_keep_nested_table_heads_in_the_signature_scope() {
    accepted(
        r#"---@param box Box
local function use(box)
---@type number[]
local numbers=box.map({1,2})
---@type string[]
local strings=box.map({"text"})
end

---@shape Box
---@field map fun<T>(values:T[]):T[]"#,
    );
}

#[test]
fn documented_anonymous_table_functions_expose_only_their_original_overload_heads() {
    let source=r#"local callbacks={
---@overload fun(value:number):number
---@overload fun(value:string):string
---@param value number|string|boolean
---@return number|string|boolean
echo=function(value)return value end,
---@overload fun(value:number):"first"
---@param value number
---@return "first"|"second"
choose=
---@overload fun(value:number):"second"
function(value)return "first" end
}
---@type number
local number=callbacks.echo(1)
---@type string
local string=callbacks.echo("text")
---@type "first"
local first=callbacks.choose(1)"#;
    accepted(source);
    let bad=format!("{source}\nlocal rejected=callbacks.echo(true)");
    let report=check(&bad);
    assert_eq!(report.diagnostics.len(),1,"{:#?}",report.diagnostics);
    assert_eq!(report.diagnostics[0].code,"TS2345");
    assert_eq!(&bad[report.diagnostics[0].span.start..report.diagnostics[0].span.end],"true");
}

#[test]
fn inline_assertion_generic_heads_keep_the_original_creation_site_scope() {
    accepted(r#"---@generic T
---@param input T
local function outer(input)
local identity=--[[---@type fun<U>(value:U):U]] function(value)return value end
---@type T
local copied=identity(input)
return copied
end
---@type number
local number=outer(1)
---@type string
local string=outer("text")"#);
}

#[test]
fn project_cli_routes_original_library_signature_keys_without_cross_file_scope_loss() {
    use std::{fs,process::Command};
    let directory=std::env::temp_dir().join(format!("luanalysis signature library {}",std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let library=directory.join("callable types.lua");
    let main=directory.join("usage.lua");
    fs::write(&library,"---@alias Identity fun<T>(value:T):T").unwrap();
    let source="---@param identity Identity\nlocal function use(identity)\n---@type number\nlocal number=identity(1)\n---@type string\nlocal string=identity(\"text\")\n---@type boolean\nlocal rejected=identity(2)\nend";
    fs::write(&main,source).unwrap();
    let run=||Command::new(env!("CARGO_BIN_EXE_luanalysis")).arg("check").arg("--library").arg(&library).arg("--dump-types").arg(&main).output().unwrap();
    let bad=run();
    assert_eq!(bad.status.code(),Some(1),"{}",String::from_utf8_lossy(&bad.stderr));
    let errors=String::from_utf8_lossy(&bad.stderr);
    assert!(errors.contains("usage.lua:8:16: error[TS2322]"),"{errors}");
    assert!(!errors.contains("TS2304")&&!errors.contains("panicked"),"{errors}");
    fs::write(&main,source.replace("---@type boolean","---@type number")).unwrap();
    let corrected=run();
    assert_eq!(corrected.status.code(),Some(0),"{}",String::from_utf8_lossy(&corrected.stderr));
    let output=String::from_utf8_lossy(&corrected.stdout);
    assert!(output.contains("number: number")&&output.contains("string: string")&&output.contains("rejected: number"),"{output}");
    fs::remove_dir_all(directory).unwrap();
}
