use luanalysis::check;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const FACTORY: &str = r#"
---@alias Factory fun(count:number,label:string):number
---@param option number
local function outer(option)
    ---@param callback fun():void
    local function start(callback) callback() end
    start(function()
        ---@type Factory
        local factory=function(count,label)
            ---@type number
            local checked=count
            ---@type string
            local text=label
            return checked+option
        end
        factory(42,'text')
    end)
end
"#;

#[test]
fn alias_factory_inside_a_contextual_callback_keeps_its_selected_parameter_facet() {
    let report = check(FACTORY);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
    let count = FACTORY.find("function(count,label)").unwrap() + "function(".len();
    assert_eq!(report.type_at(count), Some("number"));
    assert_eq!(report.type_at(count + "count,".len()), Some("string"));

    let rejected = FACTORY.replace("local checked=count", "local checked=label");
    let report = check(&rejected);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    let report = check(FACTORY);
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis alias contextual factory {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str) -> Output {
        self.check_with_profile(source, false)
    }
    fn check_with_profile(&self, source: &str, lua54: bool) -> Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis"));
        command.current_dir(&self.0).arg("check");
        if lua54 {
            command.args(["--lua-version", "5.4"]);
        } else {
            command.args(["--no-stdlib", "--stdlib", "require"]);
        }
        command
            .args(["--dump-types", "--module-root", ".", "main.lua"])
            .output()
            .unwrap()
    }
}

#[test]
fn environment_capture_reentry_restores_the_ancestor_factory_signature() {
    let fixture = Fixture::new();
    let source = r#"
---@alias Factory fun(count:number,label:string):fun(value:boolean):void
---@param option number
local function outer(option)
    ---@type Factory
    local factory=function(count,label)
        local chunk=option or 128
        ---@type number
        local checked=count
        ---@type string
        local text=label
        return function(value)
            local category=type(value)
            local copied=count
        end
    end
end
"#;
    let output = fixture.check_with_profile(source, true);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let displayed = text(&output);
    for expected in [
        "count: number",
        "label: string",
        "value: boolean",
        "copied: number",
    ] {
        assert!(displayed.contains(expected), "{displayed}");
    }
    let output = fixture.check_with_profile(
        &source.replace("local checked=count", "local checked=label"),
        true,
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("TS2322"), "{}", text(&output));
    assert!(!text(&output).contains("TS7006"), "{}", text(&output));
    let output = fixture.check_with_profile(source, true);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
}

#[test]
fn creation_ancestry_maps_independent_generic_factory_instances() {
    let fixture = Fixture::new();
    let source = r#"
---@generic T
---@param captured T
local function outer(captured)
    ---@type fun():fun(value:T):T
    local factory=function()
        local chunk=captured or captured
        return function(value)
            local category=type(value)
            return value
        end
    end
    return factory()
end
local numeric=outer(42)
local textual=outer('text')
---@type number
local number=numeric(7)
---@type string
local string=textual('correct')
"#;
    let output = fixture.check_with_profile(source, true);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let displayed = text(&output);
    assert!(
        displayed.contains("numeric: fun(value: number): number"),
        "{displayed}"
    );
    assert!(
        displayed.contains("textual: fun(value: string): string"),
        "{displayed}"
    );
    let output =
        fixture.check_with_profile(&source.replace("numeric(7)", "numeric('wrong')"), true);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("TS2345"), "{}", text(&output));
    assert!(!text(&output).contains("TS7006"), "{}", text(&output));
    let output = fixture.check_with_profile(source, true);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
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

#[test]
fn imported_json_style_factory_alias_types_callbacks_and_returned_closures() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("encoder.lua"),
        "---@alias GenerateEncode fun(nullv:any,push:(fun(component:string):void),replace:(fun(replacer:(fun(builder:string[],next:number):string[],number)):void)):(fun(v:any):void)\nreturn {}\n",
    )
    .unwrap();
    let source = r#"
local encoder=require('encoder')
---@param callback fun():void
local function start(callback) callback() end
---@param option number
local function outer(option)
    local canceled=false
    start(function()
        ---@param builder string[]
        ---@param nextIndex number
        ---@return string[],number
        local function replacer(builder,nextIndex) return builder,nextIndex end
        ---@type GenerateEncode
        local generate=function(nullv,push,replaceBuilder)
            local count=option
            return function(v)
                if canceled then return end
                if v==nullv then push('null') end
                count=count+1
                replaceBuilder(replacer)
            end
        end
    end)
end
"#;
    let output = fixture.check(source);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let displayed = text(&output);
    assert!(displayed.contains("nullv: any"), "{displayed}");
    assert!(displayed.contains("v: any"), "{displayed}");
    assert!(
        displayed.contains("push: fun(component: string): void"),
        "{displayed}"
    );

    let rejected = source.replace("push('null')", "push(42)");
    let output = fixture.check(&rejected);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("TS2345"), "{}", text(&output));
    assert!(!text(&output).contains("TS7006"), "{}", text(&output));
    let output = fixture.check(source);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
}
