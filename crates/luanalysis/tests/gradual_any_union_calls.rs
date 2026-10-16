use luanalysis::{CheckReport, check};

const CALL: &str = "---@alias Handler (fun(value:number,flag:boolean):number)|(fun(value:string,flag:boolean):string)\n\n---@param handler Handler\n---@param value any\nlocal function use(handler,value)\nlocal result=handler(value,true)\n---@type number|string\nlocal checked=result\nend\n";

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn one_error(source: &str, code: &str, expression: &str) {
    let report = check(source);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    let error = &report.diagnostics[0];
    assert_eq!(error.code, code);
    assert_eq!(&source[error.span.start..error.span.end], expression);
}

#[test]
fn gradual_any_union_calls_preserve_the_original_return_union() {
    let report = clean(CALL);
    assert_eq!(
        report
            .bindings
            .iter()
            .find(|binding| binding.name == "result")
            .unwrap()
            .ty,
        "string | number"
    );
    let wrong = CALL.replace("---@type number|string", "---@type boolean");
    one_error(&wrong, "TS2322", "result");
    clean(&wrong.replace("---@type boolean", "---@type number|string"));
}

#[test]
fn typed_and_unknown_arguments_do_not_receive_any_compatibility() {
    for ty in ["number", "unknown"] {
        let wrong = CALL.replace("---@param value any", &format!("---@param value {ty}"));
        one_error(&wrong, "TS2345", "value");
        clean(&wrong.replace(&format!("---@param value {ty}"), "---@param value any"));
    }
    let wrong = CALL.replace("handler(value,true)", "handler(1,true)");
    one_error(&wrong, "TS2345", "1");
    clean(&wrong.replace("handler(1,true)", "handler(value,true)"));
    let wrong = CALL.replace(
        "fun(value:string,flag:boolean)",
        "fun(value:never,flag:boolean)",
    );
    one_error(&wrong, "TS2345", "value");
    clean(CALL);
}

#[test]
fn other_parameters_and_physical_arity_remain_checked() {
    let wrong = CALL.replace("handler(value,true)", "handler(value,'wrong')");
    one_error(&wrong, "TS2345", "'wrong'");
    clean(&wrong.replace("handler(value,'wrong')", "handler(value,true)"));
    for call in ["handler(value)", "handler(value,true,false)"] {
        one_error(&CALL.replace("handler(value,true)", call), "TS2554", call);
    }
    clean(CALL);
}

#[test]
fn original_four_handler_dispatch_and_private_callback_need_no_rewrite() {
    use luanalysis::project_check::{
        ProjectCheckOptions, check_project_with_options, static_require_requests,
    };
    use luanalysis::standard_environment::StandardEnvironmentProfile;
    use luanalysis_project::{Project, ProjectConfig, ProjectLoadPlan, Resolver};
    use std::fs;

    // These are the original ge_tts JSON dispatch expression and generator
    // pattern. No typeof/key correlation or source rewrite is needed for any.
    let source = r#"---@shape Dispatcher
---@field boolean fun(value:boolean):void
---@field number fun(value:number):void
---@field string fun(value:string):void
---@field table fun(value:table):void

---@param dispatcher Dispatcher
---@param value any
local function defaultencode(dispatcher,value)
    return dispatcher[--[[---@not 'nil' | 'function' | 'thread' | 'userdata']] type(value)](value)
end

---@alias Generate fun(dispatcher:Dispatcher):fun(value:any):void

---@type Generate
local generate=function(dispatcher)
    return function(value)
        dispatcher[--[[---@not 'nil' | 'function' | 'thread' | 'userdata']] type(value)](value)
    end
end
"#;
    // The original dispatcher relies on the selected Lua profile's finite
    // type-name union. check() only supplies the legacy type():string fallback.
    let directory = std::env::temp_dir().join(format!(
        "luanalysis Any union profile {}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("main.lua");
    let check_profile = |source: &str| {
        fs::write(&file, source).unwrap();
        let resolver = Resolver::new(ProjectConfig::new([directory.clone()])).unwrap();
        let project = Project::load_plan(
            &resolver,
            ProjectLoadPlan::new([file.clone()]),
            static_require_requests,
        );
        let report = check_project_with_options(
            project,
            ProjectCheckOptions {
                jobs: 1,
                environment: StandardEnvironmentProfile::Lua52,
                ..Default::default()
            },
        );
        assert!(
            report.environment_errors.is_empty(),
            "{:#?}",
            report.environment_errors
        );
        assert!(
            report.project.entry_errors.is_empty(),
            "{:#?}",
            report.project.entry_errors
        );
        assert!(
            report.project.library_errors.is_empty(),
            "{:#?}",
            report.project.library_errors
        );
        assert!(
            report.project.role_conflicts.is_empty(),
            "{:#?}",
            report.project.role_conflicts
        );
        let entry = report.project.entries[0];
        for file in &report.files {
            if file.file != entry {
                assert!(file.report.diagnostics.is_empty(), "{file:#?}");
            }
        }
        report.file(entry).unwrap().report.clone()
    };
    let original = check_profile(source);
    assert!(
        original.diagnostics.is_empty(),
        "{:#?}",
        original.diagnostics
    );
    let wrong = source.replace("---@param value any", "---@param value unknown");
    let rejected = check_profile(&wrong);
    assert_eq!(rejected.diagnostics.len(), 1, "{:#?}", rejected.diagnostics);
    let error = &rejected.diagnostics[0];
    assert_eq!(error.code, "TS2345");
    assert_eq!(&wrong[error.span.start..error.span.end], "value");
    let corrected = check_profile(source);
    assert!(
        corrected.diagnostics.is_empty(),
        "{:#?}",
        corrected.diagnostics
    );
    fs::remove_dir_all(&directory).unwrap();
}
