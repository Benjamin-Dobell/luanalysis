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

fn rejects(source: &str, assignment: &str, value: &str) {
    let start = source.find(assignment).unwrap() + assignment.rfind(value).unwrap();
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2322"
            && error.span.start == start
            && error.span.end == start + value.len()),
        "{source}\nExpected TS2322 at {start}..{}\n{:#?}",
        start + value.len(),
        report.diagnostics
    );
}

#[test]
fn inferred_field_copies_have_fixed_broad_scalar_local_contracts() {
    let source = "local value={text='a',number=1,flag=true}\nlocal text=value.text\nlocal number=value.number\nlocal flag=value.flag\ntext='b'\nnumber=2\nflag=false\n---@type 'a'\nlocal originalText=value.text\n---@type 1\nlocal originalNumber=value.number\n---@type true\nlocal originalFlag=value.flag\n";
    let report = clean(source);
    for (binding, expected) in [
        ("text=value.text", "string"),
        ("number=value.number", "number"),
        ("flag=value.flag", "boolean"),
    ] {
        let ty = report.type_at(source.find(binding).unwrap()).unwrap();
        assert!(
            ty == expected || expected == "boolean" && ty.contains("false") && ty.contains("true"),
            "{binding}: {ty}"
        );
    }
    for (assignment, rhs) in [
        ("text=3", "3"),
        ("number='wrong'", "'wrong'"),
        ("flag=3", "3"),
    ] {
        let wrong = format!("{source}{assignment}\n");
        rejects(&wrong, assignment, rhs);
        clean(source);
    }
}

#[test]
fn a_local_copy_does_not_retain_the_fields_singleton_read_type() {
    for (field, literal, annotation) in [("text", "'a'", "'a'"), ("number", "1", "1")] {
        let source = format!(
            "local value={{{field}={literal}}}\nlocal copy=value.{field}\n---@type {annotation}\nlocal invalid=copy\n"
        );
        rejects(&source, "local invalid=copy", "copy");
    }
}

#[test]
fn lazy_captured_locals_use_the_same_initial_scalar_contract() {
    let source = "local value={tag='a',number=1}\nlocal copy=value.tag\nlocal numeric=value.number\n---@return string\nlocal function read() return copy end\n---@return number\nlocal function number() return numeric end\nlocal text=read()\nlocal result=number()\n";
    clean(source);
    let wrong = source.replace("---@return string", "---@return 'a'");
    rejects(&wrong, "local function read() return copy end", "copy");
    let wrong = source.replace("---@return number", "---@return 1");
    rejects(
        &wrong,
        "local function number() return numeric end",
        "numeric",
    );
}

#[test]
fn written_local_singletons_and_literal_unions_keep_their_contracts() {
    let source = "local value={tag='a',\n---@type 1|2\nchoice=1,\n}\n---@type 'a'\nlocal written=value.tag\nlocal choices=value.choice\nchoices=1\nchoices=2\n---@type 1|2\nlocal accepted=choices\n---@class Constants\nlocal Constants={}\nConstants.ONE=1\n---@return 1\nfunction Constants.read() return Constants.ONE end\n";
    let report = clean(source);
    let ty = report
        .type_at(source.find("choices=value.choice").unwrap())
        .unwrap();
    assert!(
        ty.contains('1') && ty.contains('2') && !ty.contains("number"),
        "{ty}"
    );
    let wrong = format!("{source}written='b'\n");
    rejects(&wrong, "written='b'", "'b'");
    let wrong = format!("{source}choices=3\n");
    rejects(&wrong, "choices=3", "3");
}
