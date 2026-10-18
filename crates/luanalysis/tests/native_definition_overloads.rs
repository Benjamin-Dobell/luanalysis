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
fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .expect(name)
        .ty
}

#[test]
fn an_empty_definition_uses_only_its_explicit_overload_heads() {
    let source = "---@class Factory\n---@overload fun(value:number):number\n---@overload fun(value:string):string\nlocal differentName={}\nlocal number=differentName(42)\nlocal string=differentName(\"text\")";
    let report = clean(source);
    assert_eq!(ty(&report, "number"), "number");
    assert_eq!(ty(&report, "string"), "string");
    assert_ne!(ty(&report, "differentName"), "Factory");
    let wrong = format!("{source}\nlocal invalid=differentName(true)");
    let rejected = check(&wrong);
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769")
                && &wrong[error.span.start..error.span.end] == "true"),
        "{:#?}",
        rejected.diagnostics
    );
}

#[test]
fn signature_local_generics_infer_independently_on_a_definition() {
    let source = "---@class Factory\n---@overload fun<T:number>(value:T):T\n---@overload fun<T:string>(value:T):T\nlocal factory={}\nlocal number=factory(42)\nlocal string=factory(\"text\")\nlocal repeated=factory(84)\n---@type 42\nlocal exact_number=factory(42)\n---@type \"text\"\nlocal exact_string=factory(\"text\")\n---@type 84\nlocal exact_repeated=factory(84)";
    let report = clean(source);
    assert_eq!(ty(&report, "number"), "number");
    assert_eq!(ty(&report, "string"), "string");
    assert_eq!(ty(&report, "repeated"), "number");
    assert_eq!(ty(&report, "exact_number"), "42");
    assert_eq!(ty(&report, "exact_string"), "\"text\"");
    assert_eq!(ty(&report, "exact_repeated"), "84");
    let wrong_argument = format!("{source}\nfactory(true)");
    assert!(check(&wrong_argument).diagnostics.iter().any(|error| {
        matches!(error.code, "TS2345" | "TS2769")
            && &wrong_argument[error.span.start..error.span.end] == "true"
    }));
    let wrong_result = source.replace("exact_number=factory(42)", "exact_number=factory(84)");
    assert!(check(&wrong_result).diagnostics.iter().any(|error| {
        error.code == "TS2322" && &wrong_result[error.span.start..error.span.end] == "factory(84)"
    }));
}

#[test]
fn returned_definition_heads_retain_the_outer_mapper_without_owning_a_fake_function() {
    let report = clean(
        "---@generic T\n---@param value T\nlocal function make(value)\n---@class Factory\n---@overload fun():T\nlocal factory={}\nreturn factory\nend\nlocal number=make(42)\nlocal string=make(\"text\")\nlocal a=number()\nlocal b=string()\nlocal c=make(84)()",
    );
    assert_eq!(ty(&report, "a"), "number");
    assert_eq!(ty(&report, "b"), "string");
    assert_eq!(ty(&report, "c"), "number");
}

#[test]
fn an_annotation_only_factory_capture_collects_free_parameters_from_original_heads() {
    let report = clean(
        "---@generic T\n---@param value T\nlocal function make(value)\n---@class Factory\n---@overload fun():T\nlocal factory={}\n---@return Factory\nlocal function get() return factory end\nreturn get\nend\nlocal number=make(42)\nlocal string=make(\"text\")\nlocal a=number()()\nlocal b=string()()",
    );
    assert_eq!(ty(&report, "a"), "number");
    assert_eq!(ty(&report, "b"), "string");
}

#[test]
fn a_signature_local_parameter_shadows_an_outer_parameter_in_named_head_dependencies() {
    let report = clean(
        "---@generic T\n---@param ignored T\nlocal function make(ignored)\n---@class Factory\n---@overload fun<T>(value:T):T\nlocal factory={}\nreturn factory\nend\nlocal fromNumber=make(42)\nlocal fromString=make(\"outer\")\nlocal a=fromNumber(\"inner\")\nlocal b=fromString(84)",
    );
    assert_eq!(ty(&report, "a"), "string");
    assert_eq!(ty(&report, "b"), "number");
}

#[test]
fn an_alias_generic_constraint_alone_keeps_the_outer_parameter_dependency() {
    let source = "---@generic T\n---@param value T\nlocal function make(value)\n---@alias Identity fun<U:T>(value:U):U\n\n---@class Factory\n---@overload fun():Identity\nlocal factory={}\nreturn factory\nend\nlocal factory=make(42)\nlocal identity=factory()\nlocal result=identity(84)\n---@type 84\nlocal exact=identity(84)";
    let report = clean(source);
    assert_eq!(ty(&report, "result"), "number");
    assert_eq!(ty(&report, "exact"), "84");
    let wrong_result = source.replace("exact=identity(84)", "exact=identity(85)");
    assert!(check(&wrong_result).diagnostics.iter().any(|error| {
        error.code == "TS2322" && &wrong_result[error.span.start..error.span.end] == "identity(85)"
    }));
    let rejected = check(&format!("{source}\nlocal invalid=identity(\"wrong\")"));
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345"),
        "{:#?}",
        rejected.diagnostics
    );
}

#[test]
fn a_returned_overloaded_method_stays_in_the_actual_definition_view() {
    let source = "---@overload fun(value:number):number\n---@overload fun(value:string):string\n---@param value number|string|boolean\n---@return number|string|boolean\nlocal function echo(value) return value end\n---@class Holder\n---@field missing number\nlocal holder={method=echo}\nlocal function retrieve() return holder.method end\nlocal method=retrieve()\nlocal number=method(42)\nlocal string=method(\"text\")";
    let report = clean(source);
    assert_eq!(ty(&report, "number"), "number");
    assert_eq!(ty(&report, "string"), "string");
    let rejected = check(&format!("{source}\nlocal invalid=method(true)"));
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769")),
        "{:#?}",
        rejected.diagnostics
    );
}

#[test]
fn factory_result_instances_do_not_inherit_the_definition_call_heads() {
    let source = "---@class Point\n---@field x number\n\n---@class Factory\n---@overload fun():Point\nlocal factory={}\nlocal instance=factory()\nlocal value=instance.x";
    let report = clean(source);
    assert_eq!(ty(&report, "value"), "number");
    let rejected = check(&format!("{source}\nlocal invalid=instance()"));
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2349"),
        "{:#?}",
        rejected.diagnostics
    );
    let rejected = check(&format!("{source}\n---@type Point\nlocal raw={{x=42}}"));
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"),
        "{:#?}",
        rejected.diagnostics
    );
}

#[test]
fn actual_cli_reports_the_original_definition_overload_argument_then_accepts_correction() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-definition-overload-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("factory.ttslua");
    let source = "---@class Factory\n---@overload fun(value:number):number\nlocal factory={}\nlocal result=factory(\"wrong\")\n";
    let run = |source: &str| {
        fs::write(&path, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--dump-types"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let rejected = run(source);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("factory.ttslua:4:22: error[TS2345]"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(
        run(&source.replace("\"wrong\"", "42")).status.code(),
        Some(0)
    );
    fs::remove_dir_all(&directory).unwrap();
}
