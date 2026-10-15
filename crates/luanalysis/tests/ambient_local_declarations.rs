use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(declaration: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis ambient locals {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("library.def.lua"), declaration).unwrap();
        fs::write(path.join("main.lua"), source).unwrap();
        Self(path)
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--no-stdlib",
                "--dump-types",
                "--library",
                "library.def.lua",
                "main.lua",
            ])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn pass(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
}
const FILE: &str = "---@class File\nlocal original = {}\n---@param count number\n---@return string\nfunction original:read(count) return missing_body(count) end\nfunction original:close() end\nreturn original\n";

#[test]
fn uninitialized_ambient_class_locals_own_inherited_dot_and_colon_contracts() {
    // tts-types/Object.def.lua uses `local Object`, `local Container`, etc.
    let declaration = "---@class Object\n---@field value number\nlocal Object\n---@param value number\nfunction Object.setValue(value) end\n---@class Container:Object\nlocal Container\n---@param label string\nfunction Container:setLabel(label) end\nreturn Container\n";
    let fixture = Fixture::new(
        declaration,
        "---@type Container\nlocal container\ncontainer.setValue(42)\ncontainer:setLabel('ok')\n---@type number\nlocal value=container.value\n",
    );
    pass(&fixture.check());
    fs::write(fixture.0.join("main.lua"), "---@type Container\nlocal container\ncontainer:setLabel(42)\n").unwrap();
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("main.lua:3:20: error[TS2345]"), "{}", text(&output));
    fs::write(fixture.0.join("main.lua"), "---@type Container\nlocal container\ncontainer:setLabel('fixed')\n").unwrap();
    pass(&fixture.check());
}

#[test]
fn original_ambient_local_owners_can_be_exported_as_global_aliases_before_a_lexical_shadow() {
    // tts-types/WebRequest.def.lua exports GlobalWebRequest, then shadows its
    // global spelling with a different local class for request instances.
    let declaration = "---@class RequestApi\nlocal original={}\napi=original\n---@class Request\nlocal api={}\n---@return string\nfunction api.getResponse() end\n---@param url string\n---@return Request\nfunction original.get(url) end\n";
    let fixture = Fixture::new(declaration, "local request=api.get('https://example.test')\n---@type string\nlocal response=request.getResponse()\n");
    pass(&fixture.check());
    fs::write(fixture.0.join("main.lua"), "local request=api.get(42)\n").unwrap();
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("main.lua:1:23: error[TS2345]"), "{}", text(&output));
    fs::write(fixture.0.join("main.lua"), "local request=api.get('fixed')\n").unwrap();
    pass(&fixture.check());
}

#[test]
fn local_ambient_class_members_are_pure_original_contracts_with_real_receivers() {
    let fixture = Fixture::new(
        FILE,
        "---@type File\nlocal file\n---@type string\nlocal read=file:read(2)\nfile:close()\n---@type string\nlocal dot=file.read(file,3)\n",
    );
    let output = fixture.check();
    pass(&output);
    assert!(text(&output).contains("read: string"), "{}", text(&output));
    assert!(!text(&output).contains("missing_body"), "{}", text(&output));
    assert!(!text(&output).contains("original:"), "{}", text(&output));
}

#[test]
fn ambient_local_argument_errors_keep_original_utf8_crlf_ranges_then_accept_correction() {
    let fixture = Fixture::new(
        FILE,
        "-- é😀\r\n---@type File\r\nlocal file\r\nlocal read=file:read('wrong')\r\n",
    );
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("main.lua:4:22: error[TS2345]"),
        "{}",
        text(&output)
    );
    fs::write(
        fixture.0.join("main.lua"),
        "---@type File\nlocal file\nlocal read=file:read(2)\n",
    )
    .unwrap();
    pass(&fixture.check());
}

#[test]
fn local_ambient_overloads_and_polymorphic_self_keep_original_signature_owners() {
    let declaration = "---@class File\nlocal original={}\n---@overload fun(count:number):number\n---@overload fun(count:string):string\n---@param count number|string\n---@return number|string\nfunction original:read(count) end\n---@return self\nfunction original:copy() end\nreturn original\n";
    let fixture = Fixture::new(
        declaration,
        "---@type File\nlocal file\n---@type number\nlocal numeric=file:read(2)\n---@type string\nlocal textual=file:read('all')\n---@type File\nlocal copy=file:copy()\n",
    );
    pass(&fixture.check());
}

#[test]
fn original_global_namespace_terminal_return_is_validated_without_runtime_inference() {
    let fixture = Fixture::new(
        "---@class Namespace\nnamespace={}\n---@return number\nfunction namespace.read() return undefined_body() end\nreturn namespace\n",
        "---@type number\nlocal result=namespace.read()\n",
    );
    pass(&fixture.check());
    fs::write(
        fixture.0.join("library.def.lua"),
        "return undefined_export\n",
    )
    .unwrap();
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("LUA2034"), "{}", text(&output));
}

#[test]
fn runtime_local_classes_keep_checked_initializers_and_ambient_placeholders_cannot_execute() {
    let fixture = Fixture::new(
        "---@class File\n---@field required string\nlocal original={}\nreturn original\n",
        "---@type File\nlocal file\n---@type string\nlocal value=file.required\n",
    );
    pass(&fixture.check());
    fs::write(
        fixture.0.join("library.def.lua"),
        "---@class File\nlocal original=side_effect()\nreturn original\n",
    )
    .unwrap();
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("LUA2031"), "{}", text(&output));
    assert!(text(&output).contains("LUA2034"), "{}", text(&output));
}

#[test]
fn active_io_library_exposes_local_file_methods() {
    // This fixture disables automatic stdlib loading. IO's file class derives
    // from the ordinary userdata declaration in the matching builtin resource.
    let declarations = format!(
        "{}\n\n{}",
        include_str!("../stdlib/Lua54/builtin.def.lua"),
        include_str!("../stdlib/Lua54/io.def.lua"),
    );
    let fixture = Fixture::new(
        &declarations,
        "local handle=io.open('example.txt')\nlocal lines=handle:lines()\nhandle:close()\n---@type fun():any\nlocal iterator=lines\n",
    );
    pass(&fixture.check());
}

#[test]
fn ambient_local_private_parameters_must_accept_missing_public_arguments() {
    let declaration = "---@class File\nlocal original={}\n---@overload fun():string\n---@param count number\n---@return string\nfunction original:read(count) return unresolved_placeholder(count) end\nreturn original\n";
    let fixture = Fixture::new(
        declaration,
        "---@type File\nlocal file\n---@type string\nlocal value=file:read()\n",
    );
    let invalid = fixture.check();
    assert_eq!(invalid.status.code(), Some(1), "{}", text(&invalid));
    assert!(text(&invalid).contains("TS2394"), "{}", text(&invalid));
    assert!(!text(&invalid).contains("TS2304"), "{}", text(&invalid));
    fs::write(
        fixture.0.join("library.def.lua"),
        declaration.replace("---@param count number", "---@param count? number"),
    )
    .unwrap();
    pass(&fixture.check());
}

#[test]
fn ambient_generic_local_owners_substitute_original_member_scopes_per_instance() {
    let declaration = "---@class Box<T>\nlocal original={}\n---@return T\nfunction original:get() end\n---@param value T\nfunction original:set(value) end\nreturn original\n";
    let fixture = Fixture::new(
        declaration,
        "---@type Box<number>\nlocal numbers\n---@type Box<string>\nlocal strings\n---@type number\nlocal numeric=numbers:get()\n---@type string\nlocal textual=strings:get()\nnumbers:set(42)\nstrings:set('yes')\n",
    );
    pass(&fixture.check());
    fs::write(
        fixture.0.join("main.lua"),
        "---@type Box<number>\nlocal numbers\nnumbers:set('wrong')\n",
    )
    .unwrap();
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("TS2345"), "{}", text(&output));
}
