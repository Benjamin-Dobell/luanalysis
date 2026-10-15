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
    let declaration = "---@class File\nlocal original={}\n---@overload fun(count:number):number\n---@param count string\n---@return string\nfunction original:read(count) end\n---@return self\nfunction original:copy() end\nreturn original\n";
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
fn unmodified_original_io_library_exposes_local_file_methods() {
    let original = include_str!("../../../vendor/luanalysis-stdlib/upstream/Lua54/io.def.lua");
    let fixture = Fixture::new(
        original,
        "local handle=io.open('example.txt')\nlocal lines=handle:lines()\nhandle:close()\n---@type fun():any\nlocal iterator=lines\n",
    );
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
