use std::{
    fs,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const OVERLOADS: &str = include_str!("../../../examples/features/overloads.lua");
const CLOSURES: &str = include_str!("../../../examples/features/generic-closures.lua");
const DECLARATIONS: &str = include_str!("../../../examples/features/counter.def.lua");
const CONSUMER: &str = include_str!("../../../examples/features/use-counter.lua");
static NEXT: AtomicU64 = AtomicU64::new(0);

fn cli(source: &str, declarations: Option<&str>, legacy: bool) -> Output {
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-feature-example-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("example.lua");
    fs::write(&path, source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"));
    command.args(["check", "--lua-version", "5.4", "--format", "text"]);
    if let Some(declarations) = declarations {
        let library = directory.join("counter.def.lua");
        fs::write(&library, declarations).unwrap();
        command.arg("--library").arg(library);
    }
    if legacy {
        command.arg("--legacy-overloads");
    }
    let output = command.arg(path).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    output
}
fn clean(output: Output) {
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn rejected(output: Output, codes: &[&str]) {
    assert_eq!(output.status.code(), Some(1));
    let errors = String::from_utf8(output.stderr).unwrap();
    assert!(codes.iter().any(|code| errors.contains(code)), "{errors}");
}

#[test]
fn documented_overloads_have_private_bodies_and_explicit_legacy_exposure() {
    clean(cli(OVERLOADS, None, false));
    let private_call = format!("{OVERLOADS}\nlocal privateResult=measure(true)\n");
    rejected(cli(&private_call, None, false), &["TS2345", "TS2769"]);
    clean(cli(&private_call, None, true));
    let bad_body = OVERLOADS.replace("return 0", "return 'wrong'");
    rejected(cli(&bad_body, None, false), &["TS2322"]);
    clean(cli(OVERLOADS, None, false));
}

#[test]
fn documented_generic_closures_keep_independent_result_types() {
    clean(cli(CLOSURES, None, false));
    let wrong = CLOSURES.replace("---@type string", "---@type number");
    rejected(cli(&wrong, None, false), &["TS2322"]);
    clean(cli(CLOSURES, None, false));
}

#[test]
fn documented_bodyless_methods_check_the_consumer_without_stub_bodies() {
    clean(cli(CONSUMER, Some(DECLARATIONS), false));
    let wrong = CONSUMER.replace("copy:add(1)", "copy:add('wrong')");
    rejected(cli(&wrong, Some(DECLARATIONS), false), &["TS2345"]);
    clean(cli(CONSUMER, Some(DECLARATIONS), false));
}
