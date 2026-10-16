//! V1 distinguishes inferred table/member literals from mutable scalar locals.
//! Source evidence: Declarations.kt field inference; string_literals.lua,
//! indexed_fields.lua, tables.lua and class.lua in IntelliJ-Luanalysis.
use luanalysis::{CheckReport, check};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn rejects(source: &str, statement: &str, expression: &str) {
    let start = source.find(statement).unwrap() + statement.rfind(expression).unwrap();
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2322"
            && error.span.start == start
            && error.span.end == start + expression.len()),
        "{source}\nExpected TS2322 at {start}..{}\n{:#?}",
        start + expression.len(),
        report.diagnostics
    );
}

#[test]
fn inferred_constructor_fields_keep_string_number_boolean_and_nested_literals() {
    let source = "local value={tag='a',[1]=1,flag=true,nested={tag='inner'}}\n---@type 'a'\nlocal tag=value.tag\n---@type 1\nlocal number=value[1]\n---@type true\nlocal flag=value.flag\n---@type 'inner'\nlocal nested=value.nested.tag\n";
    clean(source);
    let wrong = source.replace("---@type 'a'", "---@type 'b'");
    rejects(&wrong, "local tag=value.tag", "value.tag");
    clean(&wrong.replace("---@type 'b'", "---@type 'a'"));
    let wrong = source.replace("---@type 1", "---@type 2");
    rejects(&wrong, "local number=value[1]", "value[1]");
    let wrong = source.replace("---@type true", "---@type false");
    rejects(&wrong, "local flag=value.flag", "value.flag");
}

#[test]
fn captured_and_returned_allocations_keep_prospective_field_literals() {
    let source = "local value={tag='a',[1]=1}\nlocal alias=value\n---@return 'a'\nlocal function read() return alias.tag end\n---@return 1\nlocal function indexed() return value[1] end\nlocal function make() return {tag='a'} end\nlocal made=make()\n---@type 'a'\nlocal returned=made.tag\nlocal result=read()\n";
    clean(source);
    let wrong = source.replace("---@return 'a'", "---@return 'b'");
    rejects(
        &wrong,
        "local function read() return alias.tag end",
        "alias.tag",
    );
    clean(&wrong.replace("---@return 'b'", "---@return 'a'"));
}

#[test]
fn original_later_stores_keep_literals_in_deferred_bodies_and_at_reached_reads() {
    let source = "local value={}\nvalue.tag='a'\n---@type 'a'\nlocal reached=value.tag\n---@return 'a'\nlocal function deferred() return value.tag end\nlocal result=deferred()\n";
    clean(source);
    let wrong = source.replace("---@return 'a'", "---@return 'b'");
    rejects(
        &wrong,
        "local function deferred() return value.tag end",
        "value.tag",
    );
    clean(&wrong.replace("---@return 'b'", "---@return 'a'"));
}

#[test]
fn class_constants_keep_literal_contracts_in_deferred_class_methods() {
    let source = "---@class Constants\nlocal Constants={}\nConstants.ONE=1\nConstants.NAME='name'\n---@return 1\nfunction Constants.read() return Constants.ONE end\n---@return 'name'\nfunction Constants.name() return Constants.NAME end\n---@type 1\nlocal one=Constants.ONE\nlocal result=Constants.read()\n";
    clean(source);
    let wrong = source.replace("---@return 1", "---@return 2");
    rejects(
        &wrong,
        "function Constants.read() return Constants.ONE end",
        "Constants.ONE",
    );
    clean(&wrong.replace("---@return 2", "---@return 1"));
}

#[test]
fn mutable_scalar_locals_and_nonliteral_field_sources_still_widen() {
    let source = "local text='a'\ntext='b'\nlocal value={text=text}\n---@type string\nlocal broad=value.text\n";
    clean(source);
    let wrong = source.replace("---@type string", "---@type 'b'");
    rejects(&wrong, "local broad=value.text", "value.text");
    let scalar = "local text='a'\n---@type 'a'\nlocal invalid=text\n";
    rejects(scalar, "local invalid=text", "text");
    // V1 Declarations.kt broadens an inferred scalar local even when its
    // initializer reads a field whose own contract is a singleton literal.
    let copy = "local value={text='a'}\nlocal copy=value.text\n---@type 'a'\nlocal invalid=copy\n";
    rejects(copy, "local invalid=copy", "copy");
}

#[test]
fn written_broad_fields_remain_mutable_and_reject_wrong_primitive_values() {
    let source = "local value={\n---@type string\ntext='a',\n---@type number|string|nil\ncurrent=1,\n}\nvalue.text='b'\nvalue.text='c'\nvalue.current='word'\n---@type string\nlocal text=value.current\nvalue.current=nil\n---@type nil\nlocal gone=value.current\nvalue.current=2\n---@type number\nlocal restored=value.current\n";
    clean(source);
    let wrong = format!("{source}value.text=2\n");
    rejects(&wrong, "value.text=2", "2");
    clean(&wrong.replace("value.text=2", "value.text='d'"));
}

#[test]
fn same_literal_stores_are_allowed_and_written_singletons_reject_other_values() {
    let source = "local value={tag='a',[1]=1}\nvalue.tag='a'\nvalue[1]=1\n";
    clean(source);
    let wrong = format!("{source}value.tag='b'\n");
    rejects(&wrong, "value.tag='b'", "'b'");
    clean(&wrong.replace("value.tag='b'", "value.tag='a'"));
    let wrong = format!("{source}value[1]=2\n");
    rejects(&wrong, "value[1]=2", "2");
    let written = "local value={\n---@type 'a'\ntag='a',\n}\nvalue.tag='b'\n";
    rejects(written, "value.tag='b'", "'b'");
    clean(&written.replace("value.tag='b'", "value.tag='a'"));
}

#[test]
fn inferred_class_literal_write_bounds_match_the_v1_override_mutation_fixture() {
    // overrides.lua:346–351 and :413 preserve the unannotated value=1 field,
    // then explicitly reject assigning a broad number to that class member.
    let wrong = "---@class InferredLiteral\nlocal InferredLiteral={value=1}\n---@param object InferredLiteral\n---@param number number\nlocal function mutate(object,number) object.value=number end\n";
    rejects(wrong, "object.value=number", "number");
    clean(&wrong.replace("object.value=number", "object.value=1"));
}

#[test]
fn class_and_alias_function_fields_keep_callability_and_compatible_replacement() {
    let source = "---@alias Callback fun(value:number):number\n---@class Api\nlocal Api={\n---@type Callback\ncallback=function(value) return value end,\n}\nlocal alias=Api\n---@param value number\n---@return number\nlocal function twice(value) return value*2 end\nalias.callback=twice\n---@type number\nlocal result=Api.callback(2)\nlocal ordinary={callback=twice}\nlocal other=ordinary\nother.callback=twice\n---@type number\nlocal compatible=ordinary.callback(3)\n";
    clean(source);
    let wrong = source.replace("alias.callback=twice", "alias.callback='wrong'");
    rejects(&wrong, "alias.callback='wrong'", "'wrong'");
    clean(&wrong.replace("alias.callback='wrong'", "alias.callback=twice"));
    let wrong = source.replace("other.callback=twice", "other.callback='wrong'");
    rejects(&wrong, "other.callback='wrong'", "'wrong'");
}
