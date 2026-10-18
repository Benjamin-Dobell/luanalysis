//! Inspecting predicate metadata must preserve ordinary short-circuit demands.
use luanalysis::check;

const SOURCE: &str = r#"---@param value table
---@return false
local function ordinary(value) return false end

---@param value number
---@return number
local function consume(value) return 0 end

---@param value any
---@return number
local function fallback(value) return 0 end

---@param value table
---@return number
local function choose(value)
    return ordinary(value) and consume(value) or fallback(value)
end
"#;

#[test]
fn ordinary_guard_queries_do_not_replay_an_unreached_sibling_call() {
    // Reading `value` for fallback walks both short-circuit guards. The false
    // result keeps consume(value) outside the original expression's demands.
    let report = check(SOURCE);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);

    let reached = SOURCE.replace("false", "true");
    let report = check(&reached);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
}

#[test]
fn ordinary_overload_guards_keep_their_selected_false_return() {
    let source = SOURCE.replace(
        "---@param value table\n---@return false\nlocal function ordinary",
        "---@overload fun(value: table<string, any>): false\n---@overload fun(value: table): boolean\n---@param value table\n---@return boolean\nlocal function ordinary",
    );
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn an_unreached_predicate_sibling_does_not_publish_call_errors() {
    let source = SOURCE.replace(
        "---@param value number\n---@return number\nlocal function consume(value) return 0 end",
        "---@param value number\n---@return value is number\nlocal function consume(value) return true end",
    );
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn predicate_metadata_does_not_publish_unreached_nested_argument_errors() {
    let source = SOURCE
        .replace("---@param value number\n---@return number\nlocal function consume(value) return 0 end", "---@param ignored number\n---@param value table\n---@return value is table\nlocal function consume(ignored, value) return true end\n---@param input number\n---@return number\nlocal function nested(input) return input end")
        .replace("consume(value) or", "consume(nested('wrong'), value) or");
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let reached = source
        .replace(
            "---@return false\nlocal function ordinary(value) return false end",
            "---@return true\nlocal function ordinary(value) return true end",
        )
        .replace(
            "---@param value table\n---@return number\nlocal function choose",
            "---@param value table\n---@return number | boolean\nlocal function choose",
        );
    let report = check(&reached);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
}

#[test]
fn an_inline_predicate_view_bypasses_immutable_boolean_absence() {
    let source = r#"---@param value unknown
---@return boolean
local function ordinary(value) return type(value) == 'string' end

---@param value string | number
local function inspect(value)
    if ordinary(value) then
        ---@type string
        local checked = value
    end
end
"#;
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let asserted = source.replace(
        "if ordinary(value)",
        "if (ordinary --[[@as fun(value: unknown): value is string]])(value)",
    );
    let report = check(&asserted);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}
