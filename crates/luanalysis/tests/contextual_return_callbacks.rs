use luanalysis::{CheckReport, check};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

const FACTORY: &str = "---@alias Factory fun(value:number):fun(text:string):number\n---@param captured number\nlocal function outer(captured)\n---@type Factory\nlocal factory=function(value)\nreturn function(text) return value+captured end\nend\nlocal inner=factory(42)\n---@type number\nlocal accepted=inner('text')\nend\n";

#[test]
fn captured_returned_callbacks_use_their_contextual_signature_during_body_validation() {
    let report = clean(FACTORY);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "text")
            .unwrap()
            .ty,
        "string"
    );
    let invalid = FACTORY.replace(
        "return function(text) return value+captured end",
        "return function(text)\n---@type number\nlocal invalid=text\nreturn value+captured end",
    );
    let report = check(&invalid);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
    clean(FACTORY);
}

#[test]
fn contextual_generic_returned_callbacks_keep_each_original_capture_mapper() {
    let source = "---@generic T\n---@param captured T\nlocal function outer(captured)\n---@type fun():fun(value:T):T\nlocal factory=function() return function(value) return value end end\nreturn factory()\nend\nlocal numeric=outer(42)\nlocal textual=outer('text')\n---@type number\nlocal one=numeric(7)\n---@type string\nlocal two=textual('value')\n";
    clean(source);
    let invalid = source.replace("numeric(7)", "numeric('wrong')");
    let report = check(&invalid);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        report.diagnostics
    );
}
