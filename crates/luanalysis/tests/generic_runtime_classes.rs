use luanalysis::{CheckReport, check};

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}

fn rejected_at(source: &str, code: &str, expression: &str) {
    let report = check(source);
    assert!(!report.diagnostics.iter().any(|error| error.code == "LUA2031"), "{source}\n{:#?}", report.diagnostics);
    assert!(report.diagnostics.iter().any(|error| {
        error.code == code && &source[error.span.start..error.span.end] == expression
    }), "{source}\n{:#?}", report.diagnostics);
}

const BOX: &str = "---@class Box<T>\n---@field value T\nlocal Box={}\n\
---@param value T\nfunction Box:set(value) self.value=value end\n\
---@return T\nfunction Box:get() return self.value end\n\
---@return self\nfunction Box:identity() return self end\n";

#[test]
fn initialized_generic_class_methods_specialize_for_each_accessed_instance() {
    let source = format!("{BOX}\
---@type Box<number>\nlocal numbers\n\
---@type Box<string>\nlocal strings\n\
numbers:set(42)\nstrings:set('text')\n\
---@type number\nlocal number=numbers:get()\n\
---@type string\nlocal text=strings:get()\n\
---@type Box<number>\nlocal same=numbers:identity()\n");
    accepted(&source);
    rejected_at(&source.replace("numbers:set(42)", "numbers:set('wrong')"), "TS2345", "'wrong'");
    rejected_at(&source.replace("strings:set('text')", "strings:set(42)"), "TS2345", "42");
    rejected_at(&source.replace("local number=numbers:get()", "local number=strings:get()"), "TS2322", "strings:get()");
    rejected_at(&source.replace("local same=numbers:identity()", "local same=strings:identity()"), "TS2322", "strings:identity()");
}

#[test]
fn generic_runtime_definition_parameters_stay_rigid_and_keep_nominal_ownership() {
    rejected_at(&format!("{BOX}Box.value=42\n"), "TS2322", "42");
    let supplied = "---@class Supplied<T:number>\n---@field value T\nlocal Supplied={value=42}\n";
    rejected_at(supplied, "TS2322", "{value=42}");
    let unrelated = format!("{BOX}\
---@class Other<T>\n---@field value T\nlocal Other={{}}\n\
---@type Other<number>\nlocal other\n\
---@type Box<number>\nlocal box=other\n");
    rejected_at(&unrelated, "TS2322", "other");
}

#[test]
fn initialized_generic_inheritance_maps_member_parameters_and_semantic_self() {
    let source = format!("{BOX}\
---@class Child<U>:Box<U>\nlocal Child={{}}\n\
---@type Child<number>\nlocal child\n\
child:set(42)\n\
---@type number\nlocal number=child:get()\n\
---@type Child<number>\nlocal same=child:identity()\n");
    accepted(&source);
    rejected_at(&source.replace("child:set(42)", "child:set('wrong')"), "TS2345", "'wrong'");
    rejected_at(&source.replace("---@type Child<number>\nlocal same", "---@type Child<string>\nlocal same"), "TS2322", "child:identity()");
}

#[test]
fn initialized_generic_constraints_validate_arguments_and_supplied_fields() {
    let declaration = "---@class Numeric<T:number>\n---@field count number\nlocal Numeric={count=42}\n\
---@class ChildNumeric<U:number>:Numeric<U>\nlocal ChildNumeric={}\n";
    accepted(&format!("{declaration}---@type ChildNumeric<number>\nlocal child\n"));
    rejected_at(&format!("{declaration}---@type ChildNumeric<string>\nlocal child\n"), "TS2344", "string");
    rejected_at(&declaration.replace("{count=42}", "{count='wrong'}"), "TS2322", "{count='wrong'}");
    let unbounded = "---@class Numeric<T:number>\nlocal Numeric={}\n---@class ChildNumeric<U>:Numeric<U>\nlocal ChildNumeric={}\n";
    rejected_at(unbounded, "TS2344", "U");
}

#[test]
fn generic_definition_values_map_captured_outer_parameters_for_sibling_calls() {
    let source = "---@generic T\n---@param value T\nlocal function create(value)\n\
---@class Captured<U>\n---@field captured T\n---@field item U\n\
local Captured={captured=value}\nreturn Captured\nend\n\
local numbers=create(42)\nlocal strings=create('text')\n\
---@type number\nlocal number=numbers.captured\n\
---@type string\nlocal text=strings.captured\n\
---@type number\nlocal again=create(7).captured\n";
    accepted(source);
    rejected_at(&source.replace("local number=numbers.captured", "local number=strings.captured"), "TS2322", "strings.captured");
    rejected_at(&source.replace("local text=strings.captured", "local text=numbers.captured"), "TS2322", "numbers.captured");
}

#[test]
fn generic_class_field_attachments_keep_their_class_parameter_scope() {
    let source = "local M={}\n---@class Nested<T>\n---@field value T\nM.Nested={}\n\
---@param value T\nfunction M.Nested:set(value) self.value=value end\n\
---@return T\nfunction M.Nested:get() return self.value end\n\
---@type Nested<number>\nlocal numbers\nnumbers:set(42)\n\
---@type number\nlocal value=numbers:get()\n";
    accepted(source);
    rejected_at(&source.replace("numbers:set(42)", "numbers:set('wrong')"), "TS2345", "'wrong'");
}
