use luanalysis::{CheckOptions, CheckReport, check_with_options};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const CONSUME: &str = r#"---@param callback fun(value:number)
local function consume(callback)
    ---@type number, string, boolean
    local numeric, textual, flag = callback(42)
end
"#;

const DECLARED: &str = r#"---@type fun(value:number)
local declared
local first, second, third = declared(42)
local scalar = (declared(42))
---@type number, string, boolean
local numeric, textual, flag = declared(42)
"#;

fn report(source: &str, no_implicit_any: bool) -> CheckReport {
    check_with_options(
        source,
        CheckOptions {
            no_implicit_any,
            ..Default::default()
        },
    )
}

fn clean(source: &str, no_implicit_any: bool) -> CheckReport {
    let report = report(source, no_implicit_any);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn binding<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap_or_else(|| panic!("missing {name:?}: {:#?}", report.bindings))
        .ty
}

#[test]
fn omitted_callback_results_accept_scalar_and_multiple_returning_bodies() {
    // Reading three independently typed slots makes the omitted contract's
    // unconstrained pack observable; treating it as void would supply nil.
    for callback in [
        "function(value) return value end",
        "function(value) return 'text' end",
        "function(value) return value, 'tail' end",
    ] {
        let source = format!("{CONSUME}consume({callback})\n");
        for no_implicit_any in [false, true] {
            let report = clean(&source, no_implicit_any);
            let parameter = source.find("function(value)").unwrap() + "function(".len();
            assert_eq!(report.type_at(parameter), Some("number"));
        }
    }
}

#[test]
fn calling_an_omitted_result_contract_produces_any_in_every_pack_slot() {
    for source in [
        DECLARED.to_string(),
        DECLARED.replace(
            "---@type fun(value:number)\nlocal declared",
            "---@function declared(value:number)",
        ),
    ] {
        for no_implicit_any in [false, true] {
            let report = clean(&source, no_implicit_any);
            for name in ["first", "second", "third", "scalar"] {
                assert_eq!(binding(&report, name), "any", "binding {name}");
            }
            assert_eq!(binding(&report, "numeric"), "number");
            assert_eq!(binding(&report, "textual"), "string");
            assert_eq!(binding(&report, "flag"), "boolean");
        }
    }
}

#[test]
fn an_omitted_source_result_cannot_satisfy_a_written_result_contract() {
    for declaration in [
        "---@function unspecified()\n",
        "---@type fun()\nlocal unspecified\n",
    ] {
        for result in [
            "number",
            "any",
            "void",
            "()",
            "(number,string)",
            "number...",
        ] {
            let source =
                format!("{declaration}---@type fun():{result}\nlocal rejected = unspecified\n");
            for no_implicit_any in [false, true] {
                let rejected = report(&source, no_implicit_any);
                assert_eq!(
                    rejected.diagnostics.len(),
                    1,
                    "{source}\n{:#?}",
                    rejected.diagnostics
                );
                let error = &rejected.diagnostics[0];
                assert_eq!(error.code, "TS2322");
                assert_eq!(&source[error.span.start..error.span.end], "unspecified");
            }
        }
    }
}

#[test]
fn omitted_targets_accept_written_results_and_written_any_keeps_ordinary_relations() {
    for no_implicit_any in [false, true] {
        clean(
            "---@function unspecified()\n---@type fun()\nlocal accepted = unspecified\n",
            no_implicit_any,
        );
        for result in [
            "number",
            "any",
            "void",
            "()",
            "(number,string)",
            "number...",
        ] {
            clean(
                &format!(
                    "---@function written():{result}\n---@type fun()\nlocal accepted = written\n"
                ),
                no_implicit_any,
            );
        }
        // An explicitly written any result is not the omitted-result marker.
        clean(
            "---@function written():any\n---@type fun():number\nlocal accepted = written\n",
            no_implicit_any,
        );
    }
}

#[test]
fn omitted_target_results_preserve_parameter_contravariance() {
    let source = "---@function unspecified(value:string)\n---@type fun(value:number)\nlocal rejected = unspecified\n";
    for no_implicit_any in [false, true] {
        let rejected = report(source, no_implicit_any);
        assert_eq!(rejected.diagnostics.len(), 1, "{:#?}", rejected.diagnostics);
        let error = &rejected.diagnostics[0];
        assert_eq!(error.code, "TS2322");
        assert_eq!(&source[error.span.start..error.span.end], "unspecified");
        clean(
            &source.replace("value:string", "value:number"),
            no_implicit_any,
        );
    }
}

#[test]
fn generic_graph_callback_infers_its_actual_record_result_while_visit_results_are_open() {
    let source = r#"---@alias Graph<T> {value:T}
---@generic T,R
---@param graph Graph<T>
---@param callback fun(value:T):R
---@return R
local function project(graph, callback) return callback(graph.value) end
---@generic T
---@param graph Graph<T>
---@param visitor fun(value:T)
local function inspect(graph, visitor)
    return project(graph, function(value)
        ---@type number, string
        local arbitrary_number, arbitrary_string = visitor(value)
        return {value=value}
    end)
end
---@type Graph<number>
local graph = {value=42}
local result = inspect(graph, function(value) return value, 'ignored' end)
local inferred = result.value
---@type number
local numeric = inferred
"#;
    for no_implicit_any in [false, true] {
        let report = clean(source, no_implicit_any);
        assert_eq!(binding(&report, "inferred"), "number");
        let read = source.find("local inferred = result.value").unwrap()
            + "local inferred = result.".len();
        assert_eq!(report.type_at(read), Some("number"));
    }
}

#[test]
fn omitted_results_do_not_waive_callback_parameter_contracts() {
    let source = format!(
        "{CONSUME}---@param value string\nlocal function callback(value) return value end\nconsume(callback)\n"
    );
    for no_implicit_any in [false, true] {
        let rejected = report(&source, no_implicit_any);
        assert!(
            rejected.diagnostics.iter().any(|error| {
                error.code == "TS2345" && &source[error.span.start..error.span.end] == "callback"
            }),
            "{:#?}",
            rejected.diagnostics
        );
        assert!(
            rejected
                .diagnostics
                .iter()
                .all(|error| !matches!(error.code, "TS7006" | "TS7019")),
            "{:#?}",
            rejected.diagnostics
        );
        clean(
            &source.replace("---@param value string", "---@param value number"),
            no_implicit_any,
        );
    }
}

#[test]
fn explicit_void_and_empty_result_contracts_still_scalarize_to_nil() {
    for annotation in ["void", "()"] {
        let source = format!(
            "---@type fun():{annotation}\nlocal declared\nlocal scalar = declared()\nlocal first, second = declared()\n---@type number\nlocal rejected = declared()\n"
        );
        for no_implicit_any in [false, true] {
            let report = report(&source, no_implicit_any);
            assert_eq!(
                report.diagnostics.len(),
                1,
                "{source}\n{:#?}",
                report.diagnostics
            );
            let error = &report.diagnostics[0];
            assert_eq!(error.code, "TS2322");
            assert_eq!(&source[error.span.start..error.span.end], "declared()");
            for name in ["scalar", "first", "second"] {
                assert_eq!(
                    binding(&report, name),
                    "nil",
                    "{annotation}, binding {name}"
                );
            }
        }
    }
}

#[test]
fn written_scalar_tuple_and_variadic_results_retain_their_precise_contracts() {
    for (annotation, destination) in [
        ("number", "string"),
        ("(number,string)", "number,number"),
        ("number...", "string"),
    ] {
        let source = format!(
            "---@type fun():{annotation}\nlocal declared\nlocal first, second, third = declared()\n---@type {destination}\nlocal rejected, other = declared()\n"
        );
        for no_implicit_any in [false, true] {
            let report = report(&source, no_implicit_any);
            assert!(
                report.diagnostics.iter().any(|error| error.code == "TS2322"
                    && &source[error.span.start..error.span.end] == "declared()"),
                "{source}\n{:#?}",
                report.diagnostics
            );
            match annotation {
                "number" => {
                    assert_eq!(binding(&report, "first"), "number");
                    assert_eq!(binding(&report, "second"), "nil");
                    assert_eq!(binding(&report, "third"), "nil");
                }
                "(number,string)" => {
                    assert_eq!(binding(&report, "first"), "number");
                    assert_eq!(binding(&report, "second"), "string");
                    assert_eq!(binding(&report, "third"), "nil");
                }
                _ => {
                    for name in ["first", "second", "third"] {
                        let ty = binding(&report, name);
                        assert!(
                            ty.contains("number") && ty.contains("nil") && !ty.contains("any"),
                            "{name}: {ty}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn runtime_bodies_without_result_annotations_still_infer_empty_and_fixed_packs() {
    let source = "local function empty() end\nlocal function pair() return 42, 'tail' end\n---@type fun():()\nlocal typed_empty = empty\n---@type fun():(number,string)\nlocal typed_pair = pair\nlocal missing, also_missing = typed_empty()\nlocal first, second, third = typed_pair()\n";
    for no_implicit_any in [false, true] {
        let report = clean(source, no_implicit_any);
        for name in ["missing", "also_missing", "third"] {
            assert_eq!(binding(&report, name), "nil", "binding {name}");
        }
        assert_eq!(binding(&report, "first"), "number");
        assert_eq!(binding(&report, "second"), "string");
    }
}

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis omitted function results {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write_roots(&self, source: &str) {
        // One warmup task leaves four independent files for the parallel step.
        for name in [
            "first.lua",
            "second.lua",
            "third.lua",
            "fourth.lua",
            "fifth.lua",
        ] {
            fs::write(self.0.join(name), source).unwrap();
        }
    }
    fn cli(&self, jobs: usize, no_implicit_any: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis"));
        command
            .current_dir(&self.0)
            .args([
                "check",
                "--no-stdlib",
                "--format",
                "json",
                "--dump-types",
                "--jobs",
            ])
            .arg(jobs.to_string());
        if no_implicit_any {
            command.arg("--no-implicit-any");
        }
        command.arg(".").output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn json(output: Output, exit: i32) -> String {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(
        report.starts_with("{\"formatVersion\":1,") && report.ends_with("}\n"),
        "{report}"
    );
    assert!(report.contains("\"filesChecked\":5"), "{report}");
    report
}

#[test]
fn actual_cli_preserves_open_results_and_parameter_failures_with_one_two_and_four_workers() {
    let fixture = Fixture::new();
    let good = format!("{CONSUME}{DECLARED}consume(function(value) return value, 'tail' end)\n");
    let bad = good.replace("declared(42)", "declared('wrong')");
    for (source, exit) in [(&good, 0), (&bad, 1), (&good, 0)] {
        fixture.write_roots(source);
        let mut previous = None;
        for no_implicit_any in [false, true] {
            for jobs in [1, 2, 4] {
                let report = json(fixture.cli(jobs, no_implicit_any), exit);
                if exit == 0 {
                    assert!(
                        report.contains("\"success\":true")
                            && report.contains("\"diagnostics\":[]"),
                        "{report}"
                    );
                    assert!(
                        report.contains("\"name\":\"third\",\"type\":\"any\""),
                        "{report}"
                    );
                } else {
                    assert!(report.contains("\"code\":\"TS2345\""), "{report}");
                    assert!(
                        !report.contains("TS7006") && !report.contains("TS7019"),
                        "{report}"
                    );
                }
                if let Some(previous) = previous {
                    assert_eq!(
                        report, previous,
                        "jobs={jobs}, no_implicit_any={no_implicit_any}"
                    );
                }
                previous = Some(report);
            }
        }
    }
}
