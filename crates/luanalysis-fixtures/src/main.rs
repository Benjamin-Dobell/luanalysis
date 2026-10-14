use luanalysis_fixtures::{Severity, import};
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(message) = run() {
        eprintln!("luanalysis-fixtures: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        return Err(usage().into());
    };
    if command == "--help" || command == "-h" {
        println!("{}", usage());
        return Ok(());
    }
    let input = args.next().ok_or_else(|| usage().to_string())?;
    if args.next().is_some() {
        return Err(usage().into());
    }
    let input = PathBuf::from(input);
    if command == "import" {
        let marked =
            fs::read_to_string(&input).map_err(|error| format!("{}: {error}", input.display()))?;
        let fixture = import(&marked).map_err(|error| format!("{}: {error}", input.display()))?;
        print!("{}", fixture.to_json());
    } else if command == "inventory" {
        let mut files = Vec::new();
        collect(&input, &mut files)?;
        files.sort();
        let mut totals = [0usize; 4];
        for file in &files {
            let marked =
                fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))?;
            let fixture =
                import(&marked).map_err(|error| format!("{}: {error}", file.display()))?;
            let mut counts = [0usize; 4];
            for diagnostic in &fixture.diagnostics {
                counts[match diagnostic.severity {
                    Severity::Error => 0,
                    Severity::Warning => 1,
                    Severity::WeakWarning => 2,
                    Severity::Info => 3,
                }] += 1;
            }
            for (total, count) in totals.iter_mut().zip(counts) {
                *total += count;
            }
            println!(
                "{}\t{}\t{}\t{}\t{}",
                file.display(),
                counts[0],
                counts[1],
                counts[2],
                counts[3]
            );
        }
        println!(
            "Imported {} files: {} error, {} warning, {} weak_warning, {} info markers. No type checking performed.",
            files.len(),
            totals[0],
            totals[1],
            totals[2],
            totals[3]
        );
    } else {
        return Err(usage().into());
    }
    Ok(())
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            collect(&entry.path(), files)?;
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "lua")
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn usage() -> &'static str {
    "Usage: luanalysis-fixtures import <marked.lua>\n       luanalysis-fixtures inventory <fixture-directory>\n\nImport emits clean source and exact diagnostic expectations as JSON.\nInventory validates all .lua marker formats; it does not run a type checker."
}
