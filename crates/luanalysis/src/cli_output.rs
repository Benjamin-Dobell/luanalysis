//! CLI transport over owned original-source reports. No checker IDs escape.
use luanalysis::{location, project_check::ProjectCheckReport};
use luanalysis_syntax::Span;
use std::{collections::HashMap, ffi::OsString, fmt::Write, path::PathBuf};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}
impl OutputFormat {
    // Discover the output request before reporting argument errors. Values of
    // other options and filenames after `--` cannot select an output format.
    pub fn requested(args: &[OsString]) -> Self {
        let mut format = Self::Text;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if arg == "--" {
                break;
            }
            if arg == "--format" {
                if let Some(value) = args.next() {
                    if value == "json" {
                        format = Self::Json;
                    } else if value == "text" {
                        format = Self::Text;
                    }
                }
            } else if matches!(
                arg.to_str(),
                Some("--library" | "--module-root" | "--package-path")
            ) {
                args.next();
            }
        }
        format
    }
}

pub struct HostDiagnostic {
    pub path: Option<PathBuf>,
    pub code: &'static str,
    pub message: String,
}
impl HostDiagnostic {
    pub fn new(path: impl Into<PathBuf>, code: &'static str, message: impl ToString) -> Self {
        Self {
            path: Some(path.into()),
            code,
            message: message.to_string(),
        }
    }
    pub fn general(code: &'static str, message: impl ToString) -> Self {
        Self {
            path: None,
            code,
            message: message.to_string(),
        }
    }
}
struct Diagnostic {
    host: HostDiagnostic,
    span: Option<Span>,
    position: Option<(usize, usize)>,
}
struct Binding {
    path: PathBuf,
    name: String,
    ty: String,
    span: Span,
    position: (usize, usize),
}
pub struct CliReport {
    pub exit_code: i32,
    pub files_checked: usize,
    diagnostics: Vec<Diagnostic>,
    bindings: Vec<Binding>,
}
impl CliReport {
    pub fn failure(error: HostDiagnostic) -> Self {
        Self {
            exit_code: 2,
            files_checked: 0,
            diagnostics: vec![Diagnostic {
                host: error,
                span: None,
                position: None,
            }],
            bindings: Vec::new(),
        }
    }
    pub fn from_project(
        result: &ProjectCheckReport,
        paths: &HashMap<PathBuf, PathBuf>,
        mut errors: Vec<HostDiagnostic>,
        dump: bool,
    ) -> Self {
        for error in result
            .project
            .entry_errors
            .iter()
            .chain(&result.project.library_errors)
        {
            errors.push(HostDiagnostic::new(&error.path, "LUA0001", &error.message));
        }
        for conflict in &result.project.role_conflicts {
            errors.push(HostDiagnostic::new(
                &conflict.requested_path,
                "LUA0002",
                conflict,
            ));
        }
        let exit_code = if !errors.is_empty() {
            2
        } else if result.has_errors() {
            1
        } else {
            0
        };
        let mut diagnostics: Vec<_> = errors
            .into_iter()
            .map(|host| Diagnostic {
                host,
                span: None,
                position: None,
            })
            .collect();
        let mut bindings = Vec::new();
        for file in &result.files {
            let source = result
                .source(file.file)
                .expect("report source belongs to this project");
            let path = paths.get(&source.path).unwrap_or(&source.path);
            diagnostics.extend(file.report.diagnostics.iter().map(|error| Diagnostic {
                host: HostDiagnostic::new(path, error.code, &error.message),
                span: Some(error.span),
                position: Some(location(&source.source, error.span)),
            }));
            if dump {
                bindings.extend(file.report.bindings.iter().map(|binding| Binding {
                    path: path.clone(),
                    name: binding.name.clone(),
                    ty: binding.ty.clone(),
                    span: binding.span,
                    position: location(&source.source, binding.span),
                }));
            }
        }
        Self {
            exit_code,
            files_checked: result.project.files.len(),
            diagnostics,
            bindings,
        }
    }
    pub fn print(&self, format: OutputFormat) {
        if format == OutputFormat::Json {
            println!("{}", self.to_json());
            return;
        }
        for diagnostic in &self.diagnostics {
            let host = &diagnostic.host;
            if host.code == "LUA0000" {
                eprintln!("luanalysis: {}", host.message);
            } else if let Some(path) = &host.path {
                if let Some((line, column)) = diagnostic.position {
                    eprintln!(
                        "{}:{line}:{column}: error[{}]: {}",
                        path.display(),
                        host.code,
                        host.message
                    );
                } else {
                    eprintln!("{}: error[{}]: {}", path.display(), host.code, host.message);
                }
            } else {
                eprintln!("luanalysis: error[{}]: {}", host.code, host.message);
            }
        }
        for binding in &self.bindings {
            let (line, column) = binding.position;
            println!(
                "{}:{line}:{column}: {}: {}",
                binding.path.display(),
                binding.name,
                binding.ty
            );
        }
        if self.exit_code == 0 {
            println!("No type errors in {} file(s).", self.files_checked);
        }
    }
    fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"formatVersion\":1,\"success\":{},\"exitCode\":{},\"filesChecked\":{},\"rangeEncoding\":\"utf-8-bytes\",\"positionEncoding\":\"unicode-scalars-1-based\",\"diagnostics\":[",
            self.exit_code == 0,
            self.exit_code,
            self.files_checked
        );
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index != 0 {
                json.push(',');
            }
            json.push_str("{\"path\":");
            if let Some(path) = &diagnostic.host.path {
                string(&mut json, &path.to_string_lossy());
            } else {
                json.push_str("null");
            }
            json.push_str(",\"severity\":\"error\",\"code\":");
            string(&mut json, diagnostic.host.code);
            json.push_str(",\"message\":");
            string(&mut json, &diagnostic.host.message);
            range(&mut json, diagnostic.span, diagnostic.position);
            json.push('}');
        }
        json.push_str("],\"bindings\":[");
        for (index, binding) in self.bindings.iter().enumerate() {
            if index != 0 {
                json.push(',');
            }
            json.push_str("{\"path\":");
            string(&mut json, &binding.path.to_string_lossy());
            json.push_str(",\"name\":");
            string(&mut json, &binding.name);
            json.push_str(",\"type\":");
            string(&mut json, &binding.ty);
            range(&mut json, Some(binding.span), Some(binding.position));
            json.push('}');
        }
        json.push_str("]}");
        json
    }
}
fn range(json: &mut String, span: Option<Span>, position: Option<(usize, usize)>) {
    if let Some(span) = span {
        write!(
            json,
            ",\"range\":{{\"start\":{},\"end\":{}}}",
            span.start, span.end
        )
        .unwrap();
    } else {
        json.push_str(",\"range\":null");
    }
    if let Some((line, column)) = position {
        write!(
            json,
            ",\"position\":{{\"line\":{line},\"column\":{column}}}"
        )
        .unwrap();
    } else {
        json.push_str(",\"position\":null");
    }
}
fn string(json: &mut String, value: &str) {
    json.push('"');
    for ch in value.chars() {
        match ch {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            ch if ch <= '\u{1f}' => {
                write!(json, "\\u{:04x}", ch as u32).unwrap();
            }
            ch => json.push(ch),
        }
    }
    json.push('"');
}
