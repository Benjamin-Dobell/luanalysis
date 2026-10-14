use luanalysis::{CheckReport, check, location};

fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .unwrap_or_else(|| panic!("missing binding {name}: {:#?}", report.diagnostics))
        .ty
}

#[test]
fn numeric_vector_function_uses_native_inferred_return_and_original_errors() {
    // The primitive part of ge_tts/Vector2's component arithmetic, reduced to
    // an annotated local function so no class/metatable policy is assumed.
    let source = "---@param x number\n---@param y number\nlocal function add(x, y) return x + y end\nlocal result = add(2, 3)\n---@type number\nlocal checked = result";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(ty(&report, "result"), "number");
    assert_eq!(
        report.type_at(source.find("x + y").unwrap()),
        Some("number")
    );

    let invalid = source.replace("add(2, 3)", "add(2, \"bad\")");
    let report = check(&invalid);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
    assert_eq!(
        &invalid[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"bad\""
    );
    assert_eq!(location(&invalid, report.diagnostics[0].span), (4, 23));
}

#[test]
fn primitive_arithmetic_bitwise_and_comparisons_check_operand_types() {
    let accepted = check(
        "local a = 1 + 2 * 3 ^ 2\nlocal b = 7 // 2 % 2\nlocal c = ~3 & 7 | (2 ~ 1)\nlocal d = 1 << 2 >> 1\nlocal e = -2 / 4 - 1\nlocal n = 1 < 2\nlocal s = \"a\" <= \"b\"\nlocal equal = nil == false",
    );
    assert!(
        accepted.diagnostics.is_empty(),
        "{:#?}",
        accepted.diagnostics
    );
    for name in ["a", "b", "c", "d", "e"] {
        assert_eq!(ty(&accepted, name), "number");
    }
    for name in ["n", "s", "equal"] {
        assert_eq!(ty(&accepted, name), "boolean");
    }

    for (source, code, origin) in [
        ("local bad = \"text\" + 1", "TS2362", "\"text\""),
        ("local bad = 1 + \"text\"", "TS2363", "\"text\""),
        ("local bad = -false", "TS2356", "false"),
        ("local bad = true & 1", "TS2362", "true"),
        ("local bad = 1 < \"text\"", "TS2365", "<"),
    ] {
        let report = check(source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, code);
        assert_eq!(
            &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
            origin
        );
    }
}

#[test]
fn concatenation_and_length_use_lua_domains_without_js_addition_coercion() {
    let accepted = check(
        "local text = \"card \" .. 7 .. \" ready\"\nlocal size = #\"\"\nlocal fields = #{x = 1}\nlocal nested = #(\"word\")",
    );
    assert!(
        accepted.diagnostics.is_empty(),
        "{:#?}",
        accepted.diagnostics
    );
    assert_eq!(ty(&accepted, "text"), "string");
    for name in ["size", "fields", "nested"] {
        assert_eq!(ty(&accepted, name), "number");
    }

    for source in [
        "local bad = \"a\" .. nil",
        "local bad = false .. \"b\"",
        "local bad = #false",
        "local bad = #7",
        "local bad = #nil",
        "local function f() return 1 end\nlocal bad = #f",
    ] {
        let report = check(source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "LUA2020");
    }
}

#[test]
fn selected_table_literals_retain_native_excess_property_freshness() {
    // The native union boundary must preserve freshness metadata, rather than
    // changing the relation policy according to which expression selected it.
    for expression in [
        "{x = 1, extra = true}",
        "true and {x = 1, extra = true}",
        "false or {x = 1, extra = true}",
    ] {
        let source = format!("---@type {{x: number}}\nlocal value = {expression}");
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{expression}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2322");
    }
    // Ordinary local materialization still removes literal freshness.
    let staged = check(
        "local wider = true and {x = 1, extra = true}\n---@type {x: number}\nlocal value = wider",
    );
    assert!(staged.diagnostics.is_empty(), "{:#?}", staged.diagnostics);
}

#[test]
fn zero_and_empty_string_are_truthy_and_unreachable_rhs_is_not_demanded() {
    let source = "local zeroOr = 0 or \"fallback\"\nlocal emptyOr = \"\" or 42\nlocal zeroAnd = 0 and \"chosen\"\nlocal emptyAnd = \"\" and 42\nlocal skippedAnd = false and missing\nlocal skippedOr = true or missing\nlocal notZero = not 0\nlocal notEmpty = not \"\"\nlocal notNil = not nil\nlocal notFalse = not false";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(ty(&report, "zeroOr"), "number");
    assert_eq!(ty(&report, "emptyOr"), "string");
    assert_eq!(ty(&report, "zeroAnd"), "string");
    assert_eq!(ty(&report, "emptyAnd"), "number");
    for (spelling, expected) in [
        ("not 0", "false"),
        ("not \"\"", "false"),
        ("not nil", "true"),
        ("not false", "true"),
    ] {
        assert_eq!(
            report.type_at(source.find(spelling).unwrap()),
            Some(expected)
        );
    }
    let demanded = check("local bad = 0 and missing");
    assert_eq!(demanded.diagnostics.len(), 1);
    assert_eq!(demanded.diagnostics[0].code, "TS2304");
}

#[test]
fn short_circuit_unions_narrow_only_the_conditional_rhs_reference() {
    let source = "---@param value? number\nlocal function fallback(value) return (value or 0) + 1 end\nlocal result = fallback(nil)\n---@param record {x: number}|nil\nlocal function read(record)\n  local safe = record and record.x\n  return safe\nend\nlocal selected = read({x = 1})\n---@param flag boolean\nlocal function choose(flag) return flag and \"yes\" end";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(ty(&report, "result"), "number");
    assert!(ty(&report, "selected").contains("number"));
    assert!(ty(&report, "selected").contains("nil"));
    let selected = report
        .type_at(source.find("flag and").unwrap() + "flag ".len())
        .unwrap();
    assert!(selected.contains("false"), "{selected}");
    assert!(selected.contains("\"yes\""), "{selected}");
    assert!(!selected.contains("true"), "{selected}");

    let nested = "---@param object {child: {x: number}|nil}\nlocal function read(object) return object.child and object.child.x end";
    let report = check(nested);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(
        report.type_at(nested.find("object.child.x").unwrap() + "object.child.".len()),
        Some("number")
    );

    let invalid = "---@param value? number\nlocal function arithmetic(value) return value + 1 end";
    let report = check(invalid);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2362");
    assert_eq!(
        &invalid[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "value"
    );

    let restored = "---@param record {x: number}|nil\nlocal function read(record)\n  local safe = record and record.x\n  return record.x\nend";
    let report = check(restored);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");

    let restored_field = "---@param object {child: {x: number}|nil}\nlocal function read(object)\n  local safe = object.child and object.child.x\n  return object.child.x\nend";
    let report = check(restored_field);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
}

#[test]
fn unknown_operator_values_reject_and_dynamic_values_remain_explicit() {
    let invalid = "---@param value unknown\nlocal function arithmetic(value) return value + 1 end";
    let report = check(invalid);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2362");

    let dynamic = "---@param value any\nlocal function arithmetic(value) return value + 1 end\nlocal result = arithmetic(\"dynamic\")";
    let report = check(dynamic);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(ty(&report, "result"), "number");
}

#[test]
fn literal_string_indexing_uses_original_member_facts_without_key_coercion() {
    let source = "---@type {x: number|string}\nlocal item = {x = 1}\nitem.x = \"chosen\"\n---@type string\nlocal chosen = item[\"x\"]\nlocal parenthesized = item[(\"x\")]";
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(ty(&report, "chosen"), "string");
    assert_eq!(ty(&report, "parenthesized"), "string");
    let missing = "local item = {x = 1}\nlocal invalid = item[\"absent\"]";
    let report = check(missing);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "TS2339");
    assert_eq!(
        &missing[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"absent\""
    );
    for (source, original) in [
        ("local item = {x = 1}\nlocal invalid = item[1]", "1"),
        (
            "local item = {x = 1}\nlocal key = \"x\"\nlocal invalid = item[key]",
            "key",
        ),
        (
            "local item = {x = 1}\nlocal invalid = item[\"\\255\"]",
            "\"\\255\"",
        ),
    ] {
        let report = check(source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}: {:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2339");
        assert_eq!(
            &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
            original
        );
    }

    let known = "local item = {x = 1}\n---@type \"x\"\nlocal key = \"x\"\nlocal valid = item[key]";
    let report = check(known);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(ty(&report, "valid"), "number");
    assert_eq!(report.type_at(known.rfind("key]").unwrap()), Some("\"x\""));

    let broad = "---@param key string\nlocal function invalid(key)\n\
local item = {x = 1}\nreturn item[key]\nend";
    let report = check(broad);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
    assert_eq!(
        &broad[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "key"
    );
    assert_eq!(report.type_at(broad.rfind("key]").unwrap()), Some("string"));

    let supported = "local item={[1]=42,[\"1\"]=\"text\",[\"\\255\"]=true}\n\
---@type number\nlocal numeric=item[1.0]\n\
---@type string\nlocal string=item[\"1\"]\n\
---@type boolean\nlocal bytes=item[\"\\xFF\"]\n\
---@param values table<string,number>\n---@param key string\n\
---@return number\nlocal function dynamic(values,key) return values[key] end\n\
local result=dynamic({x=7},\"x\")";
    let report = check(supported);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(ty(&report, "numeric"), "number");
    assert_eq!(ty(&report, "string"), "string");
    assert_eq!(ty(&report, "bytes"), "boolean");
    assert_eq!(ty(&report, "result"), "number");
    let invalid = supported.replace("local numeric=item[1.0]", "local numeric=item[\"1\"]");
    let report = check(&invalid);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &invalid[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "item[\"1\"]"
    );
}

#[test]
fn colon_call_inserts_receiver_and_preserves_each_argument_origin() {
    let declarations = "---@param self {x: number}\n---@param delta number\nlocal function add(self, delta) return self.x + delta end\nlocal item = {x = 1, add = add}\n";
    let source = format!("{declarations}local result = item:add(2)");
    let report = check(&source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    assert_eq!(ty(&report, "result"), "number");

    let invalid = format!("{declarations}local invalid = item:add(\"bad\")");
    let report = check(&invalid);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
    assert_eq!(
        &invalid[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "\"bad\""
    );

    let wrong_receiver = "---@param self number\n---@param delta number\nlocal function add(self, delta) return self + delta end\nlocal item = {add = add}\nlocal invalid = item:add(2)";
    let report = check(wrong_receiver);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2345");
    assert_eq!(
        &wrong_receiver[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "item"
    );
}
