use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

#[test]
fn runtime_expando_candidates_never_execute_opaque_declaration_bodies() {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let directory = std::env::temp_dir().join(format!(
        "luanalysis-runtime-opaque-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("api.def.lua"),"Api={}\n---@return number\nfunction Api.inspect()\n local hidden=runtime_only_constructor()\n hidden.field=runtime_only_value()\n return runtime_only_return()\nend\n").unwrap();
    fs::write(
        directory.join("main.lua"),
        "local value={}\nvalue.count=1\n---@type number\nlocal count=value.count\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .current_dir(&directory)
        .args([
            "check",
            "--no-stdlib",
            "--library",
            "api.def.lua",
            "--dump-types",
            "main.lua",
        ])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(text.contains("count: number"), "{text}");
    fs::write(
        directory.join("main.lua"),
        "local value={}\nvalue.count=1\n---@type string\nlocal count=value.count\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
        .current_dir(&directory)
        .args([
            "check",
            "--no-stdlib",
            "--library",
            "api.def.lua",
            "main.lua",
        ])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("main.lua:4:") && text.contains("TS2322"),
        "{text}"
    );
    assert!(!text.contains("runtime_only"), "{text}");
    fs::remove_dir_all(directory).unwrap();
}
