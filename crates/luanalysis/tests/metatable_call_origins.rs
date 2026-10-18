use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis metatable call origins {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn check(&self, source: &str) -> Output {
        self.check_effects(source, false)
    }
    fn check_strict(&self, source: &str) -> Output {
        self.check_effects(source, true)
    }
    fn check_effects(&self, source: &str, strict: bool) -> Output {
        self.file("main.lua", source);
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"));
        command.current_dir(&self.0).args([
            "check",
            "--lua-version",
            "5.4",
            "--module-root",
            ".",
            "--dump-types",
        ]);
        if strict {
            command.arg("--strict-standard-effects");
        }
        command.arg("main.lua").output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn clean(output: &Output) {
    assert!(output.status.success(), "{}", text(output));
}
fn no_owner(output: &Output) {
    assert!(!output.status.success(), "{}", text(output));
    assert!(text(output).contains("Property 'read'"), "{}", text(output));
}

const FACTORY: &str = r#"---@class Instance

---@class StaticInstance
---@overload fun(): Instance
local Static = {}
setmetatable(Static, {
    ---@return Instance
    __call = function()
        local self = --[[---@type self]] {}
        ---@return number
        function self.read() return 1 end
        return self
    end,
})
---@type number
local result = Static().read()
"#;
const ORDINARY: &str = r#"---@generic T
---@param value T
---@param meta nil | table
---@return T
local function ordinary(value, meta) return value end
"#;

#[test]
fn provisional_constructor_metadata_does_not_keep_report_publication_alive() {
    let fixture = Fixture::new();
    let source = "---@class Static\n---@overload fun():Static\nlocal Static={}\nsetmetatable(Static,{\n ---@param class self\n ---@param unknown number\n ---@return self|nil\n __call=function(class)\n  ---@return number\n  local function child() return 'wrong' end\n  child()\n  return --[[---@type self]] {}\n end,\n})\n";
    let invoke = |source: &str, jobs: usize| {
        fixture.file("main.lua", source);
        let jobs = jobs.to_string();
        let mut child = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
            .current_dir(&fixture.0)
            .args([
                "check",
                "--jobs",
                &jobs,
                "--lua-version",
                "5.4",
                "--module-root",
                ".",
                "main.lua",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("report publication did not settle: {}", text(&output));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    };
    // The original source looped forever while repeatedly publishing the same
    // annotation error from a still-provisional constructor body.
    let corrected = source
        .replace(" ---@param unknown number\n", "")
        .replace("---@return self|nil", "---@return self")
        .replace("return 'wrong'", "return 1");
    for jobs in [1, 2, 4] {
        let invalid = invoke(source, jobs);
        assert_eq!(invalid.status.code(), Some(1), "jobs={jobs}: {}", text(&invalid));
        for code in ["LUA2011", "LUA2040", "LUA2041", "TS2322"] {
            assert_eq!(
                text(&invalid).matches(&format!("error[{code}]")).count(),
                1,
                "jobs={jobs}: {}",
                text(&invalid)
            );
        }
        clean(&invoke(&corrected, jobs));
    }
}

#[test]
fn exact_transitive_local_calls_preserve_standard_origin_but_real_writes_do_not() {
    let fixture = Fixture::new();
    let helpers =
        "local function leaf() return 1 end\nlocal function helper() leaf() end\nhelper()\n";
    clean(&fixture.check(&format!("{helpers}{FACTORY}")));
    clean(&fixture.check_strict(&format!("{helpers}{FACTORY}")));
    let writes = format!(
        "{ORDINARY}local function leaf() setmetatable=ordinary end\nlocal function helper() leaf() end\nhelper()\n"
    );
    // Default assumptions do not recursively inspect a helper's callee.
    // Strict mode still needs the physical standard-value preservation proof.
    clean(&fixture.check(&format!("{writes}{FACTORY}")));
    no_owner(&fixture.check_strict(&format!("{writes}{FACTORY}")));
    let corrected = format!(
        "local attach=setmetatable\n{writes}{}",
        FACTORY.replace("setmetatable(Static", "attach(Static")
    );
    clean(&fixture.check(&corrected));
    clean(&fixture.check_strict(&corrected));

    let replaced = format!("{ORDINARY}setmetatable=ordinary\n{FACTORY}");
    no_owner(&fixture.check(&replaced));
    no_owner(&fixture.check_strict(&replaced));
}

#[test]
fn a_newindex_metatable_does_not_affect_standard_identity() {
    let fixture = Fixture::new();
    let prefix = format!(
        "{ORDINARY}---@type {{value:boolean|number|nil}}\nlocal holder={{value=false}}\nsetmetatable(holder,{{__newindex=function() setmetatable=ordinary end}})\nholder.value=true\nholder.value=1\n"
    );
    clean(&fixture.check(&format!("{prefix}{FACTORY}")));
    clean(&fixture.check_strict(&format!("{prefix}{FACTORY}")));
    let missing = prefix.replace("value=false", "value=nil");
    // Metatables are not traced: a missing raw slot is an ordinary store
    // in both modes, whatever `__newindex` the table may have.
    clean(&fixture.check(&format!("{missing}{FACTORY}")));
    clean(&fixture.check_strict(&format!("{missing}{FACTORY}")));
}

#[test]
fn original_required_module_execution_preserves_only_unchanged_standard_values() {
    let fixture = Fixture::new();
    fixture.file(
        "leaf.lua",
        "local function leaf() return 1 end\nleaf()\nreturn {}\n",
    );
    fixture.file("helper.lua", "require('leaf')\nreturn {}\n");
    let source = format!("require('helper')\n{FACTORY}");
    clean(&fixture.check(&source));
    clean(&fixture.check_strict(&source));
    fixture.file(
        "leaf.lua",
        &format!("{ORDINARY}setmetatable=ordinary\nreturn {{}}\n"),
    );
    // The leaf is behind a second module execution, beyond the bounded
    // default observation of the directly imported helper's original scope.
    clean(&fixture.check(&source));
    no_owner(&fixture.check_strict(&source));
    let corrected = format!(
        "local attach=setmetatable\nrequire('helper')\n{}",
        FACTORY.replace("setmetatable(Static", "attach(Static")
    );
    clean(&fixture.check(&corrected));
    clean(&fixture.check_strict(&corrected));
}

#[test]
fn loader_mutations_inside_calls_preserve_imports_but_reached_writes_refuse_them() {
    let fixture = Fixture::new();
    fixture.file("helper.lua", "return {}\n");
    let source = format!(
        "local function replace_loader() package.searchers=package.searchers end\nreplace_loader()\nrequire('helper')\n{FACTORY}"
    );
    // A call preserves the loader, as an unknown effect does: only a loader
    // write the importing flow itself reaches refuses the import.
    clean(&fixture.check(&source));
    no_owner(&fixture.check_strict(&source));
    clean(&fixture.check(&format!("require('helper')\n{FACTORY}")));
    clean(&fixture.check_strict(&format!("require('helper')\n{FACTORY}")));
    let direct = format!("package.searchers=package.searchers\nrequire('helper')\n{FACTORY}");
    for strict in [false, true] {
        let output = fixture.check_effects(&direct, strict);
        assert_eq!(output.status.code(), Some(1), "{}", text(&output));
        assert!(text(&output).contains("LUA2036"), "{}", text(&output));
    }
}

#[test]
fn a_blocked_import_does_not_apply_original_module_writes_to_saved_standard_values() {
    let fixture = Fixture::new();
    fixture.file(
        "helper.lua",
        &format!("{ORDINARY}setmetatable=ordinary\nreturn {{}}\n"),
    );
    for loader in ["require", "load"] {
        let prefix = if loader == "load" {
            "local load=require\n"
        } else {
            ""
        };
        let source = format!("{prefix}package.searchers=package.searchers\n{loader}('helper')\n{FACTORY}");
        let refused = fixture.check(&source);
        assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
        assert!(text(&refused).contains("LUA2036"), "{}", text(&refused));
        // A refused source import cannot pretend its original file executed
        // and replaced setmetatable while checking the following constructor.
        assert!(
            !text(&refused).contains("Property 'read'"),
            "{}",
            text(&refused)
        );
        let wrong = source.replace("local result = Static().read()", "local result = 'wrong'");
        let rejected = fixture.check(&wrong);
        assert!(text(&rejected).contains("TS2322"), "{}", text(&rejected));

        // Restoring the loader certifies the import. The imported module's
        // own global writes do not reach the importer: the import is a call,
        // which preserves the original setmetatable.
        let reached = source.replace("package.searchers=package.searchers\n", "");
        clean(&fixture.check(&reached));

        // A builtin value captured before that reached replacement still
        // supplies the original operation independently of its global slot.
        let saved = format!(
            "local attach=setmetatable\n{}",
            reached.replace("setmetatable(Static", "attach(Static")
        );
        clean(&fixture.check(&saved));
        clean(&fixture.check_strict(&saved));
    }
}

#[test]
fn written_member_contracts_do_not_require_a_constructor_origin_certificate() {
    let fixture = Fixture::new();
    let source = r#"---@function opaque()

---@class Instance
---@field value number
---@function read(): number

---@class StaticInstance
---@overload fun(): Instance
local Static = {}
opaque()
setmetatable(Static, {
    ---@return Instance
    __call = function()
        local instance = --[[---@type Instance]] {value=1, read=function() return 1 end}
        return instance
    end,
})
local instance = Static()
---@type number
local field = instance.value
---@type number
local result = instance.read()
"#;
    clean(&fixture.check(source));
    let wrong = source.replace("local result = instance.read()", "local result = 'wrong'");
    let output = fixture.check(&wrong);
    assert!(text(&output).contains("TS2322"), "{}", text(&output));
    clean(&fixture.check(source));
}


#[test]
fn namespace_newindex_metatables_do_not_affect_require() {
    let fixture = Fixture::new();
    fixture.file("Helper.lua", "return {value=1}\n");
    let source = "local Class=require('Class')\nlocal Helper=require('Helper')\n---@type number\nlocal result=Class.identity(Helper.value)\n";
    let namespace = "---@class UserClass\nlocal Class={}\n\n---@param value number\n---@return number\nfunction Class.identity(value) return value end\n\nreturn Class\n";
    let changing_loader = namespace.replace(
        "local Class={}\n",
        "local Class={}\n\nsetmetatable(Class, {\n---@param target table\n---@param key string\n---@param value function\n__newindex=function(target,key,value) package.searchers=package.searchers end,\n})\n",
    );
    fixture.file("Class.lua", &changing_loader);
    clean(&fixture.check(source));
    // Metatables are not traced, so a `__newindex` handler is not a loader
    // effect even under strict runtime effects.
    clean(&fixture.check_strict(source));
    fixture.file("Class.lua", namespace);
    clean(&fixture.check(source));
    clean(&fixture.check_strict(source));
}

#[test]
fn both_effect_policies_check_written_constructor_results_and_accept_the_correction() {
    let fixture = Fixture::new();
    let invalid = FACTORY.replace(
        "function self.read() return 1 end",
        "function self.read() return 'wrong' end",
    );
    for strict in [false, true] {
        let output = fixture.check_effects(&invalid, strict);
        assert_eq!(output.status.code(), Some(1), "{}", text(&output));
        assert!(text(&output).contains("TS2322"), "{}", text(&output));
        clean(&fixture.check_effects(FACTORY, strict));
    }
}
