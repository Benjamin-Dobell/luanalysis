//! `luanalysis lsp`: a language server over stdio. Each revision of the editor's buffers is
//! checked as one complete project, as `check` checks it; hover and go-to-definition answer from
//! the last completed check. Checks run on their own thread and coalesce: requests are answered
//! while a check runs, and edits made meanwhile are checked next.

use crate::CheckInputs;
use crate::lsp_json::Json;
use luanalysis::incremental::ProjectWorkspace;
use luanalysis::project_check::{ProjectCheckOptions, ProjectCheckReport};
use luanalysis_project::{DependencyOutcome, Project, SourceOrigin};
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// LSP error codes.
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_REQUEST: i64 = -32600;
const SERVER_NOT_INITIALIZED: i64 = -32002;

pub(crate) fn run(inputs: CheckInputs, mut options: ProjectCheckOptions) -> i32 {
    // Hover reads the described types of the last check.
    options.describe_types = true;
    let (events, received) = mpsc::channel();
    let reader = events.clone();
    std::thread::spawn(move || {
        let mut input = BufReader::new(io::stdin().lock());
        loop {
            match read_message(&mut input) {
                Ok(Some(text)) => match Json::parse(&text) {
                    Ok(message) => {
                        if reader.send(Event::Message(message)).is_err() { return; }
                    }
                    Err(error) => eprintln!("luanalysis lsp: ignoring a malformed message: {error}"),
                },
                Ok(None) | Err(_) => {
                    let _ = reader.send(Event::Closed);
                    return;
                }
            }
        }
    });
    Server::new(inputs, options, events).serve(received)
}

fn read_message(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("Content-Length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(length) = length else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a message without Content-Length"));
    };
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    String::from_utf8(body).map(Some).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

enum Event {
    Message(Json),
    Closed,
    Checked(Box<Snapshot>),
}

struct Job {
    overlays: HashMap<PathBuf, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16,
}

struct Document {
    uri: String,
    text: String,
    version: i64,
}

struct Server {
    inputs: Option<CheckInputs>,
    options: ProjectCheckOptions,
    events: mpsc::Sender<Event>,
    jobs: Option<mpsc::Sender<Job>>,
    encoding: Encoding,
    initialized: bool,
    shutdown: bool,
    documents: HashMap<PathBuf, Document>,
    /// Advanced by every change to what a check reads.
    revision: u64,
    /// The revision of the last job sent to the checker.
    requested: Option<u64>,
    checking: bool,
    snapshot: Option<Snapshot>,
    published: HashSet<PathBuf>,
}

impl Server {
    fn new(inputs: CheckInputs, options: ProjectCheckOptions, events: mpsc::Sender<Event>) -> Self {
        Self {
            inputs: Some(inputs),
            options,
            events,
            jobs: None,
            encoding: Encoding::Utf16,
            initialized: false,
            shutdown: false,
            documents: HashMap::new(),
            revision: 0,
            requested: None,
            checking: false,
            snapshot: None,
            published: HashSet::new(),
        }
    }

    fn serve(&mut self, received: mpsc::Receiver<Event>) -> i32 {
        while let Ok(event) = received.recv() {
            let mut next = Some(event);
            // Handle everything already queued before deciding whether to check.
            while let Some(event) = next {
                match event {
                    Event::Closed => return if self.shutdown { 0 } else { 1 },
                    Event::Message(message) => {
                        if let Some(code) = self.message(message) { return code; }
                    }
                    Event::Checked(snapshot) => self.checked(*snapshot),
                }
                next = received.try_recv().ok();
            }
            self.schedule();
        }
        1
    }

    fn schedule(&mut self) {
        if self.checking || self.revision == 0 || self.requested == Some(self.revision) { return; }
        let Some(jobs) = &self.jobs else { return; };
        let overlays = self.documents.iter().map(|(path, document)| (path.clone(), document.text.clone())).collect();
        if jobs.send(Job { overlays }).is_ok() {
            self.requested = Some(self.revision);
            self.checking = true;
        }
    }

    fn message(&mut self, message: Json) -> Option<i32> {
        let method = message.get("method").and_then(Json::as_str).map(str::to_owned);
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        match (method, message.get("id").cloned()) {
            (Some(method), Some(id)) => self.request(&method, id, &params),
            (Some(method), None) => return self.notification(&method, &params),
            // A response to a request of ours: there are none.
            (None, _) => {}
        }
        None
    }

    fn request(&mut self, method: &str, id: Json, params: &Json) {
        if method == "initialize" {
            let result = self.initialize(params);
            self.respond(id, result);
            return;
        }
        if !self.initialized {
            return self.respond_error(id, SERVER_NOT_INITIALIZED, "the server is not initialized");
        }
        if self.shutdown {
            return self.respond_error(id, INVALID_REQUEST, "the server is shut down");
        }
        match method {
            "shutdown" => {
                self.shutdown = true;
                self.respond(id, Json::Null);
            }
            "textDocument/hover" => {
                let result = self.hover(params);
                self.respond(id, result);
            }
            "textDocument/definition" => {
                let result = self.definition(params);
                self.respond(id, result);
            }
            "textDocument/references" => {
                let declaration = params.path(&["context", "includeDeclaration"]) == Some(&Json::Bool(true));
                let result = self.occurrences(params, declaration, |path, file, start, end| Json::object([
                    ("uri", Json::from(path_to_uri(path))),
                    ("range", file.range(start, end, self.encoding)),
                ]));
                self.respond(id, result);
            }
            "textDocument/documentHighlight" => {
                let result = self.occurrences(params, true, |_, file, start, end| Json::object([
                    ("range", file.range(start, end, self.encoding)),
                ]));
                self.respond(id, result);
            }
            _ => self.respond_error(id, METHOD_NOT_FOUND, &format!("unsupported request {method}")),
        }
    }

    fn notification(&mut self, method: &str, params: &Json) -> Option<i32> {
        match method {
            "exit" => return Some(if self.shutdown { 0 } else { 1 }),
            "initialized" => self.revision += 1,
            "textDocument/didOpen" => {
                let document = params.get("textDocument");
                let uri = document.and_then(|document| document.get("uri")).and_then(Json::as_str);
                let text = document.and_then(|document| document.get("text")).and_then(Json::as_str);
                if let (Some(uri), Some(text)) = (uri, text) && let Some(path) = uri_to_path(uri) {
                    let version = document.and_then(|document| document.get("version")).and_then(Json::as_u64).unwrap_or(0) as i64;
                    self.documents.insert(path, Document { uri: uri.to_owned(), text: text.to_owned(), version });
                    self.revision += 1;
                }
            }
            "textDocument/didChange" => {
                let uri = params.path(&["textDocument", "uri"]).and_then(Json::as_str);
                let version = params.path(&["textDocument", "version"]).and_then(Json::as_u64);
                // Full synchronization: the last change holds the whole text.
                let text = params.get("contentChanges").and_then(Json::as_array)
                    .and_then(|changes| changes.last()).and_then(|change| change.get("text")).and_then(Json::as_str);
                if let (Some(uri), Some(text)) = (uri, text) && let Some(path) = uri_to_path(uri) {
                    let document = self.documents.entry(path).or_insert_with(|| Document { uri: uri.to_owned(), text: String::new(), version: 0 });
                    document.text = text.to_owned();
                    if let Some(version) = version { document.version = version as i64; }
                    self.revision += 1;
                }
            }
            "textDocument/didClose" => {
                if let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path)
                    && self.documents.remove(&path).is_some()
                {
                    self.revision += 1;
                }
            }
            "textDocument/didSave" | "workspace/didChangeWatchedFiles" => self.revision += 1,
            _ => {}
        }
        None
    }

    fn initialize(&mut self, params: &Json) -> Json {
        let encodings = params.path(&["capabilities", "general", "positionEncodings"]).and_then(Json::as_array).unwrap_or(&[]);
        self.encoding = if encodings.iter().any(|encoding| encoding.as_str() == Some("utf-8")) { Encoding::Utf8 } else { Encoding::Utf16 };
        if let Some(mut inputs) = self.inputs.take() {
            if inputs.files.is_empty() {
                let root = params.get("rootUri").and_then(Json::as_str)
                    .or_else(|| params.get("workspaceFolders").and_then(Json::as_array)
                        .and_then(|folders| folders.first()).and_then(|folder| folder.get("uri")).and_then(Json::as_str))
                    .and_then(uri_to_path)
                    .or_else(|| params.get("rootPath").and_then(Json::as_str).map(PathBuf::from))
                    .or_else(|| std::env::current_dir().ok());
                inputs.files.extend(root);
            }
            let (jobs, received) = mpsc::channel();
            let events = self.events.clone();
            let options = self.options;
            std::thread::Builder::new()
                .name("luanalysis-lsp-check".into())
                .spawn(move || check_jobs(inputs, options, received, events))
                .expect("start the language server's checking thread");
            self.jobs = Some(jobs);
        }
        self.initialized = true;
        Json::object([
            ("capabilities", Json::object([
                ("positionEncoding", Json::from(match self.encoding { Encoding::Utf8 => "utf-8", Encoding::Utf16 => "utf-16" })),
                ("textDocumentSync", Json::object([
                    ("openClose", Json::from(true)),
                    ("change", Json::from(1usize)),
                    ("save", Json::object([("includeText", Json::from(false))])),
                ])),
                ("hoverProvider", Json::from(true)),
                ("definitionProvider", Json::from(true)),
                ("referencesProvider", Json::from(true)),
                ("documentHighlightProvider", Json::from(true)),
            ])),
            ("serverInfo", Json::object([
                ("name", Json::from("luanalysis")),
                ("version", Json::from(env!("CARGO_PKG_VERSION"))),
            ])),
        ])
    }

    fn checked(&mut self, snapshot: Snapshot) {
        self.checking = false;
        if let Some(error) = &snapshot.error {
            self.notify("window/logMessage", Json::object([
                ("type", Json::from(1usize)),
                ("message", Json::from(format!("luanalysis: {error}"))),
            ]));
        }
        let mut published = HashSet::new();
        for (path, file) in &snapshot.files {
            // An open document always receives the result of the check, even an empty one.
            if file.diagnostics.is_empty() && !self.documents.contains_key(path) { continue; }
            published.insert(path.clone());
            let diagnostics = file.diagnostics.iter().map(|diagnostic| Json::object([
                ("range", file.range(diagnostic.start, diagnostic.end, self.encoding)),
                ("severity", Json::from(1usize)),
                ("code", Json::from(diagnostic.code)),
                ("source", Json::from("luanalysis")),
                ("message", Json::from(diagnostic.message.as_str())),
            ])).collect();
            self.publish(path, Json::Array(diagnostics), &file.text);
        }
        for path in self.published.difference(&published).cloned().collect::<Vec<_>>() {
            self.publish(&path, Json::Array(Vec::new()), "");
        }
        self.published = published;
        self.snapshot = Some(snapshot);
    }

    fn publish(&mut self, path: &Path, diagnostics: Json, checked: &str) {
        let document = self.documents.get(path);
        let uri = document.map_or_else(|| path_to_uri(path), |document| document.uri.clone());
        let mut params = vec![("uri".to_owned(), Json::from(uri)), ("diagnostics".to_owned(), diagnostics)];
        if let Some(document) = document.filter(|document| document.text == checked) {
            params.push(("version".to_owned(), Json::from(document.version)));
        }
        self.notify("textDocument/publishDiagnostics", Json::Object(params));
    }

    /// The checked file under a request's text document, and the offset of its position.
    fn file_at(&self, params: &Json) -> Option<(&Path, &CheckedFile, usize)> {
        let path = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path)?;
        let (path, file) = self.snapshot.as_ref()?.files.get_key_value(&path)?;
        let line = params.path(&["position", "line"]).and_then(Json::as_u64)? as usize;
        let character = params.path(&["position", "character"]).and_then(Json::as_u64)? as usize;
        Some((path, file, file.offset(line, character, self.encoding)))
    }

    fn hover(&self, params: &Json) -> Json {
        let Some((_, file, offset)) = self.file_at(params) else { return Json::Null };
        // The narrowest binding or expression that contains the position.
        let mut best: Option<(usize, usize, String)> = None;
        let mut consider = |start: usize, end: usize, text: String| {
            if start <= offset && offset <= end && best.as_ref().is_none_or(|(s, e, _)| end - start < e - s) {
                best = Some((start, end, text));
            }
        };
        for binding in &file.bindings {
            consider(binding.start, binding.end, format!("{}: {}", binding.name, binding.ty));
        }
        for ty in &file.types {
            consider(ty.start, ty.end, ty.ty.clone());
        }
        let Some((start, end, text)) = best else { return Json::Null };
        Json::object([
            ("contents", Json::object([
                ("kind", Json::from("markdown")),
                ("value", Json::from(format!("```lua\n{text}\n```"))),
            ])),
            ("range", file.range(start, end, self.encoding)),
        ])
    }

    fn definition(&self, params: &Json) -> Json {
        let Some((path, file, offset)) = self.file_at(params) else { return Json::Null };
        if let Some(occurrence) = file.occurrences.iter().find(|occurrence| occurrence.start <= offset && offset <= occurrence.end) {
            return Json::object([
                ("uri", Json::from(path_to_uri(path))),
                ("range", file.range(occurrence.declaration.0, occurrence.declaration.1, self.encoding)),
            ]);
        }
        if let Some(require) = file.requires.iter().find(|require| require.start <= offset && offset <= require.end) {
            let start = Json::object([("line", Json::from(0usize)), ("character", Json::from(0usize))]);
            return Json::object([
                ("uri", Json::from(path_to_uri(&require.target))),
                ("range", Json::object([("start", start.clone()), ("end", start)])),
            ]);
        }
        Json::Null
    }

    /// Every occurrence in the file of the local under the position, the declaration only when
    /// `declaration` is set. Locals are lexical, so all their occurrences are in this file.
    fn occurrences(&self, params: &Json, declaration: bool, item: impl Fn(&Path, &CheckedFile, usize, usize) -> Json) -> Json {
        let Some((path, file, offset)) = self.file_at(params) else { return Json::Null };
        let Some(target) = file.occurrences.iter()
            .find(|occurrence| occurrence.start <= offset && offset <= occurrence.end)
            .map(|occurrence| occurrence.declaration) else { return Json::Null };
        Json::Array(file.occurrences.iter()
            .filter(|occurrence| occurrence.declaration == target)
            .filter(|occurrence| declaration || (occurrence.start, occurrence.end) != target)
            .map(|occurrence| item(path, file, occurrence.start, occurrence.end))
            .collect())
    }

    fn respond(&self, id: Json, result: Json) {
        write_message(&Json::object([("jsonrpc", Json::from("2.0")), ("id", id), ("result", result)]));
    }

    fn respond_error(&self, id: Json, code: i64, message: &str) {
        write_message(&Json::object([
            ("jsonrpc", Json::from("2.0")),
            ("id", id),
            ("error", Json::object([("code", Json::from(code)), ("message", Json::from(message))])),
        ]));
    }

    fn notify(&self, method: &str, params: Json) {
        write_message(&Json::object([("jsonrpc", Json::from("2.0")), ("method", Json::from(method)), ("params", params)]));
    }
}

fn write_message(message: &Json) {
    let body = message.to_string();
    let mut out = io::stdout().lock();
    // A closed client ends the server through its input.
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

fn check_jobs(inputs: CheckInputs, options: ProjectCheckOptions, jobs: mpsc::Receiver<Job>, events: mpsc::Sender<Event>) {
    let mut workspace = ProjectWorkspace::new();
    while let Ok(mut job) = jobs.recv() {
        // Only the latest revision is worth checking.
        while let Ok(newer) = jobs.try_recv() { job = newer; }
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&inputs, options, &mut workspace, &job)));
        let snapshot = match checked {
            Ok(Ok(report)) => Snapshot::of(&report),
            Ok(Err(error)) => Snapshot::failed(error),
            Err(_) => {
                workspace = ProjectWorkspace::new();
                Snapshot::failed("checking failed internally; see the server's standard error".into())
            }
        };
        if events.send(Event::Checked(Box::new(snapshot))).is_err() { return; }
    }
}

fn check(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, job: &Job) -> Result<ProjectCheckReport, String> {
    let discovery = inputs.discover().map_err(|error| error.to_string())?;
    let resolver = inputs.resolver(&discovery).map_err(|error| error.to_string())?;
    let project = Project::load_plan_with_overlays(&resolver, discovery.plan.clone(), &job.overlays, |source| {
        workspace.static_require_requests(source)
    });
    Ok(workspace.check_project(project, options))
}

/// What hover, definition and diagnostics read from one completed check.
struct Snapshot {
    files: HashMap<PathBuf, CheckedFile>,
    error: Option<String>,
}

struct CheckedFile {
    /// The text that was checked, which the byte offsets below index.
    text: String,
    line_starts: Vec<usize>,
    diagnostics: Vec<CheckedDiagnostic>,
    bindings: Vec<CheckedBinding>,
    types: Vec<CheckedType>,
    occurrences: Vec<CheckedOccurrence>,
    requires: Vec<CheckedRequire>,
}

struct CheckedDiagnostic { start: usize, end: usize, code: &'static str, message: String }
struct CheckedBinding { start: usize, end: usize, name: String, ty: String }
struct CheckedType { start: usize, end: usize, ty: String }
struct CheckedOccurrence { start: usize, end: usize, declaration: (usize, usize) }
struct CheckedRequire { start: usize, end: usize, target: PathBuf }

impl Snapshot {
    fn failed(error: String) -> Self {
        Self { files: HashMap::new(), error: Some(error) }
    }

    fn of(report: &ProjectCheckReport) -> Self {
        let mut files = HashMap::new();
        for checked in &report.files {
            let Some(source) = report.project.file(checked.file) else { continue };
            if source.origin != SourceOrigin::Filesystem { continue; }
            let result = &checked.report;
            let occurrences = result.bound.as_ref().map(|bound| bound.occurrences.iter().filter_map(|occurrence| {
                let declaration = bound.declaration(occurrence.binding?).span;
                Some(CheckedOccurrence { start: occurrence.span.start, end: occurrence.span.end, declaration: (declaration.start, declaration.end) })
            }).collect()).unwrap_or_default();
            let requires = report.project.dependencies.iter().filter(|dependency| dependency.from == checked.file).filter_map(|dependency| {
                let DependencyOutcome::Resolved(target) = dependency.outcome else { return None };
                Some(CheckedRequire {
                    start: dependency.request.span.start,
                    end: dependency.request.span.end,
                    target: report.project.file(target)?.path.clone(),
                })
            }).collect();
            let mut line_starts = vec![0];
            line_starts.extend(source.source.match_indices('\n').map(|(index, _)| index + 1));
            files.insert(source.path.clone(), CheckedFile {
                text: source.source.clone(),
                line_starts,
                diagnostics: result.diagnostics.iter().map(|diagnostic| CheckedDiagnostic {
                    start: diagnostic.span.start, end: diagnostic.span.end, code: diagnostic.code, message: diagnostic.message.clone(),
                }).collect(),
                bindings: result.bindings.iter().map(|binding| CheckedBinding {
                    start: binding.span.start, end: binding.span.end, name: binding.name.clone(), ty: binding.ty.clone(),
                }).collect(),
                types: result.types.iter().map(|ty| CheckedType { start: ty.span.start, end: ty.span.end, ty: ty.ty.clone() }).collect(),
                occurrences,
                requires,
            });
        }
        Self { files, error: None }
    }
}

impl CheckedFile {
    fn floor(&self, mut offset: usize) -> usize {
        offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) { offset -= 1; }
        offset
    }

    fn position(&self, offset: usize, encoding: Encoding) -> Json {
        let offset = self.floor(offset);
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let start = self.line_starts[line];
        let character = match encoding {
            Encoding::Utf8 => offset - start,
            Encoding::Utf16 => self.text[start..offset].chars().map(char::len_utf16).sum(),
        };
        Json::object([("line", Json::from(line)), ("character", Json::from(character))])
    }

    fn range(&self, start: usize, end: usize, encoding: Encoding) -> Json {
        Json::object([("start", self.position(start, encoding)), ("end", self.position(end, encoding))])
    }

    fn offset(&self, line: usize, character: usize, encoding: Encoding) -> usize {
        let Some(&start) = self.line_starts.get(line) else { return self.text.len() };
        let end = self.line_starts.get(line + 1).map_or(self.text.len(), |&next| next - 1);
        match encoding {
            Encoding::Utf8 => self.floor((start + character).min(end)),
            Encoding::Utf16 => {
                let mut units = 0;
                for (index, character_here) in self.text[start..end].char_indices() {
                    if units >= character { return start + index; }
                    units += character_here.len_utf16();
                }
                end
            }
        }
    }
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let path = PathBuf::from(String::from_utf8(decoded).ok()?);
    // Checked paths are canonical; an editor's may name a link to the same file.
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

fn path_to_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}
