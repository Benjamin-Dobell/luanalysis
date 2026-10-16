use luanalysis::check;

#[test]
fn returned_runtime_tables_keep_conditional_store_presence() {
    let source = "---@param flag boolean\nlocal function make(flag)\n local value={}\n if flag then value.item=1 end\n return value\nend\nlocal result=make(true)\nlocal optional=result.item\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let ty = report.type_at(source.find("optional=").unwrap()).unwrap();
    assert!(ty.contains("number") && ty.contains("nil"), "{ty}");
    let invalid = source.replace("local optional=", "---@type number\nlocal optional=");
    assert!(
        check(&invalid)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322")
    );
}

#[test]
fn inferred_plain_table_members_do_not_grant_class_identity() {
    let source = "---@class Owned\n---@field value number\nlocal Owned={value=1}\nlocal plain={}\nplain.value=1\n---@type Owned\nlocal invalid=plain\n";
    let report = check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322"),
        "{:#?}",
        report.diagnostics
    );
    let corrected = source.replace("local invalid=plain", "local invalid=Owned");
    let report = check(&corrected);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn plain_colon_methods_use_a_structural_receiver_and_check_public_arguments() {
    // The mutable number slot is a written contract; an unannotated initial
    // literal retains its original literal contract under Luanalysis rules.
    let source = "local Counter={\n---@type number\nvalue=1,\n}\n---@param amount number\n---@return number\nfunction Counter:add(amount)\n self.value=self.value+amount\n return self.value\nend\nlocal result=Counter:add(2)\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(source.find("result=").unwrap()),
        Some("number")
    );
    let invalid = source.replace("Counter:add(2)", "Counter:add('wrong')");
    assert!(
        check(&invalid)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2345")
    );
}

#[test]
fn returning_a_captured_table_does_not_export_its_future_contract_as_present() {
    let source = "local value={}\nlocal function same() return value end\nlocal alias=same()\nlocal direct=value.future\nlocal indirect=alias.future\nvalue.future=1\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(report.type_at(source.find("direct=").unwrap()), Some("nil"));
    assert_eq!(
        report.type_at(source.find("indirect=").unwrap()),
        Some("nil")
    );
    let invalid = source.replace("local indirect=", "---@type number\nlocal indirect=");
    assert!(
        check(&invalid)
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2322")
    );
}

// Lua evaluates the receiver before the right-hand side, so at runtime this
// store goes to the old table. As in TypeScript, a call's rebinding of the
// receiver is not tracked: the store updates its own spelling `value`, and the
// copy `old` is another spelling that does not see it (a known unsoundness).
#[test]
fn an_rhs_call_rebinding_the_receiver_is_not_tracked() {
    let source = "local value={}\nlocal old=value\n---@return number\nlocal function replace()\n value={}\n return 1\nend\nvalue.item=replace()\nlocal previous=old.item\nlocal current=value.item\n---@type number\nlocal invalid=value.item\n";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(report.type_at(source.find("current=").unwrap()), Some("number"));
    assert_eq!(report.type_at(source.find("previous=").unwrap()), Some("nil"));
}
