use luanalysis::{CheckReport, check};

const SOURCE: &str = r#"---@shape PublicDispatcher
---@field boolean fun(value:boolean):void
---@field number fun(value:number):void
---@field string fun(value:string):void
---@field table fun(value:table):void

---@shape ImplementationDispatcher : PublicDispatcher
---@field __index fun(owner:table,key:string):never

---@param value table
local function expectTable(value) end
---@param value string
local function expectString(value) end
---@param value number
local function expectNumber(value) end

---@param value unknown
local function booleanHandler(value) end

---@param value number
local function numberHandler(value) end

---@param value string
local function stringHandler(value) end

---@param value table
local function tableHandler(value) end

---@type ImplementationDispatcher
local dispatcher={
    boolean=booleanHandler,
    number=numberHandler,
    string=stringHandler,
    table=tableHandler,
    __index=function(owner,key)
        expectTable(owner)
        expectString(key)
        while true do end
    end,
}

---@type PublicDispatcher
local published=dispatcher
"#;

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

#[test]
fn private_metatable_members_preserve_the_public_handler_contract() {
    clean(SOURCE);
    let wrong = SOURCE.replace("number=numberHandler", "number=stringHandler");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    // At the handler's field, against the public handler it implements, as TypeScript reports
    // an object literal's property.
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        report.diagnostics[0].message,
        "Type 'fun(value: string): void' is not assignable to type 'fun(value: number): void'."
    );
    assert_eq!(&wrong[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "number");
    clean(SOURCE);
}

#[test]
fn implementation_only_callback_fields_supply_their_original_parameter_context() {
    for (original, replacement, operand) in [
        ("expectString(key)", "expectNumber(key)", "key"),
        ("expectTable(owner)", "expectString(owner)", "owner"),
    ] {
        let wrong = SOURCE.replace(original, replacement);
        let report = check(&wrong);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{wrong}\n{:#?}",
            report.diagnostics
        );
        let error = &report.diagnostics[0];
        assert_eq!(error.code, "TS2345");
        let start = wrong.find(replacement).unwrap() + replacement.find(operand).unwrap();
        assert_eq!(error.span.start, start);
        assert_eq!(error.span.end, start + operand.len());
        clean(SOURCE);
    }
}
