use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Library(PathBuf);
impl Library {
    fn check(source: &str) -> (i32, String) {
        let library = Self(std::env::temp_dir().join(format!(
            "luanalysis-method-variance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir_all(&library.0).unwrap();
        fs::write(library.0.join("library.def.lua"), source).unwrap();
        fs::write(library.0.join("main.lua"), "").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&library.0)
            .args([
                "check",
                "--no-stdlib",
                "--library",
                "library.def.lua",
                "main.lua",
            ])
            .output()
            .unwrap();
        assert!(matches!(output.status.code(), Some(0 | 1)), "{output:?}");
        (
            output.status.code().unwrap(),
            format!(
                "{}{}",
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            ),
        )
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn clean(source: &str) {
    let (status, output) = Library::check(source);
    assert_eq!(status, 0, "{source}\n{output}");
}
fn rejects(source: &str, code: &str) {
    let (status, output) = Library::check(source);
    assert_eq!(status, 1, "{source}\n{output}");
    assert!(output.contains(code), "{source}\n{output}");
}

// Source-faithful SDK reductions. The corresponding TypeScript method forms
// accept both narrowed parameters; function-valued property forms reject them.
const HAND: &str = r#"---@class BaseObject
local Object
---@param value string
---@return any
function Object.setValue(value) end

---@class Hand:BaseObject
local Hand
---@param value string
---@return boolean
---@overload fun(value:'White'|'Blue'):boolean
function Hand.setValue(value) end
"#;

const STACKABLE: &str = r#"---@class BaseObject
local Object
---@param value BaseObject
---@return self
function Object.putObject(value) end

---@class Stackable:BaseObject
local Stackable
---@param value Stackable
---@return self
function Stackable.putObject(value) end
"#;

#[test]
fn sdk_named_methods_preserve_upstream_parameter_bivariance() {
    clean(HAND);
    clean(STACKABLE);
}

#[test]
fn implemented_methods_keep_their_original_runtime_declaration_provenance() {
    let source = "---@class Base\nlocal Base={}\n---@param value string\n---@return any\nfunction Base.setValue(value) return value end\n\n---@class Hand:Base\nlocal Hand={}\n---@param value 'White'|'Blue'\n---@return boolean\nfunction Hand.setValue(value) return true end\n";
    let report = luanalysis::check(source);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn sdk_card_return_contract_still_rejects_a_different_subclass() {
    rejects(
        r#"---@class BaseObject
local Object
---@param value BaseObject
---@return self
function Object.putObject(value) end
---@class Deck:BaseObject

---@class Card:BaseObject
local Card
---@param value Card|Deck
---@return Deck
function Card.putObject(value) end
"#,
        "TS2416",
    );
}

#[test]
fn function_valued_fields_and_assignment_expressions_remain_contravariant() {
    let hand = "---@class BaseObject\n---@field setValue fun(value:string):any\n\n---@class Hand:BaseObject\n---@field setValue fun(value:'White'|'Blue'):boolean\n";
    rejects(hand, "TS2416");
    clean(&hand.replace("fun(value:'White'|'Blue')", "fun(value:string)"));
    let stackable = "---@class BaseObject\n---@field putObject fun(value:BaseObject):self\n\n---@class Stackable:BaseObject\n---@field putObject fun(value:Stackable):self\n";
    rejects(stackable, "TS2416");
    clean(&stackable.replace("fun(value:Stackable)", "fun(value:BaseObject)"));
    rejects(
        "---@class Base\n---@field callback fun(value:string|number):void\n\n---@class Derived:Base\n---@field callback fun(value:number):void\n",
        "TS2416",
    );
    let assignment = "---@class Base\nlocal Base={}\n---@param value string|number\nBase.callback=function(value) end\n\n---@class Derived:Base\nlocal Derived={}\n---@param value number\nDerived.callback=function(value) end\n";
    let report = luanalysis::check(assignment);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2416"),
        "{:#?}",
        report.diagnostics
    );
    let corrected = luanalysis::check(
        &assignment.replace("---@param value number", "---@param value string|number"),
    );
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}

#[test]
fn bodyless_dot_and_colon_methods_keep_origins_through_generic_inheritance() {
    clean(
        r#"---@class Base<T>
---@function dot(value:T):void
---@function :colon(value:T):void

---@class Middle<U>:Base<U>

---@class Derived:Middle<string|number>
---@function dot(value:number):void
---@function :colon(value:number):void
"#,
    );
}

#[test]
fn nested_function_results_do_not_inherit_the_outer_method_origin() {
    let source = "---@class Base\nlocal Base\n---@return fun(value:string|number):void\nfunction Base.callback() end\n\n---@class Derived:Base\nlocal Derived\n---@return fun(value:number):void\nfunction Derived.callback() end\n";
    rejects(source, "TS2416");
    clean(&source.replace(
        "---@return fun(value:number)",
        "---@return fun(value:string|number)",
    ));
}

#[test]
fn private_method_implementations_must_accept_every_public_overload() {
    for declaration in [
        "function Example.method(value) end",
        "function Example:method(value) end",
    ] {
        let source = format!(
            "---@class Example\nlocal Example\n---@param value number\n---@overload fun(value:string|number):void\n{declaration}\n"
        );
        rejects(&source, "TS2394");
        clean(&source.replace("---@param value number", "---@param value string|number"));
    }
    let source = "---@class Example\nlocal Example={}\n---@param value number\n---@overload fun(value:string|number):void\nfunction Example.method(value) end\n";
    let report = luanalysis::check(source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2394"),
        "{:#?}",
        report.diagnostics
    );
    let corrected = luanalysis::check(
        &source.replace("---@param value number", "---@param value string|number"),
    );
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
}
