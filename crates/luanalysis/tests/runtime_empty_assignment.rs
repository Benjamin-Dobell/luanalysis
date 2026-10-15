use luanalysis::check;

#[test]
fn fresh_replacement_of_an_inferred_empty_binding_keeps_the_new_allocation() {
    let source = "local record={}\nlocal previous=record\nrecord={new='word'}\n---@type string\nlocal result=record.new\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(report.type_at(source.find("result=").unwrap()), Some("string"));
    let invalid = source.replace("---@type string", "---@type number");
    assert!(check(&invalid).diagnostics.iter().any(|error| error.code == "TS2322"));
    let previous = format!("{source}local absent=previous.new\n");
    assert!(check(&previous).diagnostics.iter().any(|error| error.code == "TS2339"), "the old allocation cannot gain the replacement's members");
}

#[test]
fn written_empty_shapes_accept_extras_but_keep_the_declared_member_view() {
    let source = "---@type {}\nlocal record={}\nrecord={new='word'}\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let hidden = format!("{source}local hidden=record.new\n");
    assert!(check(&hidden).diagnostics.iter().any(|error| error.code == "TS2339"), "written annotations still control exposed members");
    let nested = "---@type {child:{}}\nlocal record={child={extra=true}}\n";
    assert!(check(nested).diagnostics.is_empty());
}

#[test]
fn known_structural_nominal_and_runtime_domains_keep_their_contracts() {
    for source in [
        "---@type {known:number}\nlocal record={known=1,new='word'}\n",
        "local record={old=1}\nrecord={new='word'}\n",
        "---@type table\nlocal record={}\nrecord=function() return 1 end\n",
        "---@type table\nlocal record={}\nrecord=1\n",
        "---@class Owned\nlocal Owned={}\n---@type Owned\nlocal record=Owned\nrecord={new='word'}\n",
    ] {
        let report = check(source);
        assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"), "{source}\n{:#?}", report.diagnostics);
    }
}
