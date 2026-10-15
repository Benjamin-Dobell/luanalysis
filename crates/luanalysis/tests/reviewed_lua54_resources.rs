use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "luanalysis reviewed lua54 {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path);
        fixture.source(source);
        fixture
    }
    fn source(&self, source: &str) {
        fs::write(self.0.join("main.lua"), source).unwrap();
    }
    fn check(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&self.0)
            .args(["check", "--dump-types", "--library"])
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib/Lua54"))
            .arg("main.lua")
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

#[test]
fn reviewed_resources_validate_together_and_keep_literal_base_contracts() {
    let fixture = Fixture::new(
        "---@type 'Lua 5.4'\nlocal version = _VERSION\n---@type string\nlocal rendered = tostring({})\n---@type 'nil'|'number'|'string'|'boolean'|'table'|'function'|'thread'|'userdata'\nlocal category = type(true)\nprint(version, rendered, category)\n",
    );
    pass(&fixture.check());
}

#[test]
fn reviewed_math_public_arities_varargs_and_two_result_contracts_are_preserved() {
    let fixture = Fixture::new(
        "local a = math.atan(1)\nlocal b = math.atan(1,2)\nlocal c = math.log(2)\nlocal d = math.log(2,10)\nlocal r0 = math.random()\nlocal r1 = math.random(2)\nlocal r2 = math.random(1,2)\nlocal s0,t0 = math.randomseed()\nlocal s1,t1 = math.randomseed(1)\nlocal s2,t2 = math.randomseed(1,2)\nlocal maximum = math.max(1,2,3)\nlocal minimum = math.min(1,2,3)\nlocal whole,fraction = math.modf(1.5)\n---@type number\nlocal checked_fraction = fraction\n---@type number|nil\nlocal integer = math.tointeger({})\n---@type 'integer'|'float'|nil\nlocal category = math.type({})\n",
    );
    pass(&fixture.check());
    fixture.source("local rejected = math.atan(1,2,3)\n");
    fail(&fixture.check(), "TS2554");
    fixture.source("local rejected = math.max(1,'wrong')\n");
    fail(&fixture.check(), "TS2345");
}

#[test]
fn save_manager_reduction_requires_a_nullable_conversion_guard_and_accepts_correction() {
    // ge_tts/SaveManager.ttslua:184,190 converts parsed substring lengths.
    let fixture = Fixture::new(
        "local saved = '12 state'\nlocal length = tonumber(string.sub(saved,1,2))\n---@type number\nlocal unsafe = length\n",
    );
    fail(&fixture.check(), "main.lua:4:16: error[TS2322]");
    fixture.source("local saved = '12 state'\nlocal length = tonumber(string.sub(saved,1,2))\nif length == nil then error('invalid saved length') end\n---@type number\nlocal checked = length\nlocal based = tonumber('ff',16)\n");
    pass(&fixture.check());
    fixture.source("local rejected = tonumber(42,16)\n");
    fail(&fixture.check(), "TS2345");
}

#[test]
fn reviewed_table_public_arities_and_absent_remove_result_are_checked() {
    let fixture = Fixture::new(
        "local values = {1,2}\ntable.insert(values,3)\ntable.insert(values,1,4)\nlocal c1 = table.concat(values)\nlocal c2 = table.concat(values,',')\nlocal c3 = table.concat(values,',',1)\nlocal c4 = table.concat(values,',',1,2)\ntable.sort(values)\ntable.sort(values,function(a,b) return a < b end)\nlocal removed = table.remove(values)\nlocal other = table.remove(values,1)\nif removed == nil then error('empty') end\n---@type number\nlocal checked = removed\n",
    );
    pass(&fixture.check());
    fixture.source("---@type number\nlocal wrong = table.sort({1,2})\n");
    fail(&fixture.check(), "TS2322");
    fixture.source("---@type number\nlocal wrong = table.remove({1,2})\n");
    fail(&fixture.check(), "TS2322");
}

#[test]
fn zone_comparator_and_ipairs_reduction_keep_contextual_record_fields() {
    // ge_tts/Zone.ttslua:490-495 sorts distance records then iterates them.
    let fixture = Fixture::new(
        "---@type {distanceSquared:number,zone:string}[]\nlocal distances = {{distanceSquared=4,zone='far'},{distanceSquared=1,zone='near'}}\ntable.sort(distances,function(a,b) return a.distanceSquared < b.distanceSquared end)\nfor index,record in ipairs(distances) do\n---@type number\nlocal checked_index = index\n---@type string\nlocal checked_zone = record.zone\nend\n---@type any,any,0\nlocal iterator,state,initial = ipairs(distances)\n---@type 0\nlocal zero = initial\n",
    );
    pass(&fixture.check());
}

#[test]
fn homogeneous_pairs_next_and_ipairs_use_native_iterator_contracts() {
    let fixture = Fixture::new(
        "---@param values table<boolean,string>\nlocal function visit(values)\nfor key,value in pairs(values) do\n---@type boolean\nlocal checked_key = key\n---@type string\nlocal checked_value = value\nend\nlocal iterator,state,initial = pairs(values)\n---@type nil\nlocal empty_control = initial\nlocal key,value = next(values)\nend\nvisit({[false]='false key',[true]='true key'})\n",
    );
    pass(&fixture.check());
}

#[test]
fn debug_gsub_reduction_scalarizes_chained_results_and_retains_two_result_calls() {
    // ge_tts/Debug.ttslua:24 chains substitutions into one identifier.
    let fixture = Fixture::new(
        "local packageName = 'ge_tts.Debug'\nlocal identifier = string.gsub(string.gsub(packageName,'^ge_tts%.',''),'[/%.]','__')\n---@type string\nlocal checked = identifier\nlocal replaced,count = string.gsub(packageName,'%.','_')\n---@type number\nlocal checked_count = count\n---@param value string\nlocal function consume(value) end\nconsume((string.gsub(packageName,'%.','_')))\n",
    );
    pass(&fixture.check());
    fixture.source("---@param value string\nlocal function consume(value) end\nconsume(string.gsub('a.b','%.','_'))\n");
    fail(&fixture.check(), "TS2554");
}

#[test]
fn http_lowercase_header_callback_reduction_preserves_string_dictionary_values() {
    // ge_tts/Http.ttslua:33,36 normalizes requested and actual header names.
    let fixture = Fixture::new(
        "---@param headers table<string,string>\n---@param name string\n---@return string|nil\nlocal function header(headers,name)\nlocal wanted = string.lower(name)\nfor key,value in pairs(headers) do\nif string.lower(key) == wanted then return value end\nend\nreturn nil\nend\nlocal result = header({['Content-Type']='application/json'},'content-type')\n---@type string|nil\nlocal checked = result\n",
    );
    pass(&fixture.check());
}

#[test]
fn practical_string_dot_contracts_keep_nullable_captures_and_dynamic_formatting() {
    let fixture = Fixture::new(
        "local length = string.len('word')\nlocal lower = string.lower('WORD')\nlocal upper = string.upper('word')\nlocal sub2 = string.sub('word',2)\nlocal sub3 = string.sub('word',2,3)\nlocal byte = string.byte('word')\nlocal first,last = string.find('word','or',1,true)\n---@type number|nil\nlocal checked_first = first\nlocal capture = string.match('123','(%d+)')\n---@type string|number|nil\nlocal checked_capture = capture\nlocal repeated = string.rep('a',2,'-')\nlocal reversed = string.reverse('word')\nlocal chars = string.char(65,66)\nlocal formatted = string.format('%s:%d','value',42)\n---@type string\nlocal checked_format = formatted\n",
    );
    pass(&fixture.check());
    fixture.source("local rejected = string.lower({})\n");
    fail(&fixture.check(), "TS2345");
}

#[test]
fn original_pack_related_declarations_are_available_with_their_written_contracts() {
    // This checks declaration coverage and ordinary argument/result contracts.
    // It does not establish protected-call result correlation, arbitrary pack
    // forwarding, or inference of pack/unpack values from a format string.
    let fixture = Fixture::new(
        "---@return number\nlocal function callback() return 42 end\n---@type boolean\nlocal succeeded = pcall(callback)\n---@type string\nlocal packed = string.pack('zz','first','second')\n---@type number\nlocal size = string.packsize('i')\n",
    );
    pass(&fixture.check());
    fixture.source("local rejected = string.packsize(42)\n");
    fail(&fixture.check(), "TS2345");
    fixture.source("---@type number\nlocal corrected = string.packsize('i')\n");
    pass(&fixture.check());
}
