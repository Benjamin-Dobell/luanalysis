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

fn method(owner: &str, called: bool, body: &str) -> String {
    format!(
        "{owner}\n---@overload fun():number\n---@overload fun(value:number):number\n---@param value? number|string\n---@return number\nfunction API.bad(value)\n{body}\nend\n{}",
        if called { "API.bad()\n" } else { "" }
    )
}

const OWNERS: [&str; 3] = [
    "---@class API\nlocal API={}",
    "---@shape APIShape\n---@field bad fun(value?:number):number\n---@type APIShape\nlocal API={bad=function() return 0 end}",
    "local API={bad=function() return 0 end}",
];

#[test]
fn overloaded_nominal_shape_and_namespace_private_bodies_are_checked_without_a_call() {
    bad_method_bodies(false);
}

#[test]
fn calling_an_overloaded_method_does_not_hide_its_private_body_errors() {
    bad_method_bodies(true);
}

fn bad_method_bodies(called: bool) {
    for owner in OWNERS {
        let source = method(
            owner,
            called,
            "---@type {x:number}\nlocal state={x=1}\nstate.x='wrong'\nreturn 'bad return'",
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            2,
            "{source}\n{:#?}",
            report.diagnostics
        );
        for literal in ["'wrong'", "'bad return'"] {
            assert!(
                report.diagnostics.iter().any(|error| error.code == "TS2322"
                    && &source[error.span.start..error.span.end] == literal),
                "{literal}\n{source}\n{:#?}",
                report.diagnostics
            );
        }
        clean(&source.replace("'wrong'", "2").replace("'bad return'", "0"));
    }
}

#[test]
fn implementation_parameters_keep_private_annotations_instead_of_public_overload_types() {
    for owner in OWNERS {
        for called in [false, true] {
            let source = method(
                owner,
                called,
                "if value then\n---@type number\nlocal rejected=value\nend\nreturn 0",
            );
            let report = check(&source);
            assert_eq!(
                report.diagnostics.len(),
                1,
                "{source}\n{:#?}",
                report.diagnostics
            );
            assert_eq!(report.diagnostics[0].code, "TS2322");
            assert_eq!(
                &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
                "value"
            );
            clean(&source.replace("---@param value? number|string", "---@param value? number"));
        }
    }
}

#[test]
fn a_selected_raw_callback_without_a_contextual_frame_still_checks_its_body() {
    let source = "---@param callback unknown\nlocal function retain(callback) end\nretain(function()\n---@type number\nlocal invalid='wrong'\nend)";
    let report = check(source);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].code, "TS2322");
    assert_eq!(
        &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
        "'wrong'"
    );
    clean(&source.replace("'wrong'", "1"));
}

#[test]
fn captured_overloaded_method_bodies_keep_their_original_lexical_generic_mapper() {
    let source = "---@generic T\n---@param value T\nlocal function make(value)\n---@class API\nlocal API={}\n---@overload fun():T\n---@overload fun(ignored:number):T\n---@param ignored? number\n---@return T\nfunction API.read(ignored)\n---@type T\nlocal preserved=value\n---@type number\nlocal invalid='wrong'\nreturn preserved\nend\nreturn API\nend\nlocal first=make(1)\nlocal second=make('text')";
    let report = check(source);
    assert!(
        report.diagnostics.iter().any(|error| error.code == "TS2322"
            && &source[error.span.start..error.span.end] == "'wrong'"),
        "{:#?}",
        report.diagnostics
    );
    clean(&source.replace("'wrong'", "1"));
}

#[test]
fn actual_cli_checks_a_selected_private_body_and_its_source_correction() {
    let wrong = method(
        OWNERS[0],
        false,
        "---@type {posX:number}\nlocal state={posX=0}\nif value then state.posX='wrong' end\nreturn 0",
    );
    let directory =
        std::env::temp_dir().join(format!("luanalysis-selected-body-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("original transform.lua");
    std::fs::write(&path, &wrong).unwrap();
    let invoke = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .args(["check", "--lua-version", "5.4"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let rejected = invoke();
    assert_eq!(rejected.status.code(), Some(1));
    let errors = String::from_utf8(rejected.stderr).unwrap();
    assert!(errors.contains("TS2322"), "{errors}");
    let corrected = wrong.replace("state.posX='wrong'", "state.posX=1");
    std::fs::write(&path, corrected).unwrap();
    let accepted = invoke();
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    std::fs::remove_dir_all(directory).unwrap();
}
