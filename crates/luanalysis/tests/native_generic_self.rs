use luanalysis::{CheckReport, check};

const FACTORY: &str = r#"---@generic T
---@param value T
local function make(value)
 ---@class Object
 local Object = {
  ---@type T
  value = value,
  ---@param self self
  ---@return T
  read = function(self) return self.value end,
 }
 ---@return T
 function Object:again() return self.value end
 ---@return self
 function Object:identity() return self end
 return Object
end
"#;
fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}
fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap()
        .ty
}

#[test]
fn generic_factory_definition_keeps_post_constructor_method_presence_and_self_mapper() {
    let source = format!(
        "{FACTORY}local numeric=make(1)\nlocal textual=make('word')\nlocal numeric_again=make(2)\n---@type number\nlocal first=numeric:read()\n---@type string\nlocal second=textual:again()\n---@type number\nlocal third=numeric_again:identity():again()\n"
    );
    let report = clean(&source);
    assert_eq!(ty(&report, "first"), "number");
    assert_eq!(ty(&report, "second"), "string");
    assert_eq!(ty(&report, "third"), "number");
    assert!(ty(&report, "numeric").starts_with("def Object<"));
}

#[test]
fn generic_definition_method_body_cannot_replace_an_arbitrary_t_with_string() {
    let source = FACTORY.replace(
        "function Object:again() return self.value end",
        "function Object:again() return 'wrong' end",
    );
    let report = check(&source);
    let error = report
        .diagnostics
        .iter()
        .find(|error| error.code == "TS2322")
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(&source[error.span.start..error.span.end], "'wrong'");
    clean(FACTORY);
}

#[test]
fn semantic_self_mapping_does_not_initialize_missing_members_of_a_generic_factory_class() {
    let source = format!(
        "{}local numeric=make(1)\nlocal rejected=numeric:identity()\n",
        FACTORY.replace(
            "---@class Object",
            "---@class Object\n ---@field missing boolean"
        )
    );
    // The class table is of its class before it has every member, as in v1.
    let report = check(&source);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|error| error.code != "TS2589" && error.code != "TS2345"),
        "{:#?}",
        report.diagnostics
    );
    clean(&source.replace("value = value,", "value = value, missing = true,"));
}

#[test]
fn generic_self_cli_error_keeps_original_utf8_crlf_range_and_accepts_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis generic self {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.lua");
    let source = format!("-- é😀\n{FACTORY}local textual=make('word')\n---@type number\nlocal wrong=textual:identity():again()\n").replace('\n', "\r\n");
    fs::write(&path, &source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .args(["check", "--no-stdlib", "--dump-types"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let output = run();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("main.lua:21:13: error[TS2322]"), "{text}");
    fs::write(&path, source.replace("---@type number", "---@type string")).unwrap();
    let output = run();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}
