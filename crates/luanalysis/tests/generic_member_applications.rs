use luanalysis::{CheckReport, check};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn rejects(source: &str, code: &str) {
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|error| error.code == code),
        "{source}\n{:#?}",
        report.diagnostics
    );
}

fn ty<'a>(report: &'a CheckReport, name: &str) -> &'a str {
    &report
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .expect(name)
        .ty
}

const GENERIC: &str = r#"---@class Box<T>
---@field value T

---@generic T
---@param input Box<T>
local function attach(input)
    local object = --[[---@type Box<T>]] input
    ---@param value T
    function object.echo(value)
        ---@type T
        local captured = input.value
        return value, captured
    end
    return object
end

---@param numeric Box<number>
---@param textual Box<string>
local function use(numeric, textual)
    local numberResult = attach(numeric).echo(1)
    local stringResult = attach(textual).echo('text')
    return numberResult, stringResult
end
"#;

#[test]
fn applied_local_owner_preserves_distinct_class_and_lexical_generic_identities() {
    let report = clean(GENERIC);
    assert_eq!(ty(&report, "numberResult"), "number");
    assert_eq!(ty(&report, "stringResult"), "string");
    rejects(
        &GENERIC.replace("attach(numeric).echo(1)", "attach(numeric).echo('wrong')"),
        "TS2345",
    );
}

#[test]
fn distinct_fixed_owner_applications_keep_their_original_member_contracts() {
    let source = r#"---@class Box<T>
---@field value T

---@param input Box<number>
local function numeric(input)
    ---@type Box<number>
    local object = input
    function object.read() return input.value end
    return object
end

---@param input Box<string>
local function textual(input)
    ---@type Box<string>
    local object = input
    function object.read() return input.value end
    return object
end

---@param n Box<number>
---@param s Box<string>
local function use(n, s)
    local numberResult = numeric(n).read()
    local stringResult = textual(s).read()
    return numberResult, stringResult
end
"#;
    let report = clean(source);
    assert_eq!(ty(&report, "numberResult"), "number");
    assert_eq!(ty(&report, "stringResult"), "string");
}

#[test]
fn noninvertible_owner_array_pattern_does_not_publish_on_scalar_specializations() {
    let source = r#"---@class Box<T>
---@field value T

---@generic U
---@param input Box<U[]>
local function attach(input)
    local object = --[[---@type Box<U[]>]] input
    ---@param value U
    function object.echo(value) return value end
    return object
end

---@param array Box<number[]>
local function use(array)
    local numberResult = attach(array).echo(1)
    return numberResult
end
"#;
    let report = clean(source);
    assert_eq!(ty(&report, "numberResult"), "number");
    rejects(
        &format!(
            "{source}\n---@param scalar Box<number>\nlocal function invalid(scalar) return scalar.echo(1) end\n"
        ),
        "TS2339",
    );
}

#[test]
fn bodyless_captured_alias_application_maps_its_original_enclosing_formal() {
    let source = r#"---@class Box<T>
---@field value T

---@generic U
---@param input Box<U>
local function attach(input)
    ---@alias Captured Box<U>
    ---@function Captured:read():U

    ---@type Captured
    local object = input
    return object
end

---@param numeric Box<number>
---@param textual Box<string>
local function use(numeric, textual)
    local numberResult = attach(numeric):read()
    local stringResult = attach(textual):read()
    return numberResult, stringResult
end
"#;
    let report = clean(source);
    assert_eq!(ty(&report, "numberResult"), "number");
    assert_eq!(ty(&report, "stringResult"), "string");
}

#[test]
fn applied_colon_method_keeps_its_owner_arguments_and_polymorphic_derived_self() {
    let source = r#"---@class Box<T>
---@field value T

---@generic U
---@param input Box<U>
local function attach(input)
    local object = --[[---@type Box<U>]] input
    ---@param value U
    ---@return self
    function object:replace(value)
        self.value = value
        return self
    end
    return object
end

---@class Derived:Box<number>
---@field extra string

---@param derived Derived
local function use(derived)
    local result = derived:replace(1)
    ---@type Derived
    local same = result
    return same.extra
end
"#;
    clean(source);
    rejects(
        &source.replace("derived:replace(1)", "derived:replace('wrong')"),
        "TS2345",
    );
}

#[test]
fn fixed_owner_method_and_public_overload_preserve_derived_self() {
    let source = r#"---@class Box<T>
---@field value T

---@param input Box<number>
local function attach(input)
    local object = --[[---@type Box<number>]] input
    ---@overload fun(value:number):self
    ---@param value number|string
    ---@return self
    function object:replace(value)
        if type(value) == 'number' then self.value = value end
        return self
    end
    return object
end

---@class Derived:Box<number>
---@field extra string

---@param derived Derived
local function use(derived)
    local result = derived:replace(1)
    ---@type Derived
    local same = result
    return same.extra
end
"#;
    clean(source);
    rejects(
        &source.replace("derived:replace(1)", "derived:replace('wrong')"),
        "TS2345",
    );
}

#[test]
fn infinite_container_methods_share_the_original_lexical_generic_capture() {
    // The typed-local receiver and mutable filter match ge_tts's constructor pattern.
    let source = r#"---@class Instance
---@field destroy fun():void

---@class InfiniteContainerInstance<T:Instance>
---@field setFilterInstanceEnter fun(callback:(nil|fun(instance:T):boolean)):void
---@field filterInstance fun(instance:T):boolean

---@generic T:Instance
---@param input InfiniteContainerInstance<T>
local function attach(input)
    local object = --[[---@type InfiniteContainerInstance<T>]] input
    ---@type nil|fun(instance:T):boolean
    local filter = nil

    ---@param callback nil|fun(instance:T):boolean
    function object.setFilterInstanceEnter(callback)
        filter = callback
    end

    ---@param instance T
    ---@return boolean
    function object.filterInstance(instance)
        return not filter or filter(instance)
    end
    return object
end

---@class Item:Instance
---@field id number

---@param input InfiniteContainerInstance<Item>
local function use(input)
    local object = attach(input)
    object.setFilterInstanceEnter(function(item)
        ---@type number
        local id = item.id
        return id > 0
    end)
end
"#;
    clean(source);
    rejects(
        &source.replace("local id = item.id", "local id = 'wrong'"),
        "TS2322",
    );
    rejects(&source.replace("item.id", "item.missing"), "TS2339");
}

#[test]
fn typed_local_alias_keeps_its_original_application_for_native_owner_resolution() {
    let source = r#"---@class Box<T>
---@field value T

---@alias Wrapped<E> Box<E[]>

---@generic U
---@param input Wrapped<U>
local function attach(input)
    local object = --[[---@type Wrapped<U>]] input
    ---@param value U
    function object.echo(value) return value end
    return object
end

---@param numeric Box<number[]>
local function use(numeric)
    local numberResult = attach(numeric).echo(1)
    return numberResult
end
"#;
    let report = clean(source);
    assert_eq!(ty(&report, "numberResult"), "number");
    rejects(&source.replace(".echo(1)", ".echo('wrong')"), "TS2345");
}

#[test]
fn cli_routes_foreign_applied_members_through_original_owner_scope() {
    use std::{
        fs,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-member-application-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("types.def.lua"),
        "---@class Box<T>\n---@field value T\n",
    )
    .unwrap();
    fs::write(
        directory.join("augment.lua"),
        r#"---@alias Wrapped<E> Box<E[]>
---@generic U
---@param input Wrapped<U>
local function attach(input)
    local object = --[[---@type Wrapped<U>]] input
    ---@param value U
    function object.echo(value) return value end
    return object
end
return attach
"#,
    )
    .unwrap();
    let source = "local attach=require('augment')\n---@param input Box<number[]>\nlocal function use(input)\n local numberResult=attach(input).echo(1)\n ---@type number\n local result=numberResult\n return result\nend\n";
    let run = |source: &str| {
        fs::write(directory.join("main.lua"), source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .current_dir(&directory)
            .args([
                "check",
                "--format",
                "json",
                "--no-stdlib",
                "--dump-types",
                "--library",
                "types.def.lua",
                "augment.lua",
                "main.lua",
            ])
            .output()
            .unwrap()
    };
    let good = run(source);
    assert_eq!(
        good.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&good.stdout),
        String::from_utf8_lossy(&good.stderr)
    );
    assert!(good.stderr.is_empty());
    assert!(String::from_utf8_lossy(&good.stdout).contains("\"name\":\"numberResult\",\"type\":\"number\""));
    let bad = run(&source.replace(".echo(1)", ".echo('wrong')"));
    assert_eq!(
        bad.status.code(),
        Some(1),
        "{}{}",
        String::from_utf8_lossy(&bad.stdout),
        String::from_utf8_lossy(&bad.stderr)
    );
    assert!(bad.stderr.is_empty());
    assert!(String::from_utf8_lossy(&bad.stdout).contains("TS2345"));
    let corrected = run(source);
    assert_eq!(
        corrected.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&corrected.stdout),
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert!(corrected.stderr.is_empty());
    fs::remove_dir_all(directory).unwrap();
}
