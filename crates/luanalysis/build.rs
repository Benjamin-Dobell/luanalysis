use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let package = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let tests = package.join("tests");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", tests.display());
    // A missing external test tree can later be restored with its old mtimes.
    // In that state also watch the parent entry that records the restoration.
    if !tests.is_dir() {
        println!("cargo:rerun-if-changed={}", package.display());
    }
    let mut roots = Vec::new();
    if tests.is_dir() {
        for entry in fs::read_dir(&tests).expect("read integration test directory") {
            let path = entry.expect("read integration test entry").path();
            let candidate = if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
                Some((
                    path.file_stem()
                        .unwrap()
                        .to_str()
                        .expect("UTF-8 Cargo test name")
                        .to_owned(),
                    path.clone(),
                ))
            } else if path.is_dir() && path.join("main.rs").is_file() {
                Some((
                    path.file_name()
                        .unwrap()
                        .to_str()
                        .expect("UTF-8 Cargo test name")
                        .to_owned(),
                    path.join("main.rs"),
                ))
            } else {
                None
            };
            if let Some(root) = candidate {
                roots.push(root);
            }
        }
    }
    roots.sort();
    let mut modules = String::new();
    let mut manifest = String::new();
    let mut names = HashSet::new();
    let mut targets = HashSet::new();
    for (target, path) in roots {
        assert!(
            targets.insert(target.clone()),
            "duplicate Cargo-style test target {target}"
        );
        let normalized: String = target
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    ch.to_string()
                } else if ch == '-' {
                    "_".to_owned()
                } else {
                    format!("_u{:x}_", ch as u32)
                }
            })
            .collect();
        let base = format!("suite_{normalized}");
        let mut module = base.clone();
        let mut collision = 0;
        while !names.insert(module.clone()) {
            collision += 1;
            module = format!("{base}_{collision}");
        }
        let source = path.to_str().expect("UTF-8 integration source path");
        modules.push_str(&format!("#[path = {source:?}]\nmod {module};\n"));
        manifest.push_str(&format!("{module}\t{target}\t{source}\n"));
    }
    write_if_changed(&output.join("integration_tests.rs"), &modules);
    write_if_changed(&output.join("integration_tests.tsv"), &manifest);
}

fn write_if_changed(path: &Path, content: &str) {
    if fs::read_to_string(path).ok().as_deref() != Some(content) {
        fs::write(path, content).expect("write discovered integration test modules");
    }
}
