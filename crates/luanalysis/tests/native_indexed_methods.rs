use luanalysis::{check, CheckReport};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}

fn rejects_argument(source: &str, argument: &str) {
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{source}\n{:#?}", report.diagnostics);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.code, "TS2345", "{:#?}", report.diagnostics);
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], argument);
}

const INDEXED: &str = r#"---@class Element

---@class Registry
---@field [string] Element
local Registry={}
---@param value number
---@return number
function Registry.Any.read(value) return value end
---@type Element
local instance
local result=instance.read(1)
"#;

#[test]
fn class_valued_indexers_publish_methods_on_their_original_value_class() {
    let report = clean(INDEXED);
    assert_eq!(report.bindings.iter().find(|binding| binding.name == "result").unwrap().ty, "number");
    rejects_argument(&INDEXED.replace("instance.read(1)", "instance.read('wrong')"), "'wrong'");
}

#[test]
fn indexed_colon_receivers_keep_self_and_written_argument_contracts() {
    let source = INDEXED
        .replace("---@class Element", "---@class Element\n---@field number number")
        .replace("function Registry.Any.read(value) return value end", "function Registry.Any:read(value) return self.number+value end")
        .replace("instance.read(1)", "instance:read(1)");
    clean(&source);
    rejects_argument(&source.replace("instance:read(1)", "instance:read('wrong')"), "'wrong'");
}

#[test]
fn written_named_class_fields_keep_distinct_method_owners() {
    let source = r#"---@class First

---@class Second

---@class Registry
---@field first First
---@field second Second
local Registry={}
---@param value number
function Registry.first.read(value) end
---@param value string
function Registry.second.read(value) end
---@type First
local first
---@type Second
local second
first.read(1)
second.read('text')
"#;
    clean(source);
    rejects_argument(&source.replace("first.read(1)", "first.read('wrong')"), "'wrong'");
    rejects_argument(&source.replace("second.read('text')", "second.read(2)"), "2");
}

#[test]
fn disjoint_written_indexer_domains_do_not_merge_class_method_signatures() {
    let source = r#"---@class StringElement

---@class NumberElement

---@class Registry
---@field [string] StringElement
---@field [number] NumberElement
local Registry={}
---@param value number
function Registry.Any.read(value) end
Registry[1].read=function(value) return value end
---@type StringElement
local textual
textual.read(1)
"#;
    clean(source);
    rejects_argument(&source.replace("textual.read(1)", "textual.read('wrong')"), "'wrong'");
}

#[test]
fn indexed_owner_projection_respects_lexically_shadowed_registry_bindings() {
    let source = format!("{INDEXED}do\nlocal Registry={{Any={{}}}}\nfunction Registry.Any.unrelated() end\nend\nlocal missing=instance.unrelated\n");
    let report = check(&source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
    assert_eq!(&source[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "unrelated");
}

#[test]
fn a_non_generic_class_valued_self_member_does_not_create_an_owner_cycle() {
    let source = INDEXED.replace("---@class Element\n\n---@class Registry\n---@field [string] Element", "---@class Element\n---@field child Element")
        .replace("Registry.Any.read", "Registry.child.read");
    clean(&source);
    rejects_argument(&source.replace("instance.read(1)", "instance.read('wrong')"), "'wrong'");
}

#[test]
fn applied_indexer_value_classes_keep_the_original_specialization() {
    let source = r#"---@class Element<T>
---@field value T

---@class Registry<T>
---@field [string] Element<T>

---@type Registry<number>
local registry
---@return number
function registry.Any:read() return self.value end
---@type Element<number>
local numeric
local result=numeric:read()
"#;
    clean(source);
    let wrong = format!("{source}---@type Element<string>\nlocal textual\nlocal missing=textual.read\n");
    let report = check(&wrong);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2339");
    assert_eq!(&wrong[report.diagnostics[0].span.start..report.diagnostics[0].span.end], "read");
}
