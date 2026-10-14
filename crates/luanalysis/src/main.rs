use std::fs;
use std::path::PathBuf;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        help();
        return 2;
    };
    if command == "--help" || command == "-h" {
        help();
        return 0;
    }
    if command == "--version" || command == "-V" {
        println!(
            "luanalysis {} (Bun core {})",
            env!("CARGO_PKG_VERSION"),
            luanalysis_engine::BUN_REVISION
        );
        return 0;
    }
    if command != "check" {
        eprintln!("luanalysis: unknown command {:?}", command);
        help();
        return 2;
    }
    let mut dump = false;
    let mut files = Vec::new();
    let mut positional = false;
    for arg in args {
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if !positional && arg == "--dump-types" {
            dump = true;
            continue;
        }
        if !positional && (arg == "--help" || arg == "-h") {
            help();
            return 0;
        }
        if !positional && arg.to_string_lossy().starts_with('-') {
            eprintln!("luanalysis: unknown option {:?}", arg);
            return 2;
        }
        files.push(PathBuf::from(arg));
    }
    if files.is_empty() {
        eprintln!("luanalysis: check requires at least one Lua file");
        return 2;
    }
    let mut errors = 0;
    let mut io_errors = false;
    for path in &files {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{}: error[LUA0001]: {error}", path.display());
                io_errors = true;
                continue;
            }
        };
        let report = luanalysis::check(&source);
        for error in &report.diagnostics {
            let (line, column) = luanalysis::location(&source, error.span);
            eprintln!(
                "{}:{line}:{column}: error[{}]: {}",
                path.display(),
                error.code,
                error.message
            );
        }
        errors += report.diagnostics.len();
        if dump {
            for binding in &report.bindings {
                let (line, column) = luanalysis::location(&source, binding.span);
                println!(
                    "{}:{line}:{column}: {}: {}",
                    path.display(),
                    binding.name,
                    binding.ty
                );
            }
        }
    }
    if io_errors {
        2
    } else if errors > 0 {
        1
    } else {
        println!("No type errors in {} file(s).", files.len());
        0
    }
}

fn help() {
    println!(
        "luanalysis check [--dump-types] <file.lua>...\n\nNative Lua/Luanalysis checking slice backed by the Bun Rust type core.\nFunctions, calls, control flow and project loading are not implemented yet."
    );
}
