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
            "luanalysis ambient main heads {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.file("api.def.lua", declarations);
        fixture.file("main.lua", source);
        fixture
    }
    fn file(&self, name: &str, source: &str) {
        fs::write(self.0.join(name), source).unwrap();
    }
    fn check(&self) -> Output {
        self.check_with_arguments(&[])
    }
    fn check_with_arguments(&self, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args([
                "check",
                "--no-stdlib",
                "--dump-types",
                "--library",
                "api.def.lua",
                "main.lua",
            ])
            .args(extra)
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
fn argument_failure(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{}", text(output));
    assert!(
        text(output).contains("TS2345") || text(output).contains("TS2769"),
        "{}",
        text(output)
    );
}

// These excerpts come from the active declaration resources, where every
// intended public form is spelled by an original explicit overload head.
const ACTIVE_GLOBAL: &str = include_str!("../stdlib/Lua54/global.def.lua");
const ACTIVE_TABLE: &str = include_str!("../stdlib/Lua54/table.def.lua");
fn excerpt<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let from = source.find(start).expect("original declaration start");
    let to = source[from..].find(end).expect("original declaration end") + from + end.len();
    &source[from..to]
}
fn tonumber() -> &'static str {
    excerpt(
        ACTIVE_GLOBAL,
        "---@overload fun(e: number | string): nil | number",
        "function tonumber(e, base) end",
    )
}
fn concat() -> String {
    let owner = excerpt(ACTIVE_TABLE, "---@class std__Table", "table = {}");
    let packed = excerpt(ACTIVE_TABLE, "---@shape std__Packed<T>", "---@field n number");
    let method = excerpt(
        ACTIVE_TABLE,
        "---@overload fun(list:string[]|number[]|(string|number)[]|std__Packed<string|number>):string",
        "function table.concat(list, sep, i, j) end",
    );
    format!("{owner}\n{packed}\n{method}\n")
}

#[test]
fn active_tonumber_resource_exposes_both_explicit_public_arities() {
    let fixture = Fixture::new(
        tonumber(),
        "---@type number|nil\nlocal one=tonumber('42')\n---@type number|nil\nlocal numeric=tonumber(42)\n---@type number|nil\nlocal two=tonumber('ff',16)\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "local rejected=tonumber(255,16)\n");
    argument_failure(&fixture.check());
    fixture.file("main.lua", "local accepted=tonumber('ff',16)\n");
    pass(&fixture.check());
}

#[test]
fn active_table_concat_resource_exposes_all_four_explicit_public_arities() {
    let fixture = Fixture::new(
        &concat(),
        "---@param values string[]\nlocal function check(values)\n---@type string\nlocal a=table.concat(values)\n---@type string\nlocal b=table.concat(values,',')\n---@type string\nlocal c=table.concat(values,',',1)\n---@type string\nlocal d=table.concat(values,',',1,2)\nend\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "---@param values string[]\nlocal function check(values)\nlocal rejected=table.concat(values,',',1,'wrong')\nend\n");
    argument_failure(&fixture.check());
    fixture.file("main.lua", "---@param values string[]\nlocal function check(values)\nlocal accepted=table.concat(values,',',1,2)\nend\n");
    pass(&fixture.check());
}

#[test]
fn generic_written_main_stays_private_until_its_public_head_is_explicit() {
    let private = "---@overload fun(value:number):number\n---@generic T\n---@param first T\n---@param second? T\n---@return T\nfunction choose(first,second) return nonexistent_placeholder() end\n";
    let fixture = Fixture::new(
        private,
        "---@type number\nlocal one=choose(42)\n---@type string\nlocal two=choose('left','right')\n---@type boolean\nlocal three=choose(true,false)\n",
    );
    fail(&fixture.check(), "TS2554");
    let declarations = private.replace(
        "---@generic T",
        "---@overload fun<T>(first:T,second:T):T\n---@generic T",
    );
    fixture.file("api.def.lua", &declarations);
    pass(&fixture.check());
    fixture.file(
        "main.lua",
        "---@type number\nlocal rejected=choose('left','right')\n",
    );
    fail(&fixture.check(), "TS2322");
    fixture.file(
        "main.lua",
        "---@type string\nlocal accepted=choose('left','right')\n",
    );
    pass(&fixture.check());
}

#[test]
fn explicit_colon_heads_share_one_receiver_with_a_compatible_private_self_contract() {
    let fixture = Fixture::new(
        "---@class Color\n---@field r number\nColor={}\n---@overload fun():string\n---@overload fun(alpha:boolean):string\n---@param self Color\n---@param alpha? boolean\n---@return string\nfunction Color:toHex(alpha) return unresolved_placeholder(self,alpha) end\n",
        "---@type Color\nlocal color\n---@type string\nlocal a=color:toHex()\n---@type string\nlocal b=color:toHex(true)\n---@type string\nlocal c=color.toHex(color)\n---@type string\nlocal d=color.toHex(color,false)\n",
    );
    pass(&fixture.check());
    fixture.file(
        "main.lua",
        "---@type Color\nlocal color\nlocal rejected=color:toHex('wrong')\n",
    );
    argument_failure(&fixture.check());
    fixture.file("main.lua", "local rejected=Color.toHex({},true)\n");
    argument_failure(&fixture.check());
    fixture.file(
        "main.lua",
        "---@type Color\nlocal color\nlocal accepted=color.toHex(color,true)\n",
    );
    pass(&fixture.check());
}

#[test]
fn incomplete_ambient_main_is_not_exposed_as_an_any_accepting_signature() {
    let fixture = Fixture::new(
        "---@overload fun(value:number):number\nfunction exact(value,extra) return missing_placeholder(value,extra) end\n",
        "---@type number\nlocal accepted=exact(42)\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "local rejected=exact(42,'extra')\n");
    fail(&fixture.check(), "TS2554");
    fixture.file("main.lua", "local rejected=exact(false)\n");
    argument_failure(&fixture.check());
}

#[test]
fn partially_annotated_ambient_main_does_not_publish_untyped_written_parameters() {
    let fixture = Fixture::new(
        "---@overload fun(value:number):number\n---@param value number\n---@return number\nfunction exact(value,extra) return missing_placeholder(extra) end\n",
        "local accepted=exact(42)\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "local rejected=exact(42,'extra')\n");
    fail(&fixture.check(), "TS2554");
}

#[test]
fn runtime_implementation_remains_hidden_even_when_its_main_contract_is_fully_written() {
    let fixture = Fixture::new(
        "",
        "---@overload fun(value:number):number\n---@param value number|string\n---@param extra? boolean\n---@return number|string\nlocal function exact(value,extra) return value end\nlocal accepted=exact(42)\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "---@overload fun(value:number):number\n---@param value number|string\n---@param extra? boolean\n---@return number|string\nlocal function exact(value,extra) return value end\nlocal rejected=exact('hidden')\n");
    argument_failure(&fixture.check());
    fixture.file("main.lua", "---@overload fun(value:number):number\n---@param value number|string\n---@param extra? boolean\n---@return number|string\nlocal function exact(value,extra) return value end\nlocal rejected=exact(42,true)\n");
    fail(&fixture.check(), "TS2554");
}

#[test]
fn written_function_type_main_stays_private_and_only_covers_explicit_public_heads() {
    let private = "---@overload fun(value:number):number\n---@type fun(first:number|string,second?:string):number|string\nfunction join(first,second) return undefined_placeholder(first,second) end\n";
    let fixture = Fixture::new(
        private,
        "---@type number\nlocal one=join(42)\n---@type string\nlocal two=join('left','right')\n",
    );
    fail(&fixture.check(), "TS2554");
    fixture.file(
        "api.def.lua",
        &private.replace(
            "---@type",
            "---@overload fun(first:string,second:string):string\n---@type",
        ),
    );
    pass(&fixture.check());
    fixture.file("main.lua", "local rejected=join('left',false)\n");
    argument_failure(&fixture.check());
    fixture.file("main.lua", "local accepted=join('left','right')\n");
    pass(&fixture.check());
}

#[test]
fn original_ambient_ellipsis_without_a_vararg_tag_is_an_unknown_tail() {
    let fixture = Fixture::new(
        "---@param prefix string\nfunction report(prefix,...) end\n---@overload fun(value:number):number\n---@param prefix number|string\n---@return number\nfunction dispatch(prefix,...) return missing_placeholder(...) end\n",
        "local original=report\nreport('message',42,false,{value='text'})\nreport('empty')\nlocal absent=report('message')\n---@type nil\nlocal checked=absent\n---@type number\nlocal count=dispatch(42)\n",
    );
    let output = fixture.check();
    pass(&output);
    assert!(text(&output).contains("...: unknown"), "{}", text(&output));
    fixture.file("main.lua", "dispatch('message',false,42)\n");
    fail(&fixture.check(), "TS2554");
    fixture.file(
        "api.def.lua",
        "---@param prefix string\nfunction report(prefix,...) end\n---@overload fun(value:number):number\n---@overload fun(prefix:string,...:unknown):number\n---@param prefix number|string\n---@return number\nfunction dispatch(prefix,...) return missing_placeholder(...) end\n",
    );
    pass(&fixture.check());
    fixture.file("main.lua", "report(42,'message')\n");
    argument_failure(&fixture.check());
    fixture.file("main.lua", "report('message',42)\n");
    pass(&fixture.check());
}

#[test]
fn strict_runtime_ellipsis_without_a_vararg_tag_reports_implicit_any() {
    let fixture = Fixture::new("", "local function report(...) end\nreport(42)\n");
    fail(&fixture.check_with_arguments(&["--no-implicit-any"]), "TS7019");
}

#[test]
fn nested_placeholder_functions_do_not_become_public_ambient_main_declarations() {
    let fixture = Fixture::new(
        "---@param value number\n---@return string\nfunction render(value)\n---@return MissingBodyType\nlocal function placeholder() return unknown_body_value end\nreturn placeholder()\nend\n",
        "---@type string\nlocal value=render(42)\n",
    );
    pass(&fixture.check());
}
