//! Original Lua generic-for acceptance. Pending cases require successful native
//! checking or ordinary type errors; an unsupported diagnostic is not success.

use luanalysis::{CheckReport, check};

const LOCAL_ROW_HEADER: &str = r#"---@shape Packed<T>
---@field [number] T
---@field n number

---@function packedOnly(state:Packed<string>,control:number):nil|(number,string|nil)
---@function choose(): ((fun(state:Packed<string>,control:number):nil|(number,string|nil)),Packed<string>,0)|((fun(state:string[],control:number):nil|(number,string)),string[],0)
"#;

const LOCAL_ROW_LOOP: &str = r#"for key,value in step,state,control do
---@type number
local checkedKey=key
---@type nil|string
local checkedValue=value
end
"#;

#[test]
fn destructured_iterator_header_preserves_original_rows_before_a_future_write() {
    for suffix in ["", "state={'replacement after the loop'}"] {
        accepted(&format!("{LOCAL_ROW_HEADER}local step,state,control=choose()\n{LOCAL_ROW_LOOP}{suffix}"));
    }
}

#[test]
fn replacing_any_iterator_sibling_discards_the_original_row_relation() {
    for replacement in [
        "state={'replacement'}",
        "step=packedOnly",
        "control=1",
    ] {
        let source = format!("{LOCAL_ROW_HEADER}local step,state,control=choose()\n{replacement}\n{LOCAL_ROW_LOOP}");
        let report = check(&source);
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.code == "TS2345"),
            "{replacement}: {:#?}", report.diagnostics);
    }
}

#[test]
fn effects_between_frozen_header_reads_cannot_join_different_reached_values() {
    let source = format!(r#"{LOCAL_ROW_HEADER}local step,state,control=choose()
local function replaceState()
state={{'replacement without n'}}
return state
end
for key,value in step,replaceState(),control do end
"#);
    let report = check(&source);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.code == "TS2345"),
        "{:#?}", report.diagnostics);
}

#[test]
fn independent_original_iterator_groups_do_not_share_rows() {
    let source = format!("{LOCAL_ROW_HEADER}local step,state,control=choose()\nlocal otherStep,otherState,otherControl=choose()\nfor key,value in step,otherState,control do end");
    let report = check(&source);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.code == "TS2345"),
        "{:#?}", report.diagnostics);
}

#[test]
fn iterator_reconstruction_preserves_written_and_asserted_state_views() {
    let header = r#"---@function hidden(): (fun(state:{payload:string,hidden:number},control:number):nil|(number,string)),{payload:string,hidden:number},0

"#;
    for binding in [
        "---@type fun(state:{payload:string,hidden:number},control:number):nil|(number,string),{payload:string},number\nlocal step,state,control=hidden()\nfor key,value in step,state,control do end",
        "local step,state,control=hidden()\nfor key,value in step,--[[---@type {payload:string}]]state,control do end",
    ] {
        let report = check(&format!("{header}{binding}"));
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.code == "TS2345"),
            "{binding}: {:#?}", report.diagnostics);
    }
}

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

const CORRELATED_HEADER: &str = r#"---@param state {payload:string}
---@param control number
---@return nil|(number,string)
local function textIterator(state,control)
if control<1 then return 1,state.payload end
return nil
end
---@param state {payload:number}
---@param control string
---@return nil|(string,number)
local function numberIterator(state,control)
if control=="initial" then return "key",state.payload end
return nil
end
---@param ready boolean
---@return ((fun(state:{payload:string},control:number):nil|(number,string)),{payload:string},number)|((fun(state:{payload:number},control:string):nil|(string,number)),{payload:number},string)
local function header(ready)
if ready then return textIterator,{payload="text"},0 end
return numberIterator,{payload=42},"initial"
end
---@param ready boolean
local function use(ready)
for key,value in header(ready) do
if type(key)=="number" then
---@type string
local text=value
else
---@type number
local number=value
end
end
end"#;

#[test]
fn correlated_header_fixture_preserves_two_original_three_slot_rows() {
    let parsed = luanalysis_syntax::parse(CORRELATED_HEADER);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let annotation = parsed
        .statements
        .iter()
        .find_map(|statement| {
            let luanalysis_syntax::StmtKind::LocalFunction { name, function } = &statement.kind
            else {
                return None;
            };
            (name.text == "header").then(|| function.annotations.returns.as_ref().unwrap())
        })
        .unwrap();
    let packs = annotation.packs.as_ref().unwrap();
    assert_eq!(packs.alternatives.len(), 2);
    assert!(
        packs
            .alternatives
            .iter()
            .all(|row| row.fixed.len() == 3 && row.variadic.is_none())
    );
    assert_eq!(annotation.types.len(), 6);
}

#[test]
fn nil_only_termination_keeps_false_and_short_payload_rows() {
    let source = r#"---@param state nil
---@param control nil|boolean|number
---@return nil|false|(number,string)
local function iterator(state,control)
if control==nil then return false end
if control==false then return 7,"value" end
return nil
end
for key,payload in iterator,nil,nil do
if key==false then
---@type nil
local absent=payload
else
---@type number
local index=key
---@type string
local text=payload
end
end"#;
    accepted(source);
    let wrong = source.replace(
        "---@type nil\nlocal absent",
        "---@type string\nlocal absent",
    );
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322");
    assert_eq!(&wrong[error.span.start..error.span.end], "payload");
}

#[test]
fn iterator_header_is_frozen_before_body_reassigns_the_iterator() {
    accepted(
        r#"---@param state nil
---@param control number
---@return nil|(number,string)
local function first(state,control)
if control<1 then return 1,"first" end
return nil
end
---@param state nil
---@param control number
---@return nil|(number,number)
local function second(state,control)
if control<1 then return 1,42 end
return nil
end
---@type (fun(state:nil,control:number):nil|(number,string))|(fun(state:nil,control:number):nil|(number,number))
local iterator=first
local _,before=iterator(nil,0)
---@type nil|string
local before_text=before
for key,value in iterator,nil,0 do
iterator=second
---@type string
local original=value
end"#,
    );
}

#[test]
fn visible_loop_key_writes_do_not_replace_the_hidden_control() {
    let source = r#"---@param state nil
---@param control number
---@return nil|(number,string)
local function iterator(state,control)
if control==0 then return 1,"value" end
return nil
end
for key,value in iterator,nil,0 do
key=0
---@type number
local changed=key
---@type string
local original=value
end"#;
    accepted(source);
    let wrong = source.replace(
        "---@type number\nlocal changed",
        "---@type 1\nlocal changed",
    );
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322");
    assert_eq!(&wrong[error.span.start..error.span.end], "key");
}

#[test]
fn a_written_literal_iterator_key_rejects_an_incompatible_body_assignment() {
    let source = "---@param state nil\n---@param control 0|1\n---@return nil|(1,string)\nlocal function iterator(state,control)\nif control==0 then return 1,'value' end\nreturn nil\nend\nfor key,value in iterator,nil,0 do\nkey=0\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322");
    assert_eq!(&source[error.span.start..error.span.end], "0");
}

#[test]
fn header_alternatives_keep_the_callable_state_and_control_together() {
    accepted(CORRELATED_HEADER);
}

#[test]
fn an_any_expanded_control_keeps_the_known_iterator_and_payload_type() {
    let source = r#"---@param state nil
---@param control number
---@return nil|(number,string)
local function iterator(state,control)
if control<1 then return 1,"value" end
return nil
end
---@param dynamic any
local function use(dynamic)
for key,value in iterator,nil,dynamic() do
---@type number
local index=key
---@type string
local text=value
end
end"#;
    let report = accepted(source);
    assert_eq!(
        report.type_at(source.find("text=value").unwrap() + "text=".len()),
        Some("string")
    );
    let wrong = source.replace("---@type string\nlocal text", "---@type number\nlocal text");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322");
    assert_eq!(&wrong[error.span.start..error.span.end], "value");
}

#[test]
fn iterator_initial_control_mismatch_points_to_the_original_header_argument() {
    let source = r#"---@param state nil
---@param control number
---@return nil|(number,string)
local function iterator(state,control) return nil end
for key,value in iterator,nil,"bad" do end"#;
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2345");
    assert_eq!(&source[error.span.start..error.span.end], "\"bad\"");
    accepted(&source.replace("\"bad\"", "0"));
}

#[test]
fn iterator_results_must_fit_the_frozen_control_parameter_on_the_next_step() {
    let source = r#"---@param state nil
---@param control number
---@return nil|(false,string)
local function iterator(state,control) return false,"value" end
for key,value in iterator,nil,0 do end"#;
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2345");
    assert_eq!(&source[error.span.start..error.span.end], "iterator");
    assert!(error.message.contains("'false'") && error.message.contains("'number'"), "{error:?}");
    assert!(error.message.ends_with(" The previous first iterator result supplies the next control argument."), "{error:?}");
    assert!(!error.message.contains("contextual type"), "{error:?}");
    accepted(&source.replace(
        "---@param control number",
        "---@param control number|boolean",
    ));
}

#[test]
fn cli_iterator_payload_error_disappears_after_original_annotation_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis iterator CLI {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("iterator.lua");
    let source = "---@param state nil\n---@param control number\n---@return nil|(number,string)\n\
local function iterator(state,control)\nif control<1 then return 1,\"value\" end\nreturn nil\nend\n\
for key,value in iterator,nil,0 do\n---@type number\nlocal rejected=value\nend";
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
    let errors = String::from_utf8_lossy(&bad.stderr);
    assert!(
        errors.contains("iterator.lua:10:16") && errors.contains("TS2322"),
        "{errors}"
    );
    assert!(
        !errors.contains("LUA2023") && !errors.contains("LUA2020"),
        "{errors}"
    );
    fs::write(
        &path,
        source.replace("---@type number\n", "---@type string\n"),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("rejected: string"));
    fs::remove_dir_all(directory).unwrap();
}
