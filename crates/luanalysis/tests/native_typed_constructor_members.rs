use luanalysis::{CheckReport, check};

const CONSTRUCTOR: &str = r#"---@class Instance

---@return Instance
local function newInstance()
    local instance = --[[---@type Instance]] {}
    ---@param amount number
    ---@return number
    function instance.value(amount)
        return amount + 1
    end
    return instance
end

---@param instance Instance
---@return number
local function use(instance)
    return instance.value(1)
end
"#;

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

#[test]
fn original_typed_local_constructor_functions_publish_the_named_instance_contract() {
    clean(CONSTRUCTOR);
    let bad = CONSTRUCTOR.replace("instance.value(1)", "instance.value('wrong')");
    let report = check(&bad);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| error.code != "TS2339" && error.code != "TS2349"),
        "{:#?}",
        report.diagnostics
    );
    clean(&bad.replace("'wrong'", "1"));
}

#[test]
fn typed_constructor_function_bodies_keep_their_written_return_checks() {
    let bad = CONSTRUCTOR.replace("return amount + 1", "return 'wrong'");
    let report = check(&bad);
    let error = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2322")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&bad[error.span.start..error.span.end], "'wrong'");
    clean(CONSTRUCTOR);
}

#[test]
fn annotation_owner_does_not_spread_to_a_shadow_or_unannotated_alias() {
    let source = format!(
        "{CONSTRUCTOR}\nlocal alias = {{accidental = function() end}}\n---@param instance Instance\nlocal function bad(instance) instance.accidental() end\n"
    );
    let report = check(&source);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2339"
            && source[error.span.start..error.span.end] == *"accidental"),
        "{:#?}",
        report.diagnostics
    );
    clean(&source.replace("instance.accidental()", "instance.value(1)"));
}

#[test]
fn original_local_annotation_and_colon_definition_use_the_same_class_self() {
    clean(
        r#"---@class Instance

---@return Instance
local function newInstance()
    ---@type Instance
    local instance = --[[---@type Instance]] {}
    ---@return self
    function instance:identity()
        return self
    end
    return instance
end

---@param instance Instance
---@return Instance
local function use(instance)
    return instance:identity():identity()
end
"#,
    );
}

#[test]
fn a_generic_annotation_shadow_does_not_augment_the_same_spelled_class() {
    let source = r#"---@class Instance

---@generic Instance
---@param input Instance
local function adapt(input)
    local instance = --[[---@type Instance]] input
    function instance.accidental() end
    return instance
end

---@param actual Instance
local function bad(actual)
    actual.accidental()
end
"#;
    let report = check(source);
    let call = source.rfind("accidental").unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2339" && error.span.start == call),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_typed_constructor_override_uses_the_original_named_definition_target() {
    let source = CONSTRUCTOR.replace(
        "---@class Instance",
        "---@class Base\n---@field value fun(amount: number): number\n\n---@class Instance : Base",
    );
    clean(&source);
    let bad = source
        .replace("---@param amount number", "---@param amount string")
        .replace("return amount + 1", "return 1");
    let report = check(&bad);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2416" || error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
    clean(&source);
}

#[test]
fn recursive_class_schema_reads_the_written_initializer_contract_without_reexecuting_the_cast() {
    let source = r#"---@shape Contract
---@field log fun(message: string): void
---@field check fun(value: boolean, message: string): void

---@class Logger : Contract

---@return Logger
local function make()
    local self = --[[---@type Logger]] {}
    ---@param message string
    function self.log(message) end
    ---@param value boolean
    ---@param message string
    function self.check(value, message)
        if not value then self.log(message) end
    end
    return self
end
"#;
    clean(source);
    let bad = source.replace("then self.log(message)", "then self.log(1)");
    let report = check(&bad);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| error.code != "TS2589" && error.code != "TS7023"),
        "{:#?}",
        report.diagnostics
    );
    clean(&bad.replace("self.log(1)", "self.log(message)"));
}

#[test]
fn publishing_a_written_instance_owner_keeps_authoritative_casts_and_member_contracts() {
    let converted =
        CONSTRUCTOR.replace("---@type Instance]] {}", "---@type Instance]] 'unrelated'");
    clean(&converted);
    let bad = converted.replace("instance.value(1)", "instance.value('wrong')");
    let report = check(&bad);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"
                && bad[error.span.start..error.span.end] == *"'wrong'"),
        "{:#?}",
        report.diagnostics
    );
    clean(&bad.replace("instance.value('wrong')", "instance.value(1)"));
}

#[test]
fn captured_initializer_assertions_preserve_result_slots_and_literal_contracts() {
    // V1 scalar-local widening applies at both the captured initializer and
    // the returned scalar's receiving declaration; singleton storage is written.
    let source = r#"---@return number, string
local function pair() return 1, 'tail' end
local function make()
    local first, second = --[[---@type number, string]] pair()
    ---@type 1
    local literal = --[[---@type 1]] 1
    local function read() return second, literal end
    return read
end
---@type string, 1
local text, one = make()()
---@type string
local good_text = text
---@type 1
local good_one = one
"#;
    clean(source);
    let bad = source.replace("local good_text = text", "local good_text = one");
    assert!(
        check(&bad)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322")
    );
    clean(source);
}

#[test]
fn fresh_constructor_receiver_writes_do_not_reexecute_a_class_cast_to_classify_global_effects() {
    let source = r#"---@shape Contract
---@field log fun(message: string, level?: number): void
---@field check fun(value: boolean, message: string): void

---@class Logger : Contract

---@class StaticLogger
---@overload fun(): Logger
local Static = {}
local defaultLevel = 1
setmetatable(Static, {
    ---@return Logger
    __call = function()
        local self = --[[---@type Logger]] {}
        local filterLevel = 1
        ---@param message string
        ---@param level? number
        function self.log(message, level)
            level = level or defaultLevel
            if level <= filterLevel then print(message) end
        end
        ---@param value boolean
        ---@param message string
        function self.check(value, message)
            if not value then
                self.log(message, 1)
                error(message, 2)
            end
        end
        return self
    end,
})
"#;
    // The API deliberately has no builtin print/error; declaring their actual
    // contracts keeps this check independent of standard-library selection.
    let source = format!(
        "---@type fun(message: string): void\nlocal print\n---@type fun(message: string, level: number): never\nlocal error\n---@type fun<T>(value: T, meta: table): T\nlocal setmetatable\n{source}"
    );
    clean(&source);
    let bad = source.replace("self.log(message, 1)", "self.log(1, 1)");
    let report = check(&bad);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| error.code != "TS2589" && error.code != "TS7023"),
        "{:#?}",
        report.diagnostics
    );
    clean(&source);
}
