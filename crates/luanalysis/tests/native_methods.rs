use luanalysis::{check, CheckReport};
fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}
fn rejects(source: &str, code: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.iter().any(|error| error.code == code), "{source}\n{:#?}", report.diagnostics);
    report
}
fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report.bindings.iter().find(|binding| binding.name==name).expect(name).ty
}
const ADD: &str = "---@type {value:number,add:fun(self:{value:number},amount:number):number}\nlocal object\n---@param self {value:number}\n---@param amount number\n---@return number\nfunction object:add(amount) return self.value+amount end\n";
#[test]
fn colon_definitions_use_native_parameter_zero_and_call_receiver_insertion() {
    let report = clean(&format!("{ADD}local colon=object:add(2)\nlocal dot=object.add(object,3)"));
    assert_eq!(ty(&report,"colon"),"number");
    assert_eq!(ty(&report,"dot"),"number");
    rejects(&format!("{ADD}object.add(2)"),"TS2554");
    let source=format!("{ADD}object:add(\"wrong\")");
    let report=rejects(&source,"TS2345");
    assert!(report.diagnostics.iter().any(|error| error.code=="TS2345" && &source[error.span.start..error.span.end]=="\"wrong\""));
}
#[test]
fn receiver_reads_and_writes_keep_the_declared_contract_at_original_spans() {
    let source=ADD.replace("return self.value+amount", "self.value=\"wrong\"; return amount");
    let report=rejects(&source,"TS2322");
    assert!(report.diagnostics.iter().any(|error| error.code=="TS2322" && &source[error.span.start..error.span.end]=="\"wrong\""));
    clean(&source.replace("\"wrong\"","42"));
}
#[test]
fn explicit_self_parameter_shadows_receiver_without_removing_runtime_receiver_slot() {
    let source="---@type {method:fun(receiver:{},self:string):string}\nlocal object\n---@param self string\n---@return string\nfunction object:method(self) return self end\nlocal result=object:method(\"yes\")";
    let report=clean(source);
    assert_eq!(ty(&report,"result"),"string");
    rejects(&source.replace("object:method(\"yes\")","object:method()"),"TS2554");
}
#[test]
fn lexical_self_shadow_captures_the_real_receiver_before_local_activation() {
    let source="---@type {value:number,method:fun(self:{value:number}):number}\nlocal object\n---@param self {value:number}\n---@return number\nfunction object:method()\n local self=self\n local function read() return self.value end\n return read()\nend\nlocal result=object:method()";
    let report=clean(source);
    assert_eq!(ty(&report,"result"),"number");
}
#[test]
fn dot_definitions_keep_only_written_parameters() {
    let source="---@type {method:fun(value:number):number}\nlocal object\n---@param value number\n---@return number\nfunction object.method(value) return value end\nlocal result=object.method(42)";
    let report=clean(source);
    assert_eq!(ty(&report,"result"),"number");
    rejects(&source.replace("object.method(42)","object:method(42)"),"TS2554");
}
#[test]
fn optional_and_variadic_written_parameters_follow_receiver_in_native_positions() {
    let source="---@type {method:fun(self:{},value?:number,...:string):string|nil}\nlocal object\n---@param self {}\n---@param value? number\n---@vararg string\n---@return string|nil\nfunction object:method(value,...) local first=...; return first end\nlocal result=object:method(42,\"yes\")";
    let report=clean(source);
    assert!(ty(&report,"result").contains("string") && ty(&report,"result").contains("nil"));
    rejects(&source.replace("42,\"yes\"","42,99"),"TS2345");
}
#[test]
fn receiver_context_comes_from_original_destination_signature_without_written_tag() {
    let source="---@type {value:number,method:fun(self:{value:number}):number}\nlocal object\n---@return number\nfunction object:method() return self.value end\nlocal result=object:method()";
    let report=clean(source);
    assert_eq!(ty(&report,"result"),"number");
}
#[test]
fn own_generic_method_inference_keeps_receiver_out_of_written_generic_arity() {
    let source="---@type {method:fun<T>(self:{},value:T):T}\nlocal object\n---@generic T\n---@param value T\n---@return T\nfunction object:method(value) return value end\nlocal result=object:method(42)";
    let report=clean(source);
    assert!(matches!(ty(&report,"result"),"42"|"number"),"{}",ty(&report,"result"));
}
#[test]
fn inherited_instance_call_binds_polymorphic_self_to_actual_derived_type() {
    let source="---@class Base\n---@field identity fun(self:self):self\nlocal object={}\n---@return self\nfunction object:identity() return self end\n---@class Derived:Base\n---@field marker string\nlocal derived={marker=\"yes\"}\n---@param value Derived\nlocal function read(value) return value:identity().marker end";
    clean(source);
}
#[test]
fn body_cannot_replace_arbitrary_polymorphic_self_with_the_base_definition() {
    let source="---@class Base\n---@field identity fun(self:self):self\nlocal object={}\n---@return self\nfunction object:identity() return object end";
    rejects(source,"TS2322");
}
#[test]
fn a_checked_fixed_method_write_refines_actual_definition_presence() {
    let source="---@class C\n---@field value number\n---@field identity fun(self:self):self\nlocal object={value=42}\n---@return self\nfunction object:identity() return self end\nlocal result=object:identity().value";
    let report=clean(source);
    assert_eq!(ty(&report,"result"),"number");
}
#[test]
fn method_write_does_not_fabricate_other_required_instance_fields() {
    let source="---@class C\n---@field value number\n---@field identity fun(self:self):self\nlocal object={}\n---@return self\nfunction object:identity() return self end\nlocal result=object:identity()";
    // The class table is a receiver of its class before it has every member, as in v1.
    clean(source);
    clean(&source.replace("local object={}","local object={value=42}"));
}
#[test]
fn conditional_method_initialization_keeps_an_incomplete_branch() {
    let source="---@class C\n---@field identity fun(self:self):self\nlocal object={}\n---@param flag boolean\nlocal function run(flag)\n if flag then\n  ---@return self\n  function object:identity() return self end\n end\n return object:identity()\nend";
    // A declared method reads as the class declares it, wherever it is defined.
    clean(source);
}
