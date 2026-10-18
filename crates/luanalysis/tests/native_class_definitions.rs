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
fn rejects(source: &str, code: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

#[test]
fn a_class_table_reads_its_declared_members_as_its_class_does() {
    // As v1 types a class table as its class, a member it has not been given yet reads as the
    // class declares it, not as possibly nil.
    let report = clean("---@class Point\n---@field x number\nlocal Point={}\nlocal value=Point.x");
    assert_eq!(ty(&report, "value"), "number");
}

#[test]
fn deferred_original_namespace_bodies_use_declared_methods_without_populating_chunk_reads() {
    for methods in [
        "---@return number\nfunction Namespace.value() return 1 end\n---@return number\nfunction Namespace.first() return Namespace.value() end\n",
        "---@return number\nfunction Namespace.first() return Namespace.value() end\n---@return number\nfunction Namespace.value() return 1 end\n",
    ] {
        let source = format!("---@class Namespace\nlocal Namespace={{}}\nlocal before=Namespace.value\n{methods}local result=Namespace.first()\n");
        let report = clean(&source);
        assert!(!ty(&report, "before").contains("nil"), "{}", ty(&report, "before"));
        assert_eq!(ty(&report, "result"), "number");
        clean(&source.replace("local before=Namespace.value", "local before=Namespace.value()"));
    }
}

#[test]
fn deferred_namespace_policy_keeps_explicit_optional_calls_and_captured_self_partial() {
    rejects("---@class Namespace\n---@field maybe nil | fun():number\nlocal Namespace={}\n---@return number\nfunction Namespace.read() return Namespace.maybe() end\n", "TS2349");
    let source = "---@class Namespace\nlocal Namespace={}\nlocal self=Namespace\n---@return number\nlocal function early() return self.value() end\n---@return number\nfunction Namespace.value() return 1 end\nreturn early\n";
    clean(source);
    let corrected = "---@class Namespace\nlocal Namespace={}\n---@return number\nfunction Namespace.value() return 1 end\nlocal self=Namespace\n---@return number\nlocal function early() return self.value() end\nreturn early\n";
    clean(corrected);
}

#[test]
fn cli_original_json_sparse_fill_uses_its_declared_namespace_function() {
    let directory = std::env::temp_dir().join(format!("luanalysis-json-namespace-presence-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    let source = include_str!("fixtures/native_effects/json_sparse_fill.lua");
    let invoke = |source: &str| {
        std::fs::write(&path, source).unwrap();
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&directory).args(["check", "--lua-version", "5.4", "main.lua"]).output().unwrap()
    };
    let correct = invoke(source);
    assert!(correct.status.success(), "{}", String::from_utf8_lossy(&correct.stderr));
    let wrong = invoke(&source.replace("return NULL", "return 'wrong'"));
    assert_eq!(wrong.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("TS2322"), "{}", String::from_utf8_lossy(&wrong.stderr));
    let corrected = invoke(source);
    assert!(corrected.status.success(), "{}", String::from_utf8_lossy(&corrected.stderr));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn supplied_known_fields_are_checked_without_fabricating_missing_fields() {
    let source =
        "---@class Point\n---@field x number\n---@field y number\nlocal point={x=\"wrong\"}";
    let report = rejects(source, "TS2322");
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "LUA2031"),
        "{:#?}",
        report.diagnostics
    );
    clean(&source.replace("\"wrong\"", "42"));
}

#[test]
fn original_known_member_writes_do_not_admit_missing_slot_nil_or_wrong_types() {
    for value in ["nil", "\"wrong\""] {
        let source =
            format!("---@class Point\n---@field x number\nlocal point={{}}\npoint.x={value}");
        let report = rejects(&source, "TS2322");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| &source[diagnostic.span.start..diagnostic.span.end] == value),
            "{:#?}",
            report.diagnostics
        );
    }
    let report = clean(
        "---@class Point\n---@field x number\nlocal point={}\npoint.x=42\nlocal value=point.x",
    );
    assert_eq!(ty(&report, "value"), "number");
}

#[test]
fn declaration_names_do_not_change_original_initializer_shadowing() {
    let report = clean(
        "local object=42\n---@class DifferentName\n---@field x number\nlocal object=({x=object})\nlocal value=object.x",
    );
    assert_eq!(ty(&report, "value"), "number");
}

#[test]
fn a_class_table_is_a_value_of_its_class_before_it_has_every_member() {
    // As in v1, the class table is of its class.
    clean(
        "---@class Point\n---@field x number\nlocal point={}\n---@param value Point\nlocal function consume(value) end\nconsume(point)",
    );
    clean(
        "---@class Point\n---@field x number\nlocal point={x=42}\n---@param value Point\nlocal function consume(value) end\nconsume(point)",
    );
}

#[test]
fn unrelated_origins_and_ordinary_raw_tables_remain_nominal_failures() {
    rejects(
        "---@class Point\n---@field x number\nlocal point={x=42}\n---@type Point\nlocal raw={x=42}",
        "TS2322",
    );
    rejects(
        "---@class A\nlocal a={}\n---@class B\nlocal b={}\n---@param value B\nlocal function consume(value) end\nconsume(a)",
        "TS2345",
    );
    clean(
        "---@class A\nlocal a={}\n---@param value A\nlocal function consume(value) end\nconsume(a)",
    );
}

#[test]
fn original_constructor_members_are_part_of_the_instance_schema() {
    let report = clean(
        "---@class Point\n---@field x number\nlocal point={x=42,extra=\"runtime\"}\nlocal value=point.extra",
    );
    assert_eq!(ty(&report, "value"), "string");
    let source = "---@class Point\n---@field x number\nlocal point={x=42,extra=\"runtime\"}\n---@param instance Point\nlocal function read(instance) return instance.extra end";
    let report = clean(source);
    assert_eq!(ty(&report, "read"), "fun(instance: Point): \"runtime\"");
    let wrong = format!("{source}\npoint.extra=42\n");
    let rejected = rejects(&wrong, "TS2322");
    let value = wrong.rfind("42").unwrap();
    assert!(rejected.diagnostics.iter().any(|error| error.code == "TS2322"
        && error.span.start == value
        && error.span.end == value + 2));
    clean(&wrong.replace("point.extra=42", "point.extra=\"runtime\""));
}

#[test]
fn actual_literal_callbacks_receive_the_declared_member_context() {
    let report = clean(
        "---@class Callbacks\n---@field read fun(value:{x:number}):number\nlocal callbacks={read=function(value) return value.x end}\nlocal result=callbacks.read({x=42})",
    );
    assert_eq!(ty(&report, "result"), "number");
}

#[test]
fn an_explicit_static_class_uses_an_ordinary_new_field_and_explicit_instance_assertion() {
    let report = clean(
        "---@class Point\n---@field x number\n\n---@class Factory\n---@field new fun(x:number):Point\nlocal unrelatedRuntimeName={new=function(x) return --[[---@type Point]] {x=x} end}\nlocal instance=unrelatedRuntimeName.new(42)\nlocal result=instance.x",
    );
    assert_eq!(ty(&report, "result"), "number");
}

#[test]
fn aliases_captures_and_branches_retain_the_definition_write_contract() {
    let source = "---@class Point\n---@field x number\nlocal point={}\nlocal alias=point\nlocal function mutate() alias.x=\"wrong\" end\nmutate()";
    rejects(source, "TS2322");
    clean(&source.replace("\"wrong\"", "42"));
}

#[test]
fn captured_outer_type_parameters_map_both_class_and_partial_view() {
    let report = clean(
        "---@generic T\n---@param value T\nlocal function outer(value)\n---@class Captured\n---@field value T\nlocal definition={value=value}\nreturn function() return definition.value end\nend\nlocal number=outer(42)\nlocal string=outer(\"word\")\nlocal a=number()\nlocal b=string()",
    );
    assert_eq!(ty(&report, "a"), "number");
    assert_eq!(ty(&report, "b"), "string");
}

#[test]
fn a_generic_rhs_uses_the_declared_write_type_instead_of_the_nullable_read_type() {
    for target in ["point.x", "point[\"x\"]"] {
        let report = clean(&format!(
            "---@generic T\n---@return T\nlocal function make()\n---@type T\nlocal value\nreturn value\nend\n---@class Point\n---@field x number\nlocal point={{}}\n{target}=make()\nlocal result=point.x"
        ));
        assert_eq!(ty(&report, "result"), "number");
    }
}

#[test]
fn dynamic_initializer_keys_cannot_hide_a_wrong_declared_member_type() {
    let source = "---@param key string\nlocal function build(key)\n---@class C\n---@field value number\nlocal definition={[key]=\"wrong\"}\nreturn definition\nend\nlocal result=build(\"value\")";
    rejects(source, "TS2322");
    clean(&source.replace("\"wrong\"", "42"));
    clean(
        "---@param key number\nlocal function build(key)\n---@class C\n---@field value number\nlocal definition={[key]=\"unrelated\"}\nreturn definition\nend\nlocal result=build(1)",
    );
}

#[test]
fn dynamic_definition_writes_cannot_delete_or_violate_a_declared_member() {
    for value in ["nil", "\"wrong\""] {
        let source = format!(
            "---@param key string\nlocal function build(key)\n---@class C\n---@field value number\nlocal definition={{[key]=42}}\ndefinition[key]={value}\nreturn definition\nend\nlocal result=build(\"value\")"
        );
        rejects(&source, "TS2322");
        clean(&source.replace(&format!("definition[key]={value}"), "definition[key]=84"));
    }
}

#[test]
fn generic_constructor_collisions_retain_value_and_deletion_candidates() {
    for (value, alternative) in [("\"word\"", "\"word\""), ("nil", "nil")] {
        let source = format!(
            "---@generic K:string\n---@param key K\nlocal function build(key) return {{value=42,[key]={value}}} end\nlocal result=build(\"value\").value\n---@type number\nlocal invalid=result"
        );
        let report = rejects(&source, "TS2322");
        assert!(
            ty(&report, "result").contains("42")
                && ty(&report, "result").contains(alternative),
            "{}",
            ty(&report, "result")
        );
        clean(&source.replace("---@type number", &format!("---@type number|{alternative}")));
    }
}

#[test]
fn explicit_type_generic_multiple_and_nonlocal_definition_forms_stay_gated() {
    for source in [
        "---@class C\n---@type C\nlocal value={}",
        "---@class C<T>\n---@field value T\nlocal value={}",
        "---@class C\nlocal a,b={},{}",
        "---@class C\nnamespace.value={}",
    ] {
        let report = rejects(source, "LUA2031");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "LUA2031"
                    && &source[diagnostic.span.start..diagnostic.span.end] == "C"),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn actual_cli_checks_definition_initializer_then_instance_completeness() {
    use std::{fs, process::Command};
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-class-definition-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("class.ttslua");
    let run = |source: &str| {
        fs::write(&path, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--dump-types"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let empty = "---@class Point\n---@field x number\nlocal point={}\n";
    assert_eq!(run(empty).status.code(), Some(0));
    let wrong = "---@class Point\n---@field x number\nlocal point={x=\"wrong\"}\n";
    let rejected = run(wrong);
    assert_eq!(rejected.status.code(), Some(1));
    let errors = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        errors.contains("class.ttslua:3:13: error[TS2322]"),
        "{errors}"
    );
    assert_eq!(
        run(&wrong.replace("\"wrong\"", "42")).status.code(),
        Some(0)
    );
    let partial = format!(
        "{empty}---@param value Point\nlocal function consume(value) end\nconsume(point)\n"
    );
    assert_eq!(run(&partial).status.code(), Some(0));
    assert_eq!(
        run(&partial.replace("local point={}", "local point={x=42}"))
            .status
            .code(),
        Some(0)
    );
    fs::remove_dir_all(&directory).unwrap();
}
