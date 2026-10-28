use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let package = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    version(&package);
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

/// The version every interface reports, `LUANALYSIS_VERSION`: the package's, with the commit it
/// was built from as semantic-version build metadata (`0.1.0+b2cff754f241`), and `.dirty` when
/// the sources it is built from differ from that commit. The commit is git's, else
/// `LUANALYSIS_COMMIT`'s or GitHub Actions' `GITHUB_SHA`; without one it is the package version.
fn version(package: &Path) {
    // The sources a build is made from; changes elsewhere, such as documentation, leave it clean.
    const SOURCES: [&str; 5] = ["crates", "vendor", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml"];
    println!("cargo:rerun-if-env-changed=LUANALYSIS_COMMIT");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    let git = |arguments: &[&str]| {
        let output = Command::new("git").arg("-C").arg(package).args(arguments).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let mut metadata = None;
    if let (Some(commit), Some(root), Some(git_dir), Some(common_dir)) = (
        git(&["rev-parse", "HEAD"]),
        git(&["rev-parse", "--show-toplevel"]),
        git(&["rev-parse", "--absolute-git-dir"]),
        git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
    ) {
        // A new commit moves HEAD or the branch it names, and is logged; an edit changes a source.
        // A path that does not exist would run this again for every build.
        let (git_dir, common) = (Path::new(&git_dir), Path::new(&common_dir));
        let mut watched = vec![git_dir.join("HEAD"), git_dir.join("index"), git_dir.join("logs").join("HEAD"), common.join("packed-refs")];
        watched.extend(git(&["symbolic-ref", "-q", "HEAD"]).map(|branch| common.join(branch)));
        watched.extend(SOURCES.map(|source| Path::new(&root).join(source)));
        for path in watched.iter().filter(|path| path.exists()) {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        let mut status = vec!["-C".to_owned(), root.clone(), "status".to_owned(), "--porcelain".to_owned(), "--".to_owned()];
        status.extend(SOURCES.map(str::to_owned));
        let dirty = git(&status.iter().map(String::as_str).collect::<Vec<_>>()).is_none_or(|changes| !changes.is_empty());
        metadata = Some(format!("{}{}", &commit[..commit.len().min(12)], if dirty { ".dirty" } else { "" }));
    }
    let metadata = metadata.or_else(|| {
        ["LUANALYSIS_COMMIT", "GITHUB_SHA"].into_iter().find_map(|name| env::var(name).ok())
            .filter(|commit| !commit.is_empty() && commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .map(|commit| commit[..commit.len().min(12)].to_owned())
    });
    let package_version = env::var("CARGO_PKG_VERSION").unwrap();
    let version = match metadata {
        Some(metadata) => format!("{package_version}+{metadata}"),
        None => package_version,
    };
    println!("cargo:rustc-env=LUANALYSIS_VERSION={version}");
}
