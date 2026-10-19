use luanalysis::{CheckOptions, check, check_with_options};

const REPEATED: &str = r#"
---@generic T
---@param producer fun():T
---@return boolean,T...
local function repeat_results(producer)
    return true,producer(),producer(),producer()
end
---@return string,number
local function produce() return 'ok',42 end
---@type boolean,string|nil,string|number|nil,string|number|nil,string|number|nil
local flag,first,second,third,fourth=repeat_results(produce)
"#;

fn accepted(source: &str) {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
}

fn rejected(source: &str, code: &str) {
    let report = check(source);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].code, code,
        "{source}\n{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_variadic_generic_output_preserves_its_first_scalar_and_repeated_pack_element_domain() {
    accepted(REPEATED);
    rejected(
        &REPEATED.replace("boolean,string|nil", "boolean,number|nil"),
        "TS2322",
    );
    rejected(
        &REPEATED.replace("string|nil,string|number|nil", "string|nil,number|nil"),
        "TS2322",
    );
    rejected(
        &REPEATED.replace("string|nil,string|number|nil", "string|nil,string|nil"),
        "TS2322",
    );
    rejected(
        &REPEATED.replace("---@type boolean,", "---@type number,"),
        "TS2322",
    );
}

#[test]
fn repeated_generic_output_slots_remain_nullable_and_do_not_publish_body_cardinality() {
    accepted(REPEATED);
    for (written, narrowed) in [
        ("boolean,string|nil", "boolean,string"),
        ("string|nil,string|number|nil", "string|nil,string|number"),
    ] {
        rejected(&REPEATED.replace(written, narrowed), "TS2322");
    }
}

#[test]
fn a_repeated_generic_output_does_not_accept_an_unrelated_scalar_in_its_body() {
    for body in [
        "return true,producer(),false",
        "return true,false,producer()",
        "return false,producer(),producer(),producer()",
        "return true,42",
    ] {
        let source = REPEATED.replace("return true,producer(),producer(),producer()", body);
        if body.starts_with("return false,") {
            // A boolean prefix allows both true and false.
            accepted(&source);
        } else {
            rejected(&source, "TS2322");
        }
    }
    rejected(
        &REPEATED.replace("return true,producer()", "return 'wrong',producer()"),
        "TS2322",
    );
}

#[test]
fn repeated_result_packs_handle_single_and_empty_producers_without_inventing_values() {
    let scalar = REPEATED
        .replace(
            "---@return string,number\nlocal function produce() return 'ok',42 end",
            "---@return string\nlocal function produce() return 'ok' end",
        )
        .replace("string|number|nil", "string|nil");
    accepted(&scalar);
    rejected(
        &scalar.replace("string|nil,string|nil", "string|nil,number|nil"),
        "TS2322",
    );
    let empty = REPEATED
        .replace(
            "---@return string,number\nlocal function produce() return 'ok',42 end",
            "---@return void\nlocal function produce() end",
        )
        .replace(
            "boolean,string|nil,string|number|nil,string|number|nil,string|number|nil",
            "boolean,nil,nil,nil,nil",
        );
    accepted(&empty);
    rejected(
        &empty.replace("boolean,nil,nil,nil,nil", "boolean,string,nil,nil,nil"),
        "TS2322",
    );
}

#[test]
fn repeated_result_packs_include_the_original_producers_variadic_element_domain() {
    let source = REPEATED
        .replace(
            "---@return string,number\nlocal function produce() return 'ok',42 end",
            "---@return number,boolean...\nlocal function produce() return 42,true,false end",
        )
        .replace(
            "boolean,string|nil,string|number|nil,string|number|nil,string|number|nil",
            "boolean,number|nil,number|boolean|nil,number|boolean|nil,number|boolean|nil",
        );
    accepted(&source);
    rejected(
        &source.replace("number|nil,number|boolean|nil", "number|nil,number|nil"),
        "TS2322",
    );
    rejected(
        &source.replace("boolean,number|nil", "boolean,boolean|nil"),
        "TS2322",
    );
}

#[test]
fn a_scalar_generic_variadic_output_without_an_input_producer_remains_homogeneous() {
    let source = r#"
---@generic T
---@param value T
---@return T...
local function repeat_value(value) return value,value,value end
---@type number|nil,number|nil,number|nil
local first,second,third=repeat_value(42)
"#;
    accepted(source);
    rejected(
        &source.replace("number|nil,number|nil", "number|nil,string|nil"),
        "TS2322",
    );
    rejected(
        &source.replace("return value,value,value", "return value,'wrong',value"),
        "TS2322",
    );
}

#[test]
fn a_producer_generic_does_not_change_its_scalar_vararg_input_contract() {
    let source = REPEATED
        .replace(
            "---@return boolean,T...",
            "---@vararg T\n---@return boolean,T...",
        )
        .replace("repeat_results(producer)", "repeat_results(producer,...)")
        .replace("repeat_results(produce)", "repeat_results(produce,'extra')");
    accepted(&source);
    rejected(
        &source.replace(
            "repeat_results(produce,'extra')",
            "repeat_results(produce,42)",
        ),
        "TS2345",
    );
}

#[test]
fn a_repeated_result_sequence_keeps_its_generic_first_value_constraint() {
    let source = REPEATED.replace("---@generic T", "---@generic T:string");
    accepted(&source);
    rejected(
        &source.replace(
            "---@return string,number\nlocal function produce() return 'ok',42 end",
            "---@return number,string\nlocal function produce() return 42,'bad' end",
        ),
        "TS2345",
    );
}

#[test]
fn a_bounded_producer_preserves_its_required_first_scalar_in_a_generic_body() {
    let source = r#"
---@generic T:number
---@param producer fun():T
---@return T
local function first(producer)
    ---@type T
    local value=producer()
    return producer()
end
---@return number,string
local function produce() return 42,'ok' end
---@type number,string
local value,text=first(produce)
"#;
    accepted(source);
    rejected(
        &source.replace(
            "---@type T\n    local value",
            "---@type nil\n    local value",
        ),
        "TS2322",
    );
    let absent = source
        .replace(
            "---@return number,string\nlocal function produce() return 42,'ok' end",
            "---@return void\nlocal function produce() end",
        )
        .replace(
            "---@type number,string\nlocal value,text=first(produce)",
            "first(produce)",
        );
    rejected(&absent, "TS2345");
}

#[test]
fn nested_generic_repetition_preserves_rigid_elements_and_rejects_unrelated_body_values() {
    let source = REPEATED
        .replace(
            "---@return string,number\nlocal function produce()",
            r#"---@generic U
---@param producer fun():U
---@return boolean,U...
local function nested(producer) return repeat_results(producer) end
---@return string,number
local function produce()"#,
        )
        .replace("=repeat_results(produce)", "=nested(produce)");
    accepted(&source);
    rejected(
        &source.replace(
            "return repeat_results(producer) end",
            "return true,producer(),false end",
        ),
        "TS2322",
    );
    rejected(
        &source.replace("string|nil,string|number|nil", "string|nil,number|nil"),
        "TS2322",
    );
}

#[test]
fn a_variadic_generic_result_accepts_zero_or_one_forwarded_producer_sequences() {
    accepted(&REPEATED.replace(
        "return true,producer(),producer(),producer()",
        "return true,producer()",
    ));
    accepted(&REPEATED.replace(
        "return true,producer(),producer(),producer()",
        "return true",
    ));
    rejected(
        &REPEATED.replace(
            "return true,producer(),producer(),producer()",
            "return producer()",
        ),
        "TS2322",
    );
    rejected(
        &REPEATED.replace(
            "return true,producer(),producer(),producer()",
            "return producer(),producer()",
        ),
        "TS2322",
    );
    let required = REPEATED
        .replace("---@return boolean,T...", "---@return boolean,string,T...")
        .replace(
            "return true,producer(),producer(),producer()",
            "return true",
        );
    rejected(&required, "TS2322");
}

#[test]
fn a_constrained_producer_requires_a_physical_first_result_in_both_nil_modes() {
    let source = r#"
---@generic T:number
---@param producer fun():T
---@return T
local function forward(producer) return producer() end
---@return void
local function produce() end
forward(produce)
"#;
    for strict_nil in [true, false] {
        let options = CheckOptions {
            strict_nil,
            ..Default::default()
        };
        let report = check_with_options(source, options.clone());
        assert_eq!(
            report.diagnostics.len(),
            1,
            "strict_nil={strict_nil}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2345");
        let direct = source.replace(
            "---@generic T:number\n---@param producer fun():T\n---@return T",
            "---@param producer fun():number\n---@return number",
        );
        let report = check_with_options(&direct, options.clone());
        assert_eq!(
            report.diagnostics.len(),
            1,
            "strict_nil={strict_nil}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2345");
        let corrected = source.replace(
            "---@return void\nlocal function produce() end",
            "---@return number\nlocal function produce() return 42 end",
        );
        let report = check_with_options(&corrected, options);
        assert!(
            report.diagnostics.is_empty(),
            "strict_nil={strict_nil}: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn permissive_nil_keeps_one_physical_nil_distinct_from_an_empty_generic_result_sequence() {
    let source = r#"
---@generic T
---@param producer fun():T
---@return T
local function forward(producer) return producer() end
---@return nil
local function produce() return nil end
---@return void
local function empty() return forward(produce) end
"#;
    for strict_nil in [true, false] {
        let options = CheckOptions {
            strict_nil,
            ..Default::default()
        };
        let report = check_with_options(source, options.clone());
        assert_eq!(
            report.diagnostics.len(),
            1,
            "strict_nil={strict_nil}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2322");
        let corrected = source.replace(
            "---@return nil\nlocal function produce() return nil end",
            "---@return void\nlocal function produce() end",
        );
        let report = check_with_options(&corrected, options);
        assert!(
            report.diagnostics.is_empty(),
            "strict_nil={strict_nil}: {:#?}",
            report.diagnostics
        );
    }
    let bounded = source
        .replace("---@generic T\n", "---@generic T:number\n")
        .replace(
            "---@return void\nlocal function empty() return forward(produce) end",
            "forward(produce)",
        );
    let strict = check_with_options(&bounded, CheckOptions::default());
    assert_eq!(strict.diagnostics.len(), 1, "{:#?}", strict.diagnostics);
    assert_eq!(strict.diagnostics[0].code, "TS2345");
    let permissive = check_with_options(
        &bounded,
        CheckOptions {
            strict_nil: false,
            ..Default::default()
        },
    );
    assert!(
        permissive.diagnostics.is_empty(),
        "{:#?}",
        permissive.diagnostics
    );
}

#[test]
fn a_required_symbolic_first_result_survives_nested_bounded_forwarding_in_both_nil_modes() {
    let source = r#"
---@generic T:number
---@param producer fun():T
---@return T
local function forward(producer) return producer() end
---@generic U:number
---@param producer fun():U
---@return U
local function nested(producer)
    ---@type U
    local first=producer()
    return forward(producer)
end
---@return number,string
local function produce() return 42,'tail' end
---@type number,string
local first,second=nested(produce)
"#;
    for strict_nil in [true, false] {
        let options = CheckOptions {
            strict_nil,
            ..Default::default()
        };
        let report = check_with_options(source, options.clone());
        assert!(
            report.diagnostics.is_empty(),
            "strict_nil={strict_nil}: {:#?}",
            report.diagnostics
        );
        let wrong = source.replace(
            "---@type U\n    local first",
            "---@type nil\n    local first",
        );
        let report = check_with_options(&wrong, options.clone());
        assert_eq!(
            report.diagnostics.len(),
            1,
            "strict_nil={strict_nil}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2322");
        let absent = source
            .replace(
                "---@return number,string\nlocal function produce() return 42,'tail' end",
                "---@return void\nlocal function produce() end",
            )
            .replace(
                "---@type number,string\nlocal first,second=nested(produce)",
                "nested(produce)",
            );
        let report = check_with_options(&absent, options);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "strict_nil={strict_nil}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2345");
    }
}
