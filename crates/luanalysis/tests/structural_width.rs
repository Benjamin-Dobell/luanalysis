use luanalysis::{CheckReport, check};

const RESPONSE: &str = "---@shape Response\n---@field statusCode number\n---@field headers table<string,string>\n---@field body string\n\n---@param response Response\nlocal function handler(response) end\n";
const RESPONSE_VALUE: &str = "{statusCode=200,headers={},body='response',outbound='request'}";

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn one_error_at(source: &str, code: &str, expression: &str) {
    let report = check(source);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, code);
    assert_eq!(
        &source[diagnostic.span.start..diagnostic.span.end],
        expression
    );
}

#[test]
fn v1_shape_fixture_accepts_extra_constructor_members() {
    // IntelliJ-Luanalysis inspections/shape.lua:21 accepts this exact extra
    // member. Its adjacent controls reject a wrong a type and a missing b.
    clean(
        "---@shape Shape1\n---@field a number\n\n---@type Shape1\nlocal shape1\nshape1 = {a = 1, extraMember = 'whatever'}\n",
    );
    one_error_at(
        "---@shape Shape1\n---@field a number\n\n---@type Shape1\nlocal shape1={a='wrong',extraMember='whatever'}\n",
        "TS2322",
        "{a='wrong',extraMember='whatever'}",
    );
    one_error_at(
        "---@shape Shape1\n---@field a number\n\n---@shape Shape2:Shape1\n---@field b string\n\n---@type Shape2\nlocal shape2={a=1,extraMember='whatever'}\n",
        "TS2322",
        "{a=1,extraMember='whatever'}",
    );
}

#[test]
fn callback_argument_extras_do_not_replace_required_field_checks() {
    let source = format!("{RESPONSE}handler({RESPONSE_VALUE})\n");
    clean(&source);
    let wrong = source.replace("statusCode=200", "statusCode='wrong'");
    one_error_at(
        &wrong,
        "TS2345",
        "{statusCode='wrong',headers={},body='response',outbound='request'}",
    );
    clean(&wrong.replace("statusCode='wrong'", "statusCode=200"));
    let missing = source.replace("statusCode=200,", "");
    one_error_at(
        &missing,
        "TS2345",
        "{headers={},body='response',outbound='request'}",
    );
    clean(&missing.replace("{headers={}", "{statusCode=200,headers={}"));
}

#[test]
fn fresh_assignment_and_return_follow_the_same_structural_rule() {
    for statement in [
        format!("---@type Response\nlocal response={RESPONSE_VALUE}\n"),
        format!("---@return Response\nlocal function build() return {RESPONSE_VALUE} end\n"),
    ] {
        let source = format!("{RESPONSE}{statement}");
        clean(&source);
        let wrong = source.replace("statusCode=200", "statusCode='wrong'");
        one_error_at(
            &wrong,
            "TS2322",
            "{statusCode='wrong',headers={},body='response',outbound='request'}",
        );
        clean(&wrong.replace("statusCode='wrong'", "statusCode=200"));
    }
}

#[test]
fn nested_contextual_constructors_allow_extras_and_check_declared_values() {
    let source = format!(
        "{RESPONSE}\n---@shape Envelope\n---@field response Response\n\n---@param envelope Envelope\nlocal function accept(envelope) end\naccept({{response={RESPONSE_VALUE},private=true}})\n"
    );
    clean(&source);
    let wrong = source.replace("statusCode=200", "statusCode='wrong'");
    one_error_at(
        &wrong,
        "TS2345",
        "{response={statusCode='wrong',headers={},body='response',outbound='request'},private=true}",
    );
    clean(&wrong.replace("statusCode='wrong'", "statusCode=200"));
}

#[test]
fn acceptance_preserves_inferred_extras_without_extending_the_declared_shape() {
    let source = format!(
        "{RESPONSE}local response={RESPONSE_VALUE}\nhandler(response)\nlocal preserved=response.outbound\n---@type string\nlocal accepted=preserved\n"
    );
    let report = clean(&source);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "preserved")
            .unwrap()
            .ty,
        "string"
    );
    let hidden = format!(
        "{RESPONSE}\n---@param response Response\nlocal function read(response) return response.outbound end\n"
    );
    one_error_at(&hidden, "TS2339", "outbound");
    clean(&hidden.replace("response.outbound", "response.body"));
}

#[test]
fn indexer_values_nominal_origins_and_array_form_remain_checked() {
    one_error_at(
        "---@type table<string,number>\nlocal values={known=1,outbound='wrong'}\n",
        "TS2322",
        "{known=1,outbound='wrong'}",
    );
    clean("---@type table<string,number>\nlocal values={known=1,outbound=2}\n");
    one_error_at(
        "---@class Nominal\n---@field value number\n\n---@type Nominal\nlocal value={value=1,extra=true}\n",
        "TS2322",
        "{value=1,extra=true}",
    );
    one_error_at(
        "---@type number[]\nlocal values={1,extra=true}\n",
        "TS2322",
        "{1,extra=true}",
    );
    clean("---@type number[]\nlocal values={1,2}\n");
}
