use luanalysis::check;
use std::{fs, process::Command};

const RECORDS: &str = r#"
---@param record {tag:true,child:number}
local function recordGuard(record)
    if not record.tag then
        record.child=42
    end
end
---@param seen table<number,true>
local function indexedGuard(seen)
    if not seen[1] then seen[1]=true end
end
---@param nullable table<number,nil|true>
local function nullableGuard(nullable)
    if not nullable[1] then nullable[1]=true end
end
"#;

#[test]
fn an_impossible_member_branch_does_not_turn_a_nonunion_receiver_into_never() {
    let report = check(RECORDS);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let offset = RECORDS.find("record.child=42").unwrap();
    let receiver = report.type_at(offset).unwrap();
    assert!(receiver.contains("child"), "{receiver}");
    assert!(!receiver.contains("never"), "{receiver}");
}

#[test]
fn impossible_member_branches_keep_the_written_storage_contracts() {
    let source = RECORDS
        .replace("record.child=42", "record.child='wrong'")
        .replace("seen[1]=true", "seen[1]=false");
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 2, "{:#?}", report.diagnostics);
    for diagnostic in &report.diagnostics {
        assert_eq!(diagnostic.code, "TS2322", "{diagnostic:#?}");
        assert!(matches!(
            &source[diagnostic.span.start..diagnostic.span.end],
            "'wrong'" | "false"
        ));
        assert!(!diagnostic.message.contains("never"), "{diagnostic:#?}");
    }
    let corrected = check(RECORDS);
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn a_discriminated_union_retains_its_original_union_after_selecting_one_constituent() {
    let source = "---@param record {tag:'a',a:number}|{tag:'b',b:string}\nlocal function read(record)\n if record.tag=='a' then\n  if record.tag=='b' then\n   local impossible=record.a\n  end\n end\nend\n";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
    assert!(report.diagnostics[0].message.contains("'never'"));
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "a"
    );
    let corrected = check(&source.replace("if record.tag=='b'", "if record.tag=='a'"));
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn cli_checks_nonunion_storage_after_an_impossible_property_guard() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis nonunion predicates {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    fs::write(&file, RECORDS).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(
        &file,
        RECORDS.replace("record.child=42", "record.child='wrong'"),
    )
    .unwrap();
    let wrong = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(wrong.status.code(), Some(1));
    let text = String::from_utf8(wrong.stderr).unwrap();
    assert!(text.contains("error[TS2322]"), "{text}");
    assert!(!text.contains("never"), "{text}");
    fs::write(&file, RECORDS).unwrap();
    let corrected = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
        .args(["check", "--lua-version", "5.2"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        corrected.status.success(),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    fs::remove_dir_all(&directory).unwrap();
}
