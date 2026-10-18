//! Numeric range expectations were checked against local Lua 5.4 runtime
//! probes: inclusive bounds, signed/zero steps, integer/float selection,
//! header freezing, and writes to the visible variable versus hidden counter.
use luanalysis::{CheckReport, bind, check, flow};
use std::{fs, process::Command};

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
        .unwrap_or_else(|| panic!("missing {name}: {report:#?}"))
        .ty
}

#[test]
fn nonempty_signed_inclusive_ranges_guarantee_the_first_body_return() {
    for controls in ["1,3", "3,1,-1", "1,1", "1,1,-1", "0.5,1.5,0.5", "1,0.5,-1"] {
        let source = format!(
            "local function first() for index={controls} do return index end end\nlocal result=first()\n---@type number\nlocal accepted=result"
        );
        let report = clean(&source);
        assert_eq!(ty(&report, "result"), "number", "{controls}");
        assert_eq!(
            report.type_at(source.find("return index").unwrap() + 7),
            Some("number")
        );
    }
}

#[test]
fn direction_mismatched_ranges_exclude_body_returns() {
    for controls in ["3,1", "1,3,-1", "1,0.5", "-3,-1,-1"] {
        let source = format!(
            "local function empty() for index={controls} do return index end return \"empty\" end\nlocal result=empty()\n---@type string\nlocal accepted=result"
        );
        let report = clean(&source);
        assert_eq!(ty(&report, "result"), "string", "{controls}");
    }
}

#[test]
fn lua_integer_precision_and_float_loop_selection_are_distinct() {
    // Lua's integer loop is empty for these adjacent integers, while an
    // explicit float step converts both controls to the same rounded float.
    for (controls, expected) in [
        ("9007199254740993,9007199254740992", "string"),
        ("9007199254740993,9007199254740992,1.0", "number"),
        ("-9007199254740993,-9007199254740992,-1", "string"),
        ("-9007199254740993,-9007199254740992,-1.0", "number"),
        ("0xffffffffffffffff,0", "number"),
    ] {
        let source = format!(
            "local function selected() for index={controls} do return 1 end return \"empty\" end\nlocal result=selected()\n---@type {expected}\nlocal accepted=result"
        );
        let report = clean(&source);
        assert_eq!(ty(&report, "result"), expected, "{controls}");
    }
}

#[test]
fn finite_loop_continuations_reach_following_statements() {
    let source = "local function after() for index=1,3 do local copied=index end return \"done\" end\nlocal result=after()\n---@type string\nlocal accepted=result";
    let report = clean(source);
    assert_eq!(ty(&report, "result"), "string");
}

#[test]
fn finite_loop_without_a_return_keeps_annotated_fallthrough_error() {
    let source = "---@return number\nlocal function missing() for index=1,3 do local copied=index end end\nmissing()";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2355");
    assert!(report.diagnostics[0].span.start < source.find("local function").unwrap());
}

#[test]
fn definitely_entered_loop_writes_do_not_join_a_zero_iteration_path() {
    let source = "local function changed()\n---@type number|string\nlocal value=1\nfor index=1,3 do value=\"changed\" end\nreturn value\nend\nlocal result=changed()\n---@type string\nlocal accepted=result";
    let report = clean(source);
    assert_eq!(ty(&report, "result"), "string");
}

#[test]
fn empty_loop_writes_do_not_change_the_following_value() {
    let source = "local function unchanged()\n---@type number|string\nlocal value=1\nfor index=3,1 do value=\"unreachable\" end\nreturn value\nend\nlocal result=unchanged()\n---@type number\nlocal accepted=result";
    let report = clean(source);
    assert_eq!(ty(&report, "result"), "number");
}

#[test]
fn unknown_bounds_keep_body_and_zero_iteration_paths() {
    let source = "---@param bound number\nlocal function maybe(bound)\n---@type number|string\nlocal value=1\nfor index=1,bound do value=\"changed\" end\nreturn value\nend\nlocal result=maybe(3)\n---@type number|string\nlocal accepted=result";
    let report = clean(source);
    let result = ty(&report, "result");
    assert!(
        result.contains("number") && result.contains("string"),
        "{result}"
    );
}

#[test]
fn loop_header_names_see_outer_binding_and_body_names_see_numeric_local() {
    let source = "local index=7\nfor index=index,index+1 do\n---@type number\nlocal copied=index\nindex=99\nend\n---@type number\nlocal after=index";
    let report = clean(source);
    let bound = report.bound.as_ref().unwrap();
    let outer = bound.binding_at(source.find("index=7").unwrap()).unwrap();
    let declaration = bound
        .binding_at(source.find("for index").unwrap() + 4)
        .unwrap();
    let header_read = bound
        .binding_at(source.find("index=index").unwrap() + 6)
        .unwrap();
    let body_read = bound
        .binding_at(source.find("copied=index").unwrap() + 7)
        .unwrap();
    assert_ne!(outer, declaration);
    assert_eq!(outer, header_read);
    assert_eq!(declaration, body_read);
    assert_eq!(ty(&report, "copied"), "number");
    assert_eq!(ty(&report, "after"), "number");
}

#[test]
fn numeric_variable_writes_use_number_constraints() {
    let source = "for index=1,3 do index=\"bad\" end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "\"bad\"");
}

#[test]
fn break_reaches_exit_without_a_next_iteration() {
    let source = "local function stopped() for index=1,3 do break end return \"done\" end\nlocal result=stopped()\n---@type string\nlocal accepted=result";
    let report = clean(source);
    assert_eq!(ty(&report, "result"), "string");
}

#[test]
fn header_type_errors_keep_original_nonnumber_ranges() {
    // Lua 5.4 coerces numeric strings at runtime; the chosen TS-default typing
    // rule deliberately requires Number, as for the existing numeric operators.
    for (controls, invalid) in [
        ("false,3", "false"),
        ("1,nil", "nil"),
        ("1,3,\"1\"", "\"1\""),
    ] {
        let source = format!("for index={controls} do local copied=index end");
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}\n{:#?}",
            report.diagnostics
        );
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "TS2322");
        assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], invalid);
    }
}

#[test]
fn literal_zero_steps_report_a_single_original_step_error() {
    for step in ["0", "(-0)", "0.0", "-0.0"] {
        let source = format!("for index=1,3,{step} do local copied=index end");
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}\n{:#?}",
            report.diagnostics
        );
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "LUA2030");
        assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], step);
    }
}

#[test]
fn a_typed_zero_step_name_reports_the_same_runtime_error_gate() {
    let source = "---@param step 0\nlocal function invalid(step) for index=1,3,step do local copied=index end end\ninvalid(0)";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "LUA2030");
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "step");
    assert_eq!(diagnostic.span.start, source.find("1,3,step").unwrap() + 4);
}

#[test]
fn a_structural_literal_step_type_does_not_claim_lua_integer_float_identity() {
    let source = "---@param step 1\nlocal function selected(step)\nfor index=9007199254740993,9007199254740992,step do return 1 end\nreturn \"empty\"\nend\nlocal result=selected(1)\n---@type number|string\nlocal accepted=result";
    let report = clean(source);
    let result = ty(&report, "result");
    assert!(
        result.contains("number") && result.contains("string"),
        "{result}"
    );
}

#[test]
fn graph_retains_one_frozen_header_set_and_separate_first_next_predicates() {
    let file = luanalysis_syntax::parse("local bound=3\nfor index=1,bound,-1 do bound=0 end");
    assert!(file.diagnostics.is_empty());
    let bound = bind::bind(&file);
    let graph = flow::build(&file, &bound);
    let mut predicates = Vec::new();
    for node in &graph.graphs[0].nodes {
        if let flow::NodeKind::Predicate {
            predicate:
                flow::Predicate::NumericIteration {
                    start,
                    limit,
                    step,
                    stage,
                },
            sense,
        } = &node.kind
        {
            predicates.push((*start, *limit, *step, *stage, *sense));
        }
    }
    assert_eq!(predicates.len(), 4);
    assert!(
        predicates.iter().all(|entry| (entry.0, entry.1, entry.2)
            == (predicates[0].0, predicates[0].1, predicates[0].2))
    );
    for stage in [
        flow::NumericIterationStage::First,
        flow::NumericIterationStage::Next,
    ] {
        assert!(predicates.iter().any(|entry| entry.3 == stage && entry.4));
        assert!(predicates.iter().any(|entry| entry.3 == stage && !entry.4));
    }
    let header = graph.graphs[0]
        .nodes
        .iter()
        .find(|node| matches!(node.kind, flow::NodeKind::Loop))
        .unwrap();
    for evaluation in [predicates[0].0, predicates[0].1, predicates[0].2.unwrap()] {
        assert!(graph.evaluation(evaluation).completed.index < header.id.index);
    }
    assert!(
        !graph
            .pending
            .iter()
            .any(|feature| feature.kind == flow::PendingFeature::NumericIteration)
    );
}

#[test]
fn cli_rejects_zero_step_then_accepts_nonzero_correction() {
    let directory =
        std::env::temp_dir().join(format!("luanalysis numeric for {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("numeric.ttslua");
    let source = "for i=1,2,0 do\n---@type number\nlocal copied=i\nend\n";
    fs::write(&file, source).unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .args(["check", "--dump-types"])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1));
    let stderr = String::from_utf8(invalid.stderr).unwrap();
    assert!(
        stderr.contains("numeric.ttslua:1:11: error[LUA2030]"),
        "{stderr}"
    );
    assert!(!stderr.contains("LUA2023"), "{stderr}");
    fs::write(&file, source.replace("1,2,0", "1,2,1")).unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .arg("check")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert_eq!(
        String::from_utf8(corrected.stdout).unwrap(),
        "No type errors in 1 file(s).\n"
    );
    fs::remove_dir_all(directory).unwrap();
}
