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

#[test]
fn missing_union_member_preserves_nil_and_does_not_widen_to_any() {
    let source = "---@param v {x:number}|{[1]:number}\nlocal function read(v)\nlocal selected=v.x\n---@type number|nil\nlocal accepted=selected\n---@type number\nlocal rejected=selected\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let ty = report
        .type_at(source.find("selected=v.x").unwrap() + "selected=v.".len())
        .unwrap();
    assert!(ty.contains("number") && ty.contains("nil"), "{ty}");
    assert!(!ty.contains("any"), "{ty}");
}

#[test]
fn truthy_member_filters_original_union_for_numeric_fallback() {
    let source = "---@param v {x:number}|{[1]:number}\nlocal function read(v)\nlocal selected=v.x or v[1]\n---@type number\nlocal accepted=selected\nreturn selected\nend\nread({x=0})\nread({[1]=4})";
    let report = clean(source);
    assert_eq!(
        report.type_at(source.find("accepted=selected").unwrap() + "accepted=".len()),
        Some("number")
    );
}

#[test]
fn string_fallback_is_retained_and_rejected_as_a_number() {
    let source = "---@param v {x:number}|{[1]:string}\nlocal function read(v)\nlocal selected=v.x or v[1]\n---@type number\nlocal rejected=selected\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn wholly_unknown_union_key_remains_an_error() {
    let source = "---@param v {x:number}|{[1]:number}\nlocal function read(v)\nreturn v.typo\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
}

#[test]
fn union_reads_do_not_accept_nil_or_non_table_receivers() {
    for receiver in ["nil", "number", "boolean", "fun():number"] {
        let source =
            format!("---@param v {{x:number}}|{receiver}\nlocal function read(v)\nreturn v.x\nend");
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}\n{:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2339");
    }
}

#[test]
fn writes_still_require_a_contract_in_every_union_constituent() {
    let source = "---@param v {x:number}|{[1]:number}\nlocal function write(v)\nv.x=2\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
}

#[test]
fn inherited_members_filter_original_nominal_constituents() {
    let source = r#"---@class Base<T>
---@field x T

---@class Char:Base<number>

---@class Numeric
---@field [1] number

---@param v Char|Numeric
local function read(v)
if v.x then
---@type Char
local original=v
---@type Base<number>
local ancestor=original
---@type number
local field=ancestor.x
else
---@type Numeric
local original=v
---@type number
local field=original[1]
end
---@type number
local result=v.x or v[1]
return result
end
"#;
    clean(source);
    let wrong = source.replace(
        "---@type Char\nlocal original=v",
        "---@type Numeric\nlocal original=v",
    );
    let report = check(&wrong);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn explicit_nil_comparison_and_short_circuit_composites_narrow_parent() {
    clean(
        "---@param v {x:number}|{[1]:number}\nlocal function read(v)\nif v.x == nil then\n---@type number\nlocal value=v[1]\nend\n---@type number\nlocal value=(v.x ~= nil and v.x) or v[1]\nend",
    );
}

#[test]
fn false_values_do_not_get_discarded_like_absent_members() {
    let source = "---@param v {x:false}|{[1]:number}\nlocal function read(v)\nlocal value=v.x or v[1]\n---@type number|nil\nlocal accepted=value\n---@type number\nlocal rejected=value\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn predicates_on_another_receiver_do_not_narrow_this_union() {
    let source = "---@param v {x:number}|{[1]:number}\n---@param other {x:number|nil}\nlocal function read(v,other)\nif other.x then\n---@type number\nlocal rejected=v.x\nend\nend";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
}

#[test]
fn disjoint_numeric_indexer_contributes_nil_to_a_named_union_read() {
    clean(
        "---@param v {x:number}|{[number]:number}\nlocal function read(v)\n---@type number|nil\nlocal selected=v.x\n---@type number\nlocal value=v.x or v[1]\nend",
    );
}

#[test]
fn actual_cli_accepts_numeric_fallback_and_rejects_its_wrong_type() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-union-member-cli-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("original.lua");
    let source = "-- original Lua: café\r\n---@param v {x:number}|{[1]:number}\r\nlocal function read(v)\r\nlocal value=v.x or v[1]\r\n---@type number\r\nlocal accepted=value\r\nend\r\n";
    std::fs::write(&path, source).unwrap();
    let invoke = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--no-stdlib", "--format", "text"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let accepted = invoke();
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stdout)
    );
    let wrong = source.replace("{[1]:number}", "{[1]:string}");
    std::fs::write(&path, &wrong).unwrap();
    let rejected = invoke();
    assert_eq!(rejected.status.code(), Some(1));
    let output = String::from_utf8(rejected.stderr).unwrap();
    assert!(output.contains("TS2322"), "{output}");
    assert!(output.contains("original.lua:6:16:"), "{output}");
    assert!(!output.contains("TS2339"), "{output}");
    std::fs::write(&path, source).unwrap();
    let corrected = invoke();
    assert!(
        corrected.status.success(),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_broad_key_does_not_erase_possible_values_from_a_closed_union_constituent() {
    let report = check(
        "---@param v {x:number}|{[string]:string}\n---@param key string\nlocal function read(v,key)\nreturn v[key]\nend",
    );
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS7053");
}
