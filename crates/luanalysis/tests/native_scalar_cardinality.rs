//! Scalar type contracts and physical Lua result cardinality are distinct.
use luanalysis::{CheckReport, check};

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}
fn rejected(source: &str, code: &str) {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, code);
}
#[test]
fn mixed_nil_return_and_fallthrough_does_not_guarantee_a_physical_argument() {
    let declarations = "---@param flag boolean\n---@return nil\nlocal function maybe(flag) if flag then return nil end end\n---@param value nil\nlocal function consume(value) end\n";
    accepted(&format!("{declarations}consume((maybe(false)))"));
    rejected(&format!("{declarations}consume(maybe(false))"), "TS2554");
    accepted(
        "---@return nil\nlocal function one() return nil end\n---@param value nil\nlocal function consume(value) end\nconsume(one())",
    );
}
#[test]
fn unreachable_explicit_nil_return_does_not_invent_a_result() {
    accepted(
        "---@return nil\nlocal function none() if false then return nil end end\nlocal function zero() end\nzero(none())",
    );
}
#[test]
fn annotated_recursive_condition_uses_scalar_contract_without_count_cycle() {
    // The recursive condition resolves without a result-count cycle. Its
    // `return nil` stays structurally reachable, so the call has 0-1 results.
    rejected(
        "---@return nil\nlocal function recursive() if recursive() then return nil end end\nlocal function zero() end\nzero(recursive())",
        "TS2554",
    );
}
#[test]
fn void_union_scalarizes_as_nil_but_expansion_retains_zero_or_one() {
    let declarations = "---@param flag boolean\n---@return number|void\nlocal function maybe(flag) if flag then return 1 end end\n";
    let report = accepted(&format!(
        "{declarations}---@type number|nil\nlocal value=(maybe(false))"
    ));
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "value")
            .unwrap()
            .ty,
        "number | nil"
    );
    rejected(
        &format!(
            "{declarations}---@param value number|nil\nlocal function consume(value) end\nconsume(maybe(false))"
        ),
        "TS2554",
    );
}
#[test]
fn nil_function_type_contract_does_not_guarantee_a_physical_slot() {
    let declarations = "---@type fun(): nil\nlocal callback=function() end\n---@param value nil\nlocal function consume(value) end\n";
    accepted(&format!("{declarations}consume((callback()))"));
    rejected(&format!("{declarations}consume(callback())"), "TS2554");
}
#[test]
fn scalar_method_calls_use_declared_type_without_recursive_pack_count() {
    accepted(
        "---@param self {}\n---@return nil\nlocal function initial(self) return nil end\n---@type {run: fun(self: {}): nil}\nlocal object={run=initial}\n---@param self {}\n---@return nil\nfunction object.run(self) if object:run() then return nil end end\n---@type nil\nlocal result=(object:run())",
    );
}
#[test]
fn source_generic_scalar_return_context_keeps_native_inference() {
    accepted(
        "---@generic T\n---@param input any\n---@return T,string\nlocal function make(input) return input,\"tag\" end\n---@type number\nlocal value=(make(1))",
    );
}

#[test]
fn nested_returns_and_forwarding_use_detached_native_function_graphs() {
    accepted(
        "---@return nil\nlocal function none() local function inner() return 1 end end\n---@return nil\nlocal function forward() return none() end\nlocal function zero() end\nzero(forward())",
    );
    rejected(
        "---@return nil\nlocal function one() return nil end\nlocal function zero() end\nzero(one())",
        "TS2554",
    );
}

#[test]
fn cli_cardinality_error_is_corrected_by_scalarizing_the_original_lua_argument() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis scalar cardinality {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("scalar-count.lua");
    let source = "---@param flag boolean\n---@return nil\nlocal function maybe(flag) if flag then return nil end end\n---@param value nil\nlocal function consume(value) end\nconsume(maybe(false))\n---@type nil\nlocal inspected=(maybe(false))";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2554");
    let span = report.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "consume(maybe(false))");
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
        errors.contains("TS2554") && errors.contains(":6:1"),
        "{errors}"
    );
    fs::write(
        &path,
        source.replace("consume(maybe(false))", "consume((maybe(false)))"),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    let types = String::from_utf8_lossy(&corrected.stdout);
    assert!(types.contains("inspected: nil"), "{types}");
    fs::remove_dir_all(directory).unwrap();
}
