//! Overloads declared without an implementation: a field written again as another function type
//! (`---@field f fun(...)` twice), and a function defined again in a declaration file. Both are
//! tried in the order they are written, as TypeScript tries an interface's method overloads and a
//! `.d.ts` file's function declarations, and as lua-language-server and EmmyLua Analyzer read
//! repeated function fields and a meta file's repeated definitions.
use luanalysis::check;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

fn errors(source: &str) -> Vec<String> {
    check(source).diagnostics.iter().map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message)).collect()
}

#[test]
fn repeated_function_fields_are_overloads_tried_in_written_order() {
    // Both of `f`'s overloads take a string: the first written is chosen.
    let source = "---@class Foo\n---@field f fun(a: string): \"first\"\n---@field f fun(a: string | number): \"second\"\n\
---@field m fun(self: Foo, a: string): string\n---@field m fun(self: Foo, a: number): number\nlocal Foo = {}\n\n\
---@type \"first\"\nlocal first = Foo.f(\"x\")\n---@type \"second\"\nlocal second = Foo.f(1)\n\
---@type string\nlocal text = Foo:m(\"x\")\n---@type number\nlocal count = Foo:m(1)\n";
    assert_eq!(errors(source), Vec::<String>::new());
    let swapped = source.replace("---@type \"first\"", "---@type \"second\"");
    assert_eq!(errors(&swapped), ["TS2322: Type '\"first\"' is not assignable to type '\"second\"'."]);
    let rejected = errors("---@class Foo\n---@field f fun(a: string): string\n---@field f fun(a: number): number\nlocal Foo = {}\nFoo.f(true)\n");
    assert_eq!(rejected.len(), 1, "{rejected:#?}");
    assert!(rejected[0].starts_with("TS2769: No overload matches this call."), "{rejected:#?}");
}

#[test]
fn an_intersection_of_function_types_tries_its_parts_in_written_order() {
    // Each written function type is its own declaration, as TypeScript's function type literal is,
    // so the parts of an intersection are not one declaration's sites tried last first.
    let source = "---@alias F (fun(a: string): \"first\") & (fun(a: string | number): \"second\")\n\
---@param f F\n---@param t { f: F }\nlocal function probe(f, t)\n    ---@type \"first\"\n    local direct = f(\"x\")\n\
    ---@type \"first\"\n    local member = t.f(\"x\")\n    ---@type \"second\"\n    local other = f(1)\nend\n";
    assert_eq!(errors(source), Vec::<String>::new());
}

#[test]
fn a_field_declared_again_with_another_type_is_reported_there_and_the_first_is_kept() {
    let source = "---@class Same\n---@field h string\n---@field h number\n---@field g string\n---@field g fun(): number\n---@field kept boolean\n\
---@field again string\n---@field again string\n---@field optional string\n---@field optional? string\nlocal Same = {}\n\nlocal h = Same.h\nlocal kept = Same.kept\n";
    let report = check(source);
    let messages: Vec<_> = report.diagnostics.iter().map(|diagnostic| (diagnostic.code, diagnostic.span.start, diagnostic.message.as_str())).collect();
    // As TypeScript reports it, at the later declaration's name; an identical one is the same field.
    assert_eq!(messages, [
        ("TS2717", source.find("h number").unwrap(), "Subsequent property declarations must have the same type. Property 'h' must be of type 'string', but here has type 'number'."),
        ("TS2717", source.find("g fun").unwrap(), "Subsequent property declarations must have the same type. Property 'g' must be of type 'string', but here has type 'fun(): number'."),
        ("TS2687", source.find("optional? string").unwrap(), "All declarations of 'optional' must have identical modifiers."),
        ("TS2717", source.find("optional? string").unwrap(), "Subsequent property declarations must have the same type. Property 'optional' must be of type 'string', but here has type 'nil | string'."),
    ]);
    // The class keeps its members, the first declaration's type among them.
    let at = |name: &str| report.type_at(source.find(&format!("local {name} =")).unwrap() + 6).map(str::to_owned);
    assert_eq!(at("h").as_deref(), Some("string"));
    assert_eq!(at("kept").as_deref(), Some("boolean"));
}

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Project(PathBuf);

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("luanalysis declared overloads {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&root).unwrap();
        let project = Self(fs::canonicalize(root).unwrap());
        for (path, text) in files {
            fs::write(project.0.join(path), text).unwrap();
        }
        project
    }

    fn check(&self) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli")).current_dir(&self.0).args(["check", "--lua-version", "5.4", "."]).output().unwrap();
        (String::from_utf8(output.stdout).unwrap() + &String::from_utf8(output.stderr).unwrap())
            .replace(&format!("{}{}", self.0.display(), std::path::MAIN_SEPARATOR), "")
            // Paths as written, whichever separator the platform prints.
            .replace(std::path::MAIN_SEPARATOR, "/")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_declaration_files_repeated_definitions_are_overloads() {
    let api = "---@class Api\nApi = {}\n\n\
---@param text string the text\n---@return string\nfunction Api.convert(text) end\n\n\
---@param count number the count\n---@return number\nfunction Api.convert(count) end\n\n\
---@param text string\n---@return \"first\"\nfunction choose(text) end\n";
    let more = "---@param text string | number\n---@return \"second\"\nfunction choose(text) end\n";
    let main = "---@type string\nlocal text = Api.convert(\"x\")\n---@type number\nlocal count = Api.convert(1)\n\
---@type \"first\"\nlocal first = choose(\"x\")\n---@type \"second\"\nlocal second = choose(1)\nApi.convert(true)\n";
    let report = Project::new(&[("api.def.lua", api), ("more.def.lua", more), ("main.lua", main)]).check();
    // A global's definitions in several declaration files are its overloads too, in the
    // project's order; neither definition is reported as the other's redeclaration.
    assert!(!report.contains(".def.lua"), "{report}");
    assert!(report.contains("main.lua:9:13: error[TS2769]: No overload matches this call."), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_declared_head_with_overloads_stays_their_implementation_signature() {
    // As in an implementation, and as tts-types writes them, the head beside `---@overload`s is
    // their implementation signature: it is checked against them and not called through.
    let api = "---@class Vector\nVector = {}\n\n\
---@overload fun(): Vector\n---@overload fun(x: number, y: number, z: number): Vector\n\
---@param x nil | number\n---@param y nil | number\n---@param z nil | number\n---@return Vector\nfunction Vector.new(x, y, z) end\n\n\
---@overload fun(a: number): number\n---@param a string\n---@return string\nfunction mismatched(a) end\n";
    let main = "local zero = Vector.new()\nlocal three = Vector.new(1, 2, 3)\nlocal one = Vector.new(1)\n";
    let report = Project::new(&[("api.def.lua", api), ("main.lua", main)]).check();
    assert!(report.contains("api.def.lua:12:4: error[TS2394]: Overload signature 'fun(a: number): number' is not compatible with implementation signature 'fun(a: string): string'."), "{report}");
    assert!(report.contains("main.lua:3:13: error[TS2554]: Expected 0-3 arguments, but got 1."), "{report}");
    assert_eq!(report.matches("error[").count(), 2, "{report}");
}

#[test]
fn function_fields_declared_again_in_another_declaration_are_overloads() {
    // As TypeScript merges an interface's method declarations, the overloads of every declaration
    // are the member's, tried in the order they are declared; a non-function field declared
    // again is still reported.
    let first = "---@class Api\n---@field pick fun(value: string): \"first\"\n---@field count number\n";
    let second = "---@class Api\n---@field pick fun(value: string | number): \"second\"\n---@field count string\n";
    let more = "---@field Api.pick fun(value: boolean): \"third\"\n";
    let main = "---@param api Api\nlocal function use(api)\n    ---@type \"first\"\n    local text = api.pick(\"x\")\n    ---@type \"second\"\n    local number = api.pick(1)\n    ---@type \"third\"\n    local flag = api.pick(true)\nend\n";
    let report = Project::new(&[("a.lua", first), ("b.lua", second), ("c.lua", more), ("main.lua", main)]).check();
    assert!(!report.contains("main.lua"), "{report}");
    assert!(report.contains("b.lua:3:11: error[TS2717]: Subsequent property declarations must have the same type. Property 'count' must be of type 'number', but here has type 'string'."), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn a_declaration_files_repeated_methods_of_a_local_class_are_overloads() {
    // As tts-types declares its classes, on a local table.
    let api = "---@class Box\nlocal Box = {}\n\n\
---@param value string\n---@return string\nfunction Box:pick(value) end\n\n\
---@param count number\n---@return number\nfunction Box:pick(count) end\n\n\
---@param value string\n---@return \"first\"\nfunction Box.choose(value) end\n\n\
---@param value string | number\n---@return \"second\"\nfunction Box.choose(value) end\n";
    let main = "---@param box Box\nlocal function use(box)\n    ---@type string\n    local text = box:pick(\"x\")\n    ---@type number\n    local count = box:pick(1)\n    ---@type \"first\"\n    local first = box.choose(\"x\")\n    box:pick(true)\nend\n";
    let report = Project::new(&[("api.def.lua", api), ("main.lua", main)]).check();
    assert!(report.contains("main.lua:9:14: error[TS2769]: No overload matches this call."), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}

#[test]
fn an_overload_with_a_literal_parameter_is_tried_first() {
    // As TypeScript's chooser tries signatures with literal parameter types before the others.
    let source = "---@class Chooser\n---@field choose fun(value: string): number\n---@field choose fun(value: \"literal\"): string\nlocal Chooser = {}\n\n\
---@type string\nlocal special = Chooser.choose('literal')\n---@type number\nlocal ordinary = Chooser.choose('other')\n";
    assert_eq!(errors(source), Vec::<String>::new());
}

#[test]
fn member_overloads_declared_in_several_files_map_through_inheritance() {
    let owner = "---@class Box<T>\n---@field choose fun(self: Box<T>, value: T): T\n\n---@class Derived<T>: Box<T>\n";
    let more = "---@field Box.choose fun(self: Box<number>, value: string): string\n";
    let main = "---@param box Derived<number>\nlocal function inspect(box)\n    ---@type number\n    local count = box:choose(1)\n    ---@type string\n    local text = box:choose('x')\nend\n";
    let report = Project::new(&[("owner.def.lua", owner), ("more.def.lua", more), ("main.lua", main)]).check();
    assert!(!report.contains("error["), "{report}");
}

#[test]
fn function_fields_of_a_generic_class_declared_again_are_overloads_of_its_type_parameters() {
    // Each declaration's `T` is the class's, so the overloads of both map to its arguments,
    // directly and through a derived class.
    let first = "---@class Box<T>\n---@field choose fun(self: Box<T>, value: T): T\n";
    let second = "---@class Box<T>\n---@field choose fun(self: Box<T>, value: T, other: T): T\n\n---@class Derived<T>: Box<T>\n";
    let main = "---@param box Box<number>\n---@param derived Derived<string>\nlocal function use(box, derived)\n\
    ---@type number\n    local one = box:choose(1)\n    ---@type number\n    local two = box:choose(1, 2)\n\
    ---@type string\n    local inherited = derived:choose('a', 'b')\n    box:choose(1, 'x')\nend\n";
    let report = Project::new(&[("first.def.lua", first), ("second.def.lua", second), ("main.lua", main)]).check();
    assert!(report.contains("main.lua:10:19: error[TS2345]: Argument of type '\"x\"' is not assignable to parameter of type 'number'."), "{report}");
    assert_eq!(report.matches("error[").count(), 1, "{report}");
}
