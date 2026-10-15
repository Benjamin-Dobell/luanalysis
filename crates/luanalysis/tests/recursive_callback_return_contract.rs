use std::{fs, process::Command};

// The recursive helper has a written return contract, while its coroutine and
// map callbacks infer returns. Reading that contract must not infer it again
// through both callbacks and the descendants initializer.
const SOURCE: &str = r#"---@generic T,R
---@param values T[]
---@param callback fun(value:T):R
---@return R[]
local function map(values, callback)
    ---@type R[]
    local result={}
    for _, value in ipairs(values) do result[#result+1]=callback(value) end
    return result
end
---@generic T
---@param node T
---@param getChildren fun(node:T):nil|T[]
---@param visitCallback fun(node:T)
---@return thread
local function visit(node,getChildren,visitCallback)
    return coroutine.create(function()
        local children=getChildren(node)
        if children then
            local descendants=map(children,function(child)
                return visit(child,getChildren,visitCallback)
            end)
            for _, descendant in ipairs(descendants) do
                coroutine.resume(descendant)
            end
        end
    end)
end
"#;

fn diagnostic_range(source: &str, expression: &str) -> String {
    let start = source.find(expression).unwrap();
    format!(
        "\"range\":{{\"start\":{start},\"end\":{}}}",
        start + expression.len()
    )
}

#[test]
fn recursive_fixed_return_contract_preserves_callback_and_implementation_checking() {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis recursive callback return {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    let run = |source: &str, version: &str, exit: i32| {
        fs::write(&file, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--format", "json", "--lua-version", version])
            .arg(&file)
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(exit), "{text}\n{stderr}");
        assert!(stderr.is_empty(), "{stderr}");
        assert!(
            !text.contains("TS7023") && !text.contains("TS7024"),
            "{text}"
        );
        assert!(
            !text.contains("LUA2041") && !text.contains("TS2589"),
            "{text}"
        );
        text
    };
    for version in ["5.2", "5.4"] {
        let positive = run(SOURCE, version, 0);
        assert!(positive.contains("\"diagnostics\":[]"), "{positive}");

        let wrong_body = SOURCE.replace("coroutine.resume(descendant)", "coroutine.resume(42)");
        let rejected = run(&wrong_body, version, 1);
        assert!(rejected.contains("\"code\":\"TS2345\""), "{rejected}");
        assert!(
            rejected.contains(&diagnostic_range(&wrong_body, "42")),
            "{rejected}"
        );
        run(SOURCE, version, 0);

        let wrong_return = SOURCE
            .replace(
                "return coroutine.create(function()",
                "local created=coroutine.create(function()",
            )
            .replace("    end)\nend\n", "    end)\n    return 42\nend\n");
        let rejected = run(&wrong_return, version, 1);
        assert!(rejected.contains("\"code\":\"TS2322\""), "{rejected}");
        assert!(
            rejected.contains(&diagnostic_range(&wrong_return, "42")),
            "{rejected}"
        );
        run(SOURCE, version, 0);
    }
    fs::remove_dir_all(directory).unwrap();
}
