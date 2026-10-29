use luanalysis::{CheckReport, check};

const HANDLERS: &str = r#"local handlers = {
    ---@type number
    id = 1,
    ---@param receiver self
    ---@return number
    read = function(receiver)
        return receiver.id
    end,
}
---@type number
local result = handlers.read(handlers)
"#;

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}

#[test]
fn ordinary_literal_field_self_reads_its_original_structural_contract() {
    // v1 LuaPsiImplUtil.guessParentType(LuaTableField) falls back to its
    // containing literal, and LuaDocPsiImplUtil.resolveType reserves self.
    let report = clean(HANDLERS);
    assert_eq!(
        report.bindings.iter().find(|binding| binding.name == "result").unwrap().ty,
        "number"
    );
    let wrong = HANDLERS.replace("return receiver.id", "return 'wrong'");
    let report = check(&wrong);
    let error = report.diagnostics.iter().find(|error| error.code == "TS2322").unwrap();
    assert_eq!(&wrong[error.span.start..error.span.end], "'wrong'");
    let missing = HANDLERS.replace("receiver.id", "receiver.absent");
    assert!(check(&missing).diagnostics.iter().any(|error| error.code == "TS2339"));
    clean(HANDLERS);
}

#[test]
fn literal_field_self_rejects_an_argument_with_a_wrong_declared_field() {
    let wrong = HANDLERS.replace(
        "handlers.read(handlers)",
        "handlers.read({id = 'wrong', read = handlers.read})",
    );
    let report = check(&wrong);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2345"), "{:#?}", report.diagnostics);
    clean(HANDLERS);
}

#[test]
fn later_bad_writes_do_not_change_a_literal_self_field_contract() {
    let wrong = format!("{HANDLERS}handlers.id = 'wrong'\n---@type number\nlocal later = handlers.read(handlers)\n");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(&wrong[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "'wrong'");
    clean(&wrong.replace("'wrong'", "2"));
}

#[test]
fn a_recursive_literal_self_result_keeps_the_same_original_owner() {
    let source = r#"local node = {
    ---@type number
    id = 1,
    ---@param receiver self
    ---@return self
    identity = function(receiver) return receiver end,
}
local copy = node.identity(node)
---@type number
local id = copy.identity(copy).id
"#;
    clean(source);
    let wrong = source.replace("---@type number\nlocal id", "---@type string\nlocal id");
    assert!(check(&wrong).diagnostics.iter().any(|error| error.code == "TS2322"));
    clean(source);
}

#[test]
fn lexical_generic_named_self_precedes_the_literal_owner() {
    let source = r#"local functions = {
    ---@generic self
    ---@param value self
    ---@return self
    identity = function(value) return value end,
}
---@type number
local numeric = functions.identity(42)
---@type string
local textual = functions.identity('word')
"#;
    clean(source);
    let wrong = source.replace("local textual = functions.identity('word')", "local textual = functions.identity(42)");
    assert!(check(&wrong).diagnostics.iter().any(|error| error.code == "TS2322"));
    clean(source);
}

#[test]
fn nested_literals_keep_distinct_self_fields() {
    let source = r#"local outer = {
    ---@type number
    value = 1,
    inner = {
        ---@type string
        value = 'word',
        ---@param receiver self
        ---@return string
        read = function(receiver) return receiver.value end,
    },
    ---@param receiver self
    ---@return number
    read = function(receiver) return receiver.value end,
}
---@type number
local number_value = outer.read(outer)
---@type string
local string_value = outer.inner.read(outer.inner)
"#;
    clean(source);
    let wrong = source.replace("outer.inner.read(outer.inner)", "outer.inner.read(outer)");
    assert!(check(&wrong).diagnostics.iter().any(|error| error.code == "TS2345"));
    clean(source);
}

#[test]
fn a_global_class_named_self_is_not_a_contextual_owner() {
    // Outside any member, `self` names nothing, as TypeScript's `this` type (TS2526).
    let source = "---@class self\n---@field sentinel number\n\n---@type self\nlocal value\nlocal invalid = value.sentinel\n";
    let report = check(source);
    let codes: Vec<_> = report.diagnostics.iter().map(|error| error.code).collect();
    assert_eq!(codes, ["TS2526"], "{:#?}", report.diagnostics);
    assert_eq!(&source[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "self");
}

#[test]
fn factory_literal_self_keeps_each_original_generic_application() {
    let source = r#"---@generic T
---@param value T
local function make(value)
    local object = {
        ---@type T
        value = value,
        ---@param receiver self
        ---@return T
        read = function(receiver) return receiver.value end,
    }
    return object
end
local numeric = make(1)
local textual = make('word')
---@type number
local first = numeric.read(numeric)
---@type string
local second = textual.read(textual)
"#;
    clean(source);
    let wrong = source.replace("local second = textual.read(textual)", "local second = numeric.read(numeric)");
    assert!(check(&wrong).diagnostics.iter().any(|error| error.code == "TS2322"));
    clean(source);
}
