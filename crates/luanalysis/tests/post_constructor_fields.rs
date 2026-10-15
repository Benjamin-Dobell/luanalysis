use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis post fields {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.source(source);
        fixture
    }
    fn source(&self, source: &str) {
        fs::write(self.0.join("main.lua"), source).unwrap();
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args(["check", "--no-stdlib", "--dump-types", "main.lua"])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn pass(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
}
fn fail(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(text(output).contains(code), "{}", text(output));
}

#[test]
fn original_logger_scalar_fields_keep_mutable_schema_and_actual_literal_values() {
    let source = "---@class ge_tts__static_Logger\nlocal Logger = {}\nLogger.ERROR = 1\nLogger.WARNING = 2\n---@alias LogLevel 1 | 2\n---@type table<LogLevel, string>\nlocal prefixes = {[Logger.ERROR]='ERROR: ', [Logger.WARNING]='WARNING: '}\n---@type LogLevel\nlocal level = Logger.WARNING\nLogger.ERROR = 2\n---@type ge_tts__static_Logger\nlocal instance\n---@type number\nlocal inherited = instance.ERROR\nreturn Logger\n";
    let fixture = Fixture::new(source);
    pass(&fixture.check());
    fixture.source(&source.replace("Logger.ERROR = 2", "Logger.ERROR = 'wrong'"));
    fail(&fixture.check(), "TS2322");
    fixture.source(source);
    pass(&fixture.check());
}

#[test]
fn original_instance_written_field_checks_initializer_and_later_stores_at_actual_rhs() {
    let source = "---@class ge_tts__static_Instance\nlocal Instance = {}\n---@type string\nInstance.TYPE = 'Instance'\nInstance.TYPE = 'Derived'\n---@type string\nlocal identifier = Instance.TYPE\n";
    let fixture = Fixture::new(source);
    pass(&fixture.check());
    let invalid = source.replace("Instance.TYPE = 'Instance'", "Instance.TYPE = 42");
    fixture.source(&invalid);
    let output = fixture.check();
    fail(&output, "main.lua:4:17: error[TS2322]");
    assert!(!text(&output).contains("LUA2004"), "{}", text(&output));
    fixture.source(source);
    pass(&fixture.check());
    fixture.source(&source.replace("Instance.TYPE = 'Derived'", "Instance.TYPE = 42"));
    fail(&fixture.check(), "main.lua:5:17: error[TS2322]");
}

#[test]
fn written_singleton_field_contracts_are_preserved_when_copied_and_remain_mutable() {
    let source = "---@class Fields\nlocal Fields = {}\n---@type 1\nFields.one = 1\nFields.one = 1\nlocal copied = Fields.one\n";
    let fixture = Fixture::new(source);
    pass(&fixture.check());
    fixture.source(&format!("{source}Fields.one = 2\n"));
    fail(&fixture.check(), "TS2322");
    fixture.source(&format!("{source}copied = 2\n"));
    fail(&fixture.check(), "TS2322");
}

#[test]
fn broad_and_union_field_contracts_keep_the_original_schema_after_successive_stores() {
    let source = "---@class Fields\nlocal Fields = {}\n---@type number\nFields.broad = 1\nlocal copied = Fields.broad\ncopied = 2\n---@type 1 | 2\nFields.union = 1\nFields.union = 2\nFields.union = 1\n";
    let fixture = Fixture::new(source);
    pass(&fixture.check());
    fixture.source(&format!("{source}Fields.union = 3\n"));
    fail(&fixture.check(), "TS2322");
}

#[test]
fn assignment_recipes_keep_inherited_contracts_and_literal_origins() {
    let source = "---@class Base\n---@field value 1 | 2\n\n---@class Derived:Base\nlocal definition = {}\ndefinition.value = 1\nlocal copied = definition.value\ndefinition.value = 2\n";
    let fixture = Fixture::new(source);
    pass(&fixture.check());
    fixture.source(&format!("{source}copied = 3\n"));
    fail(&fixture.check(), "TS2322");
    fixture.source(&source.replace("definition.value = 2", "definition.value = 'wrong'"));
    let output = fixture.check();
    fail(&output, "main.lua:8:20: error[TS2322]");
    assert!(!text(&output).contains("TS2416"), "{}", text(&output));
    fixture.source(source);
    pass(&fixture.check());
}

#[test]
fn original_class_binding_ownership_excludes_aliases_shadows_and_ordinary_tables() {
    let source = "---@class Owned\nlocal original = {}\noriginal.value = 1\nlocal alias = original\nalias.other = 2\ndo\nlocal original = {}\noriginal.other = 3\nend\n";
    let fixture = Fixture::new(source);
    let output = fixture.check();
    fail(&output, "TS2339");
    fixture.source("local ordinary = {value=1}\nordinary.value = 2\n");
    pass(&fixture.check());
    fixture.source("local ordinary = {value=1}\nordinary.value = 'wrong'\n");
    fail(&fixture.check(), "TS2322");
}

#[test]
fn complete_instance_members_do_not_publish_definition_presence_before_a_store() {
    let source = "---@class Presence\nlocal Presence = {}\n---@type number\nlocal before = Presence.value\nPresence.value = 1\n---@type number\nlocal after = Presence.value\n";
    let fixture = Fixture::new(source);
    fail(&fixture.check(), "main.lua:4:16: error[TS2322]");
    fixture.source(&source.replace("---@type number\nlocal before = Presence.value\n", ""));
    pass(&fixture.check());
}

#[test]
fn original_post_field_contracts_keep_outer_generic_captures_in_separate_calls() {
    let source = "---@generic T\n---@param value T\nlocal function wrap(value)\n---@class Wrapped\nlocal Wrapped = {}\n---@type T\nWrapped.value = value\nreturn Wrapped\nend\nlocal numeric = wrap(42)\nlocal textual = wrap('word')\n---@type number\nlocal number_value = numeric.value\n---@type string\nlocal string_value = textual.value\n";
    let fixture = Fixture::new(source);
    pass(&fixture.check());
    fixture.source(&source.replace(
        "local string_value = textual.value",
        "local string_value = numeric.value",
    ));
    fail(&fixture.check(), "TS2322");
}

#[test]
fn unused_later_written_field_annotations_are_validated_at_original_type_sites() {
    let source = "---@class Fields\nlocal Fields = {}\nFields.value = 1\ndo return end\n---@type MissingFieldType\nFields.value = 2\n";
    let fixture = Fixture::new(source);
    fail(&fixture.check(), "main.lua:5:10: error[TS2304]");
    fixture.source(&source.replace("MissingFieldType", "number"));
    pass(&fixture.check());
}
