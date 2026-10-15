use luanalysis::{CheckReport, check};

// A written singleton contract supplies the enum-like meaning; ordinary
// mutable object members retain their usual widening behavior without it.
const SOURCE: &str = r#"---@alias Result 0 | 1 | 2

---@class ResultBase
local Base = {}
---@type { IGNORE: 0, ACCEPT: 1, REJECT: 2 }
Base.FilterResult = { IGNORE = 0, ACCEPT = 1, REJECT = 2 }
Base.FilterResult.ACCEPT = 1

---@class ResultChild : ResultBase

---@return Result
local function direct()
    return Base.FilterResult.ACCEPT
end

---@param child ResultChild
---@param enabled boolean
---@return Result
local function inherited(child, enabled)
    return enabled and child.FilterResult.ACCEPT or child.FilterResult.IGNORE
end
"#;

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

#[test]
fn written_mutable_constant_tables_preserve_direct_and_inherited_union_returns() {
    clean(SOURCE);
}

#[test]
fn written_mutable_constant_table_initializers_reject_the_wrong_singleton() {
    let bad = SOURCE.replacen("ACCEPT = 1,", "ACCEPT = 2,", 1);
    let position = bad.find("ACCEPT = 2,").unwrap() + "ACCEPT = ".len();
    let report = check(&bad);
    assert!(
        report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "TS2322"
                && diagnostic.span.start <= position
                && diagnostic.span.end > position
        }),
        "{:#?}", report.diagnostics
    );
    clean(&bad.replacen("ACCEPT = 2,", "ACCEPT = 1,", 1));
}

#[test]
fn written_mutable_constant_tables_check_later_writes_and_actual_return_values() {
    for (original, wrong, rejected) in [
        ("Base.FilterResult.ACCEPT = 1", "Base.FilterResult.ACCEPT = 2", "2"),
        (
            "return enabled and child.FilterResult.ACCEPT or child.FilterResult.IGNORE",
            "return 6",
            "6",
        ),
    ] {
        let bad = SOURCE.replace(original, wrong);
        let report = check(&bad);
        let position = bad.find(wrong).unwrap() + wrong.rfind(rejected).unwrap();
        assert!(
            report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "TS2322"
                    && diagnostic.span.start == position
                    && diagnostic.span.end == position + rejected.len()
            }),
            "{wrong}: {:#?}", report.diagnostics
        );
        clean(&bad.replace(wrong, original));
    }
}
