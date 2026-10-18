use luanalysis::check;
use std::{fs, process::Command};

const HEADER: &str = "---@param state {payload:number}\n---@param control number\n---@return nil|(number,number)\nlocal function step(state,control) return 1,state.payload end\n";
const DEAD_LOOP: &str = "---@param flag boolean\nlocal function visit(flag)\n if true then return end\n for key,value in step,{payload=42},0 do\n  if flag then local ignored=1 end\n  ---@type number\n  local checked=value\n end\nend\n";

#[test]
fn a_dead_continuation_keeps_original_iterator_rows_through_a_body_branch() {
    let report = check(&format!("{HEADER}{DEAD_LOOP}"));
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "value")
            .unwrap()
            .ty,
        "number"
    );
}

#[test]
fn dead_iterator_headers_and_bodies_still_check_state_control_and_element_types() {
    for (source, expected) in [
        (
            DEAD_LOOP.replace("step,{payload=42},0", "step,{payload='wrong'},0"),
            "TS2345",
        ),
        (
            DEAD_LOOP.replace("step,{payload=42},0", "step,{payload=42},'wrong'"),
            "TS2345",
        ),
        (
            DEAD_LOOP.replace("---@type number", "---@type string"),
            "TS2322",
        ),
    ] {
        let report = check(&format!("{HEADER}{source}"));
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        assert_eq!(report.diagnostics[0].code, expected);
        let corrected = check(&format!("{HEADER}{DEAD_LOOP}"));
        assert!(
            corrected.diagnostics.is_empty(),
            "{:#?}",
            corrected.diagnostics
        );
    }
}

#[test]
fn unconditional_body_returns_and_written_never_calls_do_not_pollute_iterator_feedback() {
    for body in ["if true then return end", "stop()"] {
        let source = format!(
            "{HEADER}---@return never\nlocal function stop() while true do end end\nlocal function visit()\n for key,value in step,{{payload=42}},0 do\n  {body}\n end\nend"
        );
        let report = check(&source);
        assert!(
            report.diagnostics.is_empty(),
            "{source}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn cli_standard_pairs_and_ipairs_keep_typed_rows_in_dead_continuations() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis unreachable iterator frames {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    for iterator in ["pairs", "ipairs"] {
        let source = format!(
            "---@param values number[]\n---@param flag boolean\nlocal function visit(values,flag)\n if true then return end\n for key,value in {iterator}(values) do\n  if flag then local ignored=1 end\n  ---@type number\n  local checked=value\n end\nend\n"
        );
        let run = |source: &str| {
            fs::write(&file, source).unwrap();
            Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
                .args(["check", "--lua-version", "5.2"])
                .arg(&file)
                .output()
                .unwrap()
        };
        let original = run(&source);
        assert!(
            original.status.success(),
            "{}",
            String::from_utf8_lossy(&original.stderr)
        );
        let wrong = run(&source.replace("---@type number", "---@type string"));
        assert_eq!(wrong.status.code(), Some(1));
        let text = String::from_utf8(wrong.stderr).unwrap();
        assert!(text.contains("error[TS2322]"), "{text}");
        assert!(
            !text.contains("LUA2020") && !text.contains("LUA2041"),
            "{text}"
        );
        let corrected = run(&source);
        assert!(
            corrected.status.success(),
            "{}",
            String::from_utf8_lossy(&corrected.stderr)
        );
    }
    fs::remove_dir_all(&directory).unwrap();
}
