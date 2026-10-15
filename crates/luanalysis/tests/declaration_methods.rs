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
            "luanalysis declaration methods {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.file("Color.def.lua", declarations);
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
                "Color.def.lua",
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

// Bounded source-faithful reduction of tts-types/tts/Color.def.lua: the class
// attachment, physical colon receiver, four-result get contract, and instance
// copy/lerp contracts retain the forms used by the real library.
const COLOR: &str = r#"---@class tts__Color
---@field r number
---@field g number
---@field b number
---@field a number
Color = {}
---@return number, number, number, number
function Color:get()
    return self.r, self.g, self.b, self.a
end
---@return tts__Color
function Color:copy() end
---@param other tts__Color
---@param t number
---@return tts__Color
function Color:lerp(other, t) end
"#;

#[test]
fn original_color_get_contract_keeps_all_four_results_for_dot_and_colon_calls() {
    let fixture = Fixture::new(
        COLOR,
        "---@type tts__Color\nlocal color\nlocal r,g,b,a=color:get()\n---@type number\nlocal red=r\n---@type number\nlocal green=g\n---@type number\nlocal blue=b\n---@type number\nlocal alpha=a\nlocal dr,dg,db,da=color.get(color)\n---@type number\nlocal dot_alpha=da\n",
    );
    let output = fixture.check();
    pass(&output);
    assert!(text(&output).contains("alpha: number"), "{}", text(&output));
    assert!(
        text(&output).contains("dot_alpha: number"),
        "{}",
        text(&output)
    );
}

#[test]
fn typed_color_parameter_can_call_ambient_instance_methods_without_a_class_table_value() {
    let fixture = Fixture::new(
        COLOR,
        "---@param color tts__Color\n---@return number\nlocal function alpha(color)\nlocal r,g,b,a=color:get()\nreturn a\nend\n---@type tts__Color\nlocal original\n---@type tts__Color\nlocal copied=original:copy()\n---@type tts__Color\nlocal blended=original:lerp(copied,0.5)\n",
    );
    pass(&fixture.check());
}

#[test]
fn ambient_method_generic_inference_excludes_the_implicit_receiver_from_written_parameters() {
    let declarations = format!(
        "{COLOR}---@generic T\n---@param value T\n---@return T\nfunction Color:keep(value) return undefined_placeholder(value) end\n"
    );
    let fixture = Fixture::new(
        &declarations,
        "---@type tts__Color\nlocal color\n---@type number\nlocal numeric=color:keep(42)\n---@type string\nlocal textual=color.keep(color,'word')\n---@generic T:tts__Color\n---@param value T\n---@return number\nlocal function alpha(value)\nlocal r,g,b,a=value:get()\nreturn a\nend\n",
    );
    pass(&fixture.check());
}

#[test]
fn ambient_self_result_preserves_the_actual_derived_receiver() {
    let declarations = format!(
        "{COLOR}---@return self\nfunction Color:identity() return unresolved_placeholder() end\n---@class NamedColor:tts__Color\n---@field name string\n"
    );
    let fixture = Fixture::new(
        &declarations,
        "---@param color NamedColor\n---@return string\nlocal function name(color) return color:identity().name end\n",
    );
    pass(&fixture.check());
}

#[test]
fn ambient_overload_heads_get_exactly_one_physical_receiver_and_keep_both_written_arities() {
    let declarations = format!(
        "{COLOR}---@overload fun():string\n---@overload fun(includeAlpha:boolean):string\nfunction Color:toHex(includeAlpha) return unresolved_placeholder(includeAlpha) end\n"
    );
    let fixture = Fixture::new(
        &declarations,
        "---@type tts__Color\nlocal color\n---@type string\nlocal a=color:toHex()\n---@type string\nlocal b=color:toHex(true)\n---@type string\nlocal c=color.toHex(color)\n---@type string\nlocal d=color.toHex(color,false)\n",
    );
    pass(&fixture.check());
    for source in [
        "---@type tts__Color\nlocal color\nlocal rejected=color.toHex()\n",
        "---@type tts__Color\nlocal color\nlocal rejected=color:toHex(true,false)\n",
    ] {
        fixture.file("main.lua", source);
        fail(&fixture.check(), "TS2554");
    }
    fixture.file(
        "main.lua",
        "---@type tts__Color\nlocal color\nlocal rejected=color.toHex({},true)\n",
    );
    let output = fixture.check();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TS2345") || text(&output).contains("TS2769"),
        "{}",
        text(&output)
    );
}

#[test]
fn explicit_self_contract_validates_the_receiver_but_does_not_add_a_second_receiver_slot() {
    let declarations = format!(
        "{COLOR}---@param self tts__Color\n---@param amount number\n---@return number\nfunction Color:scale(amount) return missing_body_reference(amount) end\n"
    );
    let fixture = Fixture::new(
        &declarations,
        "---@type tts__Color\nlocal color\n---@type number\nlocal a=color:scale(2)\n---@type number\nlocal b=color.scale(color,3)\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "local rejected=Color.scale({},2)\n");
    fail(&fixture.check(), "TS2345");
    fixture.file(
        "main.lua",
        "---@type tts__Color\nlocal color\nlocal rejected=color:scale()\n",
    );
    fail(&fixture.check(), "TS2554");
}

#[test]
fn ambient_placeholder_body_is_never_checked_or_used_to_infer_its_public_result() {
    let declarations = format!(
        "{COLOR}---@param value number\n---@return string\nfunction Color:render(value)\nlocal bad=undefined_global+false\nself.r='wrong'\nreturn 123,undefined_result\nend\n"
    );
    let fixture = Fixture::new(
        &declarations,
        "---@type tts__Color\nlocal color\n---@type string\nlocal rendered=color:render(42)\n",
    );
    let output = fixture.check();
    pass(&output);
    assert!(
        !text(&output).contains("undefined_global"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("TS2322"), "{}", text(&output));
    fixture.file(
        "main.lua",
        "---@type tts__Color\nlocal color\n---@type number\nlocal rejected=color:render(42)\n",
    );
    fail(&fixture.check(), "TS2322");
}

#[test]
fn actual_cli_reports_the_original_unicode_crlf_argument_then_accepts_the_correction() {
    let fixture = Fixture::new(
        COLOR,
        "-- café😀\r\n---@type tts__Color\r\nlocal color\r\nlocal rejected=color:lerp(color,'wrong')\r\n",
    );
    let output = fixture.check();
    fail(&output, "main.lua:4:33: error[TS2345]");
    assert!(
        !text(&output).contains("Color.def.lua:"),
        "{}",
        text(&output)
    );
    fixture.file(
        "main.lua",
        "-- café😀\r\n---@type tts__Color\r\nlocal color\r\nlocal accepted=color:lerp(color,0.5)\r\n",
    );
    pass(&fixture.check());
}

#[test]
fn unattached_colon_declarations_remain_an_explicit_original_owner_error() {
    for declaration in [
        "---@return number\nfunction Missing:get() return 42 end\n",
        "---@class First\nColor={}\n---@class Second\nColor={}\n---@return number\nfunction Color:get() return 42 end\n",
        "---@class tts__Color\nColor={}\n---@return number\nfunction Color.nested:get() return 42 end\n",
    ] {
        let fixture = Fixture::new(declaration, "local independent=42\n");
        fail(&fixture.check(), "LUA2038");
    }
}

#[test]
fn syntactically_empty_declaration_stubs_without_return_tags_have_no_results() {
    let declarations = format!(
        "{COLOR}---@param value number\nfunction Color:setRed(value) end\n---@param name string\n---@param value tts__Color\nfunction Color.Add(name,value) end\nfunction noResult() end\n---@return number\nfunction consumeNumber(value) return undefined_placeholder() end\n"
    );
    // Keep every written parameter explicit; only the absence of a return tag
    // on a syntactically empty ambient body supplies the empty result contract.
    let declarations = declarations.replace(
        "---@return number\nfunction consumeNumber(value)",
        "---@param value number\n---@return number\nfunction consumeNumber(value)",
    );
    let fixture = Fixture::new(
        &declarations,
        "---@type tts__Color\nlocal color\ncolor:setRed(42)\nColor.Add('red',color)\nnoResult()\n---@type nil\nlocal absent=color:setRed(42)\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "---@type tts__Color\nlocal color\n---@type number\nlocal wrong=color:setRed(42)\n");
    fail(&fixture.check(), "TS2322");
    fixture.file("main.lua", "---@type tts__Color\nlocal color\nconsumeNumber(color:setRed(42))\n");
    fail(&fixture.check(), "TS2554");
}

#[test]
fn nonempty_unannotated_ambient_results_are_not_inferred_from_placeholder_statements() {
    for body in ["return 42", "unresolved_placeholder()", "do end", "return"] {
        let declarations = format!("{COLOR}function Color:unknownResult() {body} end\n");
        let fixture = Fixture::new(&declarations, "local independent=42\n");
        let output = fixture.check();
        fail(&output, "LUA2035");
        assert!(!text(&output).contains("TS2304"), "{}", text(&output));
    }
}
