use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis overload failure {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn check(&self, source: &str) -> std::process::Output {
        fs::write(self.0.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--dump-types",
                "--library",
                "api.def.lua",
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
fn text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn original_ambient_overloads_reject_jointly_invalid_arguments_in_both_head_orders() {
    let one = "---@overload fun(value:any):number|nil\n";
    let two = "---@overload fun(value:string,base:number):number|nil\n";
    for headers in [format!("{one}{two}"), format!("{two}{one}")] {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("api.def.lua"),
            format!("{headers}function parse(...) end\n"),
        )
        .unwrap();
        for source in ["local result = parse(42,16)", "local result = parse({},16)"] {
            let output = fixture.check(source);
            assert_eq!(output.status.code(), Some(1), "{}", text(&output));
            assert!(
                text(&output).contains("main.lua:1:22: error[TS2345]"),
                "{}",
                text(&output)
            );
            assert!(!text(&output).contains("api.def.lua:"), "{}", text(&output));
        }
        for source in ["local result = parse(42)", "local result = parse('ff',16)"] {
            let output = fixture.check(source);
            assert_eq!(output.status.code(), Some(0), "{}", text(&output));
        }
        let wrong_second = fixture.check("local result = parse('ff','bad')");
        assert_eq!(
            wrong_second.status.code(),
            Some(1),
            "{}",
            text(&wrong_second)
        );
        assert!(
            text(&wrong_second).contains("error[TS2345]"),
            "{}",
            text(&wrong_second)
        );
        let too_many = fixture.check("local result = parse('ff',16,3)");
        assert_eq!(too_many.status.code(), Some(1), "{}", text(&too_many));
        assert!(
            text(&too_many).contains("error[TS2554]"),
            "{}",
            text(&too_many)
        );
    }
}

#[test]
fn failed_generic_heads_keep_their_instantiated_parameter_constraints() {
    let one = "---@overload fun<T:number>(value:T):T\n";
    let two = "---@overload fun<T:string>(value:T,base:number):T\n";
    for headers in [format!("{one}{two}"), format!("{two}{one}")] {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("api.def.lua"),
            format!("{headers}function parse(...) unresolved_placeholder() end\n"),
        )
        .unwrap();
        for source in [
            "local result = parse(42,16)",
            "local result = parse('ff','bad')",
            "local result = parse(true)",
        ] {
            let bad = fixture.check(source);
            assert_eq!(bad.status.code(), Some(1), "{}", text(&bad));
            assert!(text(&bad).contains("error[TS2345]"), "{}", text(&bad));
            assert!(!text(&bad).contains("api.def.lua:"), "{}", text(&bad));
        }
        for source in [
            "---@type number\nlocal result = parse(42)",
            "---@type string\nlocal result = parse('ff',16)",
        ] {
            let corrected = fixture.check(source);
            assert_eq!(corrected.status.code(), Some(0), "{}", text(&corrected));
        }
    }
}

#[test]
fn direct_literal_heads_keep_priority_and_reject_nonmatching_two_argument_calls() {
    let literal = "---@overload fun(value:\"hex\",base:number):\"literal\"\n";
    let general = "---@overload fun(value:string):\"general\"\n";
    for headers in [format!("{literal}{general}"), format!("{general}{literal}")] {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("api.def.lua"),
            format!("{headers}function parse(...) unresolved_placeholder() end\n"),
        )
        .unwrap();
        let bad = fixture.check("local result = parse('other',16)");
        assert_eq!(bad.status.code(), Some(1), "{}", text(&bad));
        assert!(text(&bad).contains("error[TS2345]"), "{}", text(&bad));
        for source in [
            "---@type \"literal\"\nlocal result = parse('hex',16)",
            "---@type \"general\"\nlocal result = parse('other')",
        ] {
            let corrected = fixture.check(source);
            assert_eq!(corrected.status.code(), Some(0), "{}", text(&corrected));
        }
    }
}

#[test]
fn runtime_implementation_signature_is_not_a_public_failure_fallback() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("api.def.lua"), "").unwrap();
    let declarations = "---@overload fun(value:any):number|nil\n\
---@overload fun(value:string,base:number):number|nil\n\
---@param value any\n---@param base? number\n---@return any\n\
local function parse(value,base) return value end\n";
    let bad = fixture.check(&format!("{declarations}local result = parse(42,16)"));
    assert_eq!(bad.status.code(), Some(1), "{}", text(&bad));
    assert!(text(&bad).contains("error[TS2345]"), "{}", text(&bad));
    let public_heads = text(&bad);
    let displayed = public_heads
        .lines()
        .find(|line| line.contains("parse: "))
        .unwrap();
    assert!(displayed.contains("fun(value: any):"), "{displayed}");
    assert!(
        displayed.contains("fun(value: string, base: number):"),
        "{displayed}"
    );
    assert!(!displayed.contains("base?:"), "{displayed}");
    let corrected = fixture.check(&format!("{declarations}local result = parse('ff',16)"));
    assert_eq!(corrected.status.code(), Some(0), "{}", text(&corrected));
}
