use luanalysis::check;

fn clean(source: &str) {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
}
fn rejected_at(source: &str, expression: &str) {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{source}\n{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322", "{source}\n{:#?}", report.diagnostics);
    assert_eq!(&source[error.span.start..error.span.end], expression);
}

#[test]
fn widening_a_mutable_predicate_slot_cannot_change_a_literal_refinement() {
    let source = r#"---@class Guards<T>
---@field test fun(value:unknown):value is T

---@param value unknown
---@return value is number
local function isTwo(value) return value==2 end

---@param narrow Guards<1>
---@param candidate unknown
local function mutate(narrow,candidate)
 ---@type Guards<number>
 local wide=narrow
 wide.test=isTwo
 if narrow.test(candidate) then
  ---@type 1
  local assumed=candidate
  return assumed
 end
end
"#;
    rejected_at(source, "narrow");
    let corrected = source.replace("Guards<number>", "Guards<1>")
        .replace("value is number", "value is 1")
        .replace("value==2", "value==1")
        .replace("isTwo", "isOne");
    clean(&corrected);
}

fn callable_relation(target: &str, source_argument: &str, target_argument: &str, accepted: bool) {
    let source = format!("---@class Guard<T>\n---@overload fun(value:unknown):value is {target}\n\n---@param value Guard<{source_argument}>\n---@return Guard<{target_argument}>\nlocal function convert(value) return value end\n");
    if accepted { clean(&source); } else { rejected_at(&source, "value"); }
}

#[test]
fn scalar_predicate_callable_contracts_keep_natural_covariance() {
    callable_relation("T", "1", "number", true);
    callable_relation("T", "number", "1", false);
}

#[test]
fn predicate_targets_exposing_mutable_records_or_arrays_are_invariant() {
    for target in ["{value:T}", "T[]"] {
        callable_relation(target, "1", "number", false);
        callable_relation(target, "number", "1", false);
    }
}

#[test]
fn erased_predicate_alias_arguments_remain_phantom() {
    let source = "---@alias Ignore<T> number\n---@class Guard<T>\n---@field test fun(value:unknown):value is Ignore<T>\n\n---@param value Guard<string>\n---@return Guard<number>\nlocal function convert(value) return value end\n";
    clean(source);
    clean(&source.replace("Guard<string>", "Guard<boolean>"));
}
