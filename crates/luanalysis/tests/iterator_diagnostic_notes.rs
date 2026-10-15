use std::{fs, process::Command};

#[test]
fn iterator_feedback_keeps_concrete_types_and_its_explanation_in_text_and_json() {
    let source = "---@param state nil\n---@param control number\n---@return nil|(false,string)\nlocal function iterator(state,control) return false,'value' end\nfor key,value in iterator,nil,0 do end\n";
    let directory = std::env::temp_dir().join(format!(
        "luanalysis iterator diagnostic note {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("input.lua");
    fs::write(&file, source).unwrap();
    for format in ["text", "json"] {
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--no-stdlib", "--format", format])
            .arg(&file)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.code(), Some(1), "{text}");
        assert!(text.contains("TS2345"), "{text}");
        assert!(text.contains("Argument of type 'false' is not assignable to parameter of type 'number'. The previous first iterator result supplies the next control argument."), "{text}");
        assert!(!text.contains("contextual type"), "{text}");
        if format == "json" {
            assert!(output.stderr.is_empty(), "{text}");
        }
    }
    fs::write(
        &file,
        source.replace(
            "---@param control number",
            "---@param control number|boolean",
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--no-stdlib"])
        .arg(&file)
        .output()
        .unwrap();
    fs::remove_dir_all(&directory).unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}
