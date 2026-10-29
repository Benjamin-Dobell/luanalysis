//! Lua keyed-table/indexer acceptance gates. Pending gates assert the eventual
//! type behavior; an unsupported diagnostic never substitutes for that behavior.

use luanalysis::{CheckReport, check};

/// As TypeScript reports `noImplicitAny` element access, an inferred table read or written
/// at a computed key it lacks is an error only under `--no-implicit-any`.
fn check_strict(source: &str) -> luanalysis::CheckReport {
    luanalysis::check_with_options(source, luanalysis::CheckOptions { no_implicit_any: true, ..luanalysis::CheckOptions::default() })
}

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    report
}

fn type_error(source: &str, original: &str) {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322", "{error:#?}");
    assert_eq!(&source[error.span.start..error.span.end], original);
}

#[test]
fn number_and_string_keys_are_distinct_while_dot_and_string_keys_match() {
    // Reduction: legacy indexed_fields.lua literalTable/vector string keys.
    let prefix = "local item={[1]=11,[\"1\"]=\"text\",name=true}\n";
    accepted(&format!(
        "{prefix}\
---@type number\nlocal numeric=item[1]\n\
---@type string\nlocal string=item[\"1\"]\n\
---@type boolean\nlocal named=item[\"name\"]\n\
---@type boolean\nlocal dotted=item.name"
    ));
    type_error(
        &format!("{prefix}---@type string\nlocal rejected=item[1]"),
        "item[1]",
    );
}

#[test]
fn integral_float_and_hex_spellings_address_the_same_numeric_key() {
    accepted(
        "local item={[1]=42}\n\
---@type number\nlocal integer=item[1]\n\
---@type number\nlocal float=item[1.0]\n\
---@type number\nlocal hexadecimal=item[0x1]",
    );
}

#[test]
fn equivalent_static_lua_keys_share_native_write_facts() {
    for (write, read) in [("0x1", "1"), ("-0x1", "-1"), ("0x1p0", "1.0")] {
        accepted(&format!(
            "---@type {{[1]:number|string,[-1]:number|string}}\n\
local item={{[1]=1,[-1]=1}}\nitem[{write}]=\"written\"\n\
---@type string\nlocal observed=item[{read}]"
        ));
    }
}

#[test]
fn distinct_lua_integer_keys_do_not_collapse_at_the_javascript_precision_boundary() {
    accepted(
        "local item={[9007199254740992]=true,[9007199254740993]=\"text\"}\n\
---@type boolean\nlocal first=item[9007199254740992]\n\
---@type string\nlocal second=item[9007199254740993]",
    );
}

#[test]
fn non_utf8_lua_string_keys_remain_byte_sequences() {
    accepted(
        "local item={[\"\\255\"]=42,[\"normal\"]=\"value\"}\n\
---@type number\nlocal bytes=item[\"\\xFF\"]\n\
---@type string\nlocal named=item.normal",
    );
}

#[test]
fn constructor_key_calls_keep_their_original_expression_and_nested_function_identity() {
    accepted(
        "---@type table<string, number>\n\
local item={[(function() return \"slot\" end)()]=42}\n\
---@type number\nlocal value=item.slot",
    );
}

#[test]
fn dictionary_constraints_check_values_but_allow_nil_deletion() {
    // Reduction: legacy tables.lua:29-44. Nil writes delete entries; they do
    // not change the annotation's declared non-nil value type into a union.
    let prefix = "---@type table<string, number>\nlocal scores={}\n\
scores[\"one\"]=1\nscores.two=2\nscores[\"one\"]=nil\nscores.two=nil\n";
    accepted(prefix);
    type_error(&format!("{prefix}scores[\"bad\"]=\"wrong\""), "\"wrong\"");
}

#[test]
fn explicitly_nullable_named_fields_allow_nil_without_relaxing_required_fields() {
    for initializer in ["{value=1}", "{}", "{value=nil}"] {
        accepted(&format!(
            "---@type {{value: number|nil}}\nlocal item={initializer}\nitem.value=nil"
        ));
    }
    type_error(
        "---@type {value: number}\nlocal item={value=1}\nitem.value=nil",
        "nil",
    );
}

#[test]
fn contextual_literal_fields_keep_matching_singletons_and_reject_wrong_values() {
    accepted("---@type {[1]: true, name: \"tag\"}\nlocal item={[1]=true,name=\"tag\"}");
    type_error(
        "---@type {[1]: true}\nlocal item={[1]=false}",
        "{[1]=false}",
    );
}

#[test]
fn union_keys_read_each_allowed_literal_slot() {
    accepted(
        "---@param item {[1]: boolean, [2]: number}\n---@param key 1|2\n\
---@return boolean|number\nlocal function read(item,key) return item[key] end",
    );
    let report = check(
        "---@param item {[1]: boolean, [2]: number}\n---@param key 1|4\n\
local function invalid(item,key) return item[key] end",
    );
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
}

#[test]
fn closed_record_keys_require_complete_domain_coverage() {
    for parameter in ["string", "'x'|'missing'"] {
        let source = format!(
            "---@param key {parameter}\nlocal function invalid(key)\n\
local item={{x=1,y=2}}\nreturn item[key]\nend"
        );
        assert!(check(&source).diagnostics.is_empty());
        let report = check_strict(&source);
        assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
        let error = &report.diagnostics[0];
        // A literal key no field has is a missing property; a broader key, an element access.
        assert_eq!(error.code, if parameter == "string" { "TS7053" } else { "TS2339" });
        assert_eq!(&source[error.span.start..error.span.end], "key");
    }
    accepted(
        "---@param key 'x'|'y'\n---@return number\n\
local function finite(key)\nlocal item={x=1,y=2}\nreturn item[key]\nend",
    );
    accepted(
        "---@param key string\n---@param item table<string,number>\n---@return number\n\
local function indexed(key,item) return item[key] end",
    );
}

#[test]
fn declared_indexer_reads_use_the_declared_value_type() {
    // TS defaults do not add undefined to every index-signature read. The
    // nullable declaration below explicitly models a Lua sparse lookup.
    accepted(
        "---@param scores table<string, number>\n---@param key string\n\
local function total(scores,key)\n---@type number\nlocal value=scores[key]\nreturn value\nend\n\
---@param scores table<string, nil|number>\n---@param key string\n\
local function sparse(scores,key)\nlocal value=scores[key]\n\
if value~=nil then\n---@type number\nlocal present=value\nend\nreturn value\nend",
    );
}

#[test]
fn boolean_and_table_keys_are_valid_lua_dictionary_keys() {
    // ge_tts uses tables keyed by runtime table/object identities. This gate
    // checks accepted key types without importing JS string coercion.
    accepted(
        "---@type table<boolean, string>\nlocal flags={}\n\
flags[true]=\"yes\"\nflags[false]=\"no\"\n\
---@type table<{id: number}, string>\nlocal objects={}\n\
local first={id=1}\nlocal second={id=1}\n\
objects[first]=\"first\"\nobjects[second]=\"second\"",
    );
}

#[test]
fn nil_is_rejected_as_a_constructor_key() {
    let report = check("local item={[nil]=42}");
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2538");
    assert!(report.diagnostics[0].message.contains("nil"));
}

#[test]
fn sequence_constructor_and_numeric_dictionary_are_structurally_compatible() {
    // Reduction: legacy tables.lua:46-75. Array contiguity diagnostics are
    // a separate compatibility decision, not silently asserted here.
    accepted(
        "---@type number[]\nlocal sequence={1,2,3}\n\
---@type table<number, number>\nlocal dictionary=sequence\n\
---@type table<number, number>\nlocal sparse={[1]=1,[3]=3}\n\
---@type number\nlocal element=sequence[2]",
    );
}

#[test]
fn lua_array_length_and_indexing_do_not_expose_a_javascript_length_property() {
    accepted(
        "---@param values number[]\n---@return number\n\
local function length(values) return #values end\n\
---@param values number[]\n---@return number\n\
local function first(values) return values[1] end",
    );
    let source =
        "---@param values number[]\nlocal function invalid(values) return values.length end";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
    let error = &report.diagnostics[0];
    assert_eq!(&source[error.span.start..error.span.end], "length");
}

#[test]
fn only_a_final_sequence_field_expands_a_call_pack() {
    let source = "---@return number, string\nlocal function pair() return 11,\"tail\" end\n\
local final={7,pair(),}\nlocal nonfinal={pair(),7}\n\
local followed_by_key={pair(),label=\"label\"}\n\
local keyed_value={value=pair()}\n\
---@type number\nlocal first=final[1]\n\
---@type number\nlocal second=final[2]\n\
---@type string\nlocal third=final[3]\n\
---@type number\nlocal nonfinal_second=nonfinal[2]\n\
---@type number\nlocal before_key=followed_by_key[1]\n\
---@type number\nlocal named=keyed_value.value";
    accepted(source);
}

#[test]
fn sequence_open_tail_starts_after_the_fixed_constructor_prefix() {
    let source = "---@vararg number\n---@return number...\n\
local function numbers(...) return ... end\n\
---@vararg number\nlocal function construct(...) return {\"head\",numbers(...)} end\n\
local item=construct(1,2)\n\
---@type string\nlocal head=item[1]\n\
---@type nil|number\nlocal tail=item[2]\n\
---@type nil|number\nlocal later=item[99]";
    accepted(source);
    type_error(
        &format!("{source}\n---@type number\nlocal rejected=item[1]"),
        "item[1]",
    );
}

#[test]
fn sequence_constructor_retains_whole_alternative_rows() {
    // Independent scalar unions would invent crossed pairs and fail this
    // union-of-complete-table-shapes relation.
    accepted(
        "---@param ready boolean\n---@return (number,string)|(string,number)\n\
local function choose(ready)\nif ready then return 1,\"a\" end\nreturn \"b\",2\nend\n\
---@param ready boolean\nlocal function construct(ready)\n\
---@type {number,string}|{string,number}\nlocal item={choose(ready)}\nreturn item\nend",
    );
}

#[test]
fn sequence_constructor_supplies_the_expected_whole_tail_to_a_generic_producer() {
    let source = "---@generic T\n---@vararg T\n---@return T...\nlocal function produce(...) return ... end\n\
---@type number[]\nlocal item={produce()}";
    let report = accepted(source);
    let actual = report.type_at(source.find("{produce()}").unwrap()).unwrap();
    assert!(
        actual.contains("number") && !actual.contains("any") && !actual.contains("unknown"),
        "{actual}"
    );
    let wrong = source.replace("{produce()}", "{produce(\"bad\")}");
    let rejected = check(&wrong);
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2322" | "TS2345")),
        "the expected tail must not erase a real string argument: {:#?}",
        rejected.diagnostics
    );
    accepted(&source.replace("{produce()}", "{produce(1)}"));
}

#[test]
fn final_sequence_no_result_and_explicit_nil_keep_distinct_constructor_effects() {
    // A zero-result call makes no constructor store. A one-result nil call
    // contributes a deletion candidate when its sequence index collides.
    accepted(
        "local function zero() end\nlocal untouched={[1]=42,zero()}\n\
---@type number\nlocal preserved=untouched[1]",
    );
    let source = "local function nothing() return nil end\n\
local candidate={[1]=42,nothing()}\n\
---@type nil|number\nlocal possible=candidate[1]";
    accepted(source);
    type_error(
        &format!("{source}\n---@type number\nlocal rejected=candidate[1]"),
        "candidate[1]",
    );
}

#[test]
fn stored_constructor_collision_keeps_fixed_and_open_tail_candidates() {
    let source = "---@vararg string\n---@return string...\n\
local function strings(...) return ... end\n\
local item={[2]=true,strings()}\n\
---@type boolean|string\nlocal candidates=item[2]";
    accepted(source);
    type_error(
        &format!("{source}\n---@type string\nlocal rejected=item[2]"),
        "item[2]",
    );
}

#[test]
fn open_sequence_entries_must_satisfy_the_declared_numeric_indexer() {
    let source = "---@vararg string\n---@return string...\n\
local function strings(...) return ... end\n\
---@type number[]\nlocal rejected={strings()}";
    type_error(source, "{strings()}");
    accepted(&source.replace("---@type number[]", "---@type string[]"));
}

#[test]
fn simultaneous_assignment_retains_the_original_numeric_field_receiver() {
    // Same receiver-replacement rule already covered for named fields, now
    // with original numeric keys. No stronger alias analysis is assumed.
    for stores in [
        "item[1],item=\"old\",{[1]=2}",
        "item,item[1]={[1]=2},\"old\"",
    ] {
        let source = format!(
            "---@type {{[1]: number|string}}\n\
local item={{[1]=1}}\nlocal previous=item\n{stores}\n\
---@type number|string\nlocal current=item[1]\n\
---@type number|string\nlocal old=previous[1]"
        );
        accepted(&source);
        // TS keeps the declared union for the replacement's mutable member.
        // The old receiver's string write must not become a string fact about
        // the replacement reached through the former source path.
        type_error(
            &format!("{source}\n---@type string\nlocal rejected=item[1]"),
            "item[1]",
        );
    }
}

#[test]
fn tableutils_iterate_pattern_preserves_dictionary_key_and_value_types() {
    // TableUtils.ttslua:24-33 reduction: remove builtins/overloads/packed-array
    // dispatch, retain generic iterator,state,control return and actual indexing.
    let source = "---@generic K,V\n---@param iterator fun(state: table<K,V>, control: K): nil|(K,V)\n\
---@param state table<K,V>\n---@param control K\n\
---@return (fun(state: table<K,V>, control: K): nil|(K,V)), table<K,V>, K\n\
local function iterate(iterator,state,control) return iterator,state,control end\n\
---@param state table<number,string>\n---@param control number\n\
---@return nil|(number,string)\n\
local function iterator(state,control)\nif control<1 then return 1,state[1] end\nend\n\
---@type table<number,string>\nlocal values={[1]=\"value\"}\n\
for key,value in iterate(iterator,values,0) do\n\
---@type number\nlocal accepted_key=key\n\
---@type string\nlocal accepted_value=value\nend";
    accepted(source);
}

#[test]
fn duplicate_constructor_keys_do_not_invent_source_order_last_write_precision() {
    for constructor in ["{[1]=42,\"text\"}", "{\"text\",[1]=42}"] {
        let source = format!(
            "local item={constructor}\n\
---@type number|string\nlocal accepted=item[1]\n\
---@type number\nlocal rejected=item[1]"
        );
        // A conservative native collision union preserves the portable Lua
        // contract, which gives no last-wins precision for either source order.
        // The repeated key itself is TypeScript's TS1117.
        let report = check(&source);
        let codes: Vec<_> = report.diagnostics.iter().map(|error| error.code).collect();
        assert_eq!(codes, ["TS1117", "TS2322"], "{:#?}", report.diagnostics);
        let error = &report.diagnostics[1];
        assert_eq!(&source[error.span.start..error.span.end], "item[1]");
    }
}

#[test]
fn cli_numeric_string_key_error_disappears_after_original_read_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis keyed CLI {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("keys.lua");
    let source = "local item={[1]=42,[\"1\"]=\"text\",[\"\\255\"]=true}\n\
---@type number\nlocal rejected=item[\"1\"]\n\
---@type boolean\nlocal bytes=item[\"\\xFF\"]";
    fs::write(&path, source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .arg("check")
            .arg("--dump-types")
            .arg(&path)
            .output()
            .unwrap()
    };
    let bad = run();
    assert_eq!(bad.status.code(), Some(1));
    let error = String::from_utf8_lossy(&bad.stderr);
    assert!(error.contains("TS2322"), "{error}");
    assert!(
        !error.contains("LUA2025") && !error.contains("LUA2019"),
        "unsupported tables do not pass: {error}"
    );
    fs::write(
        &path,
        source.replace("local rejected=item[\"1\"]", "local rejected=item[1]"),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    let types = String::from_utf8_lossy(&corrected.stdout);
    assert!(
        types.contains("rejected: number") && types.contains("bytes: boolean"),
        "{types}"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cli_sequence_expansion_error_keeps_original_source_and_correction() {
    use std::{fs, process::Command};
    let directory =
        std::env::temp_dir().join(format!("luanalysis sequence CLI {}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("sequence.lua");
    let source = "---@return number,string\nlocal function pair() return 11,\"tail\" end\n\
local item={7,pair()}\n---@type number\nlocal rejected=item[3]";
    fs::write(&path, source).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .arg("check")
            .arg("--dump-types")
            .arg(&path)
            .output()
            .unwrap()
    };
    let bad = run();
    assert_eq!(bad.status.code(), Some(1));
    let error = String::from_utf8_lossy(&bad.stderr);
    assert!(
        error.contains("sequence.lua:5:16") && error.contains("TS2322"),
        "{error}"
    );
    assert!(!error.contains("LUA2016"), "{error}");
    fs::write(
        &path,
        source.replace("---@type number\n", "---@type string\n"),
    )
    .unwrap();
    let corrected = run();
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(String::from_utf8_lossy(&corrected.stdout).contains("rejected: string"));
    fs::remove_dir_all(directory).unwrap();
}
