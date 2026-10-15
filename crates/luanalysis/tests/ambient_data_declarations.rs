use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(declarations: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis ambient data {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.file("library.def.lua", declarations);
        fixture.file("main.lua", source);
        fixture
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
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
fn fail(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(text(output).contains(code), "{}", text(output));
}

// Same original declaration forms as package.config/loaded and utf8.charpattern,
// using unrelated user names so no standard-library spelling grants a capability.
const DATA: &str = "---@class UserSettings\nsettings = {}\nsettings.path = ''\nsettings.loaded = {}\nsettings.enabled = true\nsettings.limit = -3\n---@type 'fixed'\nsettings.mode = nil\nversion = '1.0'\n";

#[test]
fn original_dotted_literal_data_contracts_are_mutable_and_preserve_runtime_domains() {
    let fixture = Fixture::new(
        DATA,
        "settings.path = 'elsewhere'\nsettings.enabled = false\nsettings.limit = 7\nsettings.loaded['module'] = 42\nversion = '2.0'\n---@type string\nlocal path = settings.path\n---@type boolean\nlocal enabled = settings.enabled\n---@type number\nlocal limit = settings.limit\n---@type table\nlocal loaded = settings.loaded\n---@type string\nlocal current = version\n---@type 'fixed'\nlocal mode = settings.mode\n",
    );
    pass(&fixture.check());
}

#[test]
fn original_dotted_literal_data_errors_use_implementation_rhs_and_accept_correction() {
    let fixture = Fixture::new(DATA, "settings.path = 42\n");
    let invalid = fixture.check();
    fail(&invalid, "main.lua:1:17: error[TS2322]");
    assert!(
        !text(&invalid).contains("library.def.lua:"),
        "{}",
        text(&invalid)
    );
    fixture.file("main.lua", "settings.path = 'valid'\n");
    pass(&fixture.check());
    fixture.file("main.lua", "settings.mode = 'other'\n");
    fail(&fixture.check(), "TS2322");
    fixture.file("main.lua", "settings.mode = 'fixed'\n");
    pass(&fixture.check());
}

#[test]
fn root_literal_data_contracts_check_writes_without_narrowing_to_the_placeholder() {
    let fixture = Fixture::new("build = 'initial'\n", "build = 'next'\n");
    pass(&fixture.check());
    fixture.file("main.lua", "build = 42\n");
    fail(&fixture.check(), "main.lua:1:9: error[TS2322]");
    fixture.file("main.lua", "build = 'corrected'\n");
    pass(&fixture.check());
}

#[test]
fn executable_data_initializers_require_written_contracts_and_are_never_inferred() {
    let fixture = Fixture::new(
        "---@class UserSettings\nsettings = {}\nsettings.path = missing_initializer()\n",
        "local valid = 1\n",
    );
    let invalid = fixture.check();
    fail(&invalid, "library.def.lua:3:10: error[LUA2035]");
    assert!(!text(&invalid).contains("TS2304"), "{}", text(&invalid));
    fixture.file("library.def.lua", "---@class UserSettings\nsettings = {}\n---@type string\nsettings.path = missing_initializer()\n");
    pass(&fixture.check());
}

#[test]
fn namespace_data_requires_an_original_owner_and_does_not_publish_a_global_member_name() {
    let fixture = Fixture::new(
        "---@class UserSettings\nsettings = {}\nsettings.path = ''\n",
        "local value = path\n",
    );
    fail(&fixture.check(), "main.lua:1:15: error[TS2304]");
    fixture.file("library.def.lua", "settings.path = ''\n");
    fixture.file("main.lua", "local valid = 1\n");
    fail(&fixture.check(), "LUA2038");
}

#[test]
fn plain_empty_table_namespaces_use_original_function_and_data_contracts() {
    let declarations = "Api = {}\n---@param value string\n---@return number,string\nfunction Api.lookup(value) return undefined_placeholder(value) end\nApi.label = ''\n";
    let fixture = Fixture::new(declarations,
        "local count,label=Api.lookup('value')\n---@type number\nlocal numeric=count\n---@type string\nlocal textual=label\nApi.label='changed'\n");
    pass(&fixture.check());
    fixture.file("main.lua", "local value=Api.lookup(42)\n");
    fail(&fixture.check(), "main.lua:1:24: error[TS2345]");
    fixture.file("main.lua", "Api.label=42\n");
    fail(&fixture.check(), "main.lua:1:11: error[TS2322]");
    fixture.file("main.lua", "local count=Api.lookup('corrected')\nApi.label='corrected'\n");
    pass(&fixture.check());
}

#[test]
fn plain_namespace_generic_heads_keep_result_packs_and_literal_data_contracts() {
    let fixture = Fixture::new(
        "Api={}\n---@generic T\n---@param value T\n---@return T,T\nfunction Api.copy(value) return undefined_placeholder(value) end\n---@type 'fixed'\nApi.mode=nil\n",
        "local one,two=Api.copy('text')\n---@type string\nlocal checked=two\nApi.mode='fixed'\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "Api.mode='wrong'\n");
    fail(&fixture.check(), "TS2322");
    fixture.file("main.lua", "local missing=Api.unknown\n");
    fail(&fixture.check(), "TS2339");
}

#[test]
fn plain_namespace_contract_errors_keep_the_original_member_without_inferring_placeholders() {
    let fixture = Fixture::new("Api={}\nApi.value=undefined_placeholder()\n", "local valid=42\n");
    let invalid = fixture.check();
    fail(&invalid, "library.def.lua:2:5: error[LUA2035]");
    assert!(!text(&invalid).contains("TS2304"), "{}", text(&invalid));
    fixture.file("library.def.lua", "Api={}\n---@type string\nApi.value=undefined_placeholder()\n");
    fixture.file("main.lua", "---@type string\nlocal value=Api.value\n");
    pass(&fixture.check());
}

#[test]
fn plain_namespace_duplicate_members_and_unsupported_roots_fail_explicitly() {
    for declarations in [
        "Api={}\nApi.value=''\nApi.value=42\n",
        "Api={}\nApi={}\nApi.value=''\n",
        "Api={existing=42}\nApi.value=''\n",
        "---@type table\nApi={}\nApi.value=''\n",
        "Api={}\nApi.nested={}\nfunction Api.nested.read() end\n",
        "Api={}\nfunction Api:read() end\n",
    ] {
        let fixture = Fixture::new(declarations, "local valid=42\n");
        fail(&fixture.check(), "LUA2038");
    }
}
