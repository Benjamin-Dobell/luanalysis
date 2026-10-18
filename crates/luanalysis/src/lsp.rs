//! A language server: `luanalysis lsp` over stdio, or `LanguageServer` in this process. Each
//! revision of the editor's buffers is checked as one complete project, as `check` checks it;
//! hover and go-to-definition answer from the last completed check. Checks run on their own
//! thread and coalesce: requests are answered while a check runs, and edits made meanwhile are
//! checked next. Completions are language-service queries on the current text, answered on a
//! thread of their own (`language_service`).

use crate::DiagnosticCategory;
use crate::cli::CheckInputs;
use crate::incremental::ProjectWorkspace;
use crate::json::Json;
use crate::language_service::{self, CompletionKind, CompletionSite, SignatureSite};
use crate::project_check::{ProjectCheckOptions, ProjectCheckReport};
use crate::symbols::{SymbolIndex, SymbolOccurrence};
use luanalysis_project::{DependencyOutcome, Project, ProjectLoadPlan, SourceOrigin};
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

/// LSP error codes.
const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_REQUEST: i64 = -32600;
const REQUEST_FAILED: i64 = -32803;
const SERVER_NOT_INITIALIZED: i64 = -32002;

/// Receives each message for the client: one JSON-RPC message, without framing.
type Output = Arc<dyn Fn(&str) + Send + Sync>;

/// A language server running in this process, as `luanalysis lsp` runs over stdio.
pub struct LanguageServer {
    connection: Connection,
    server: Option<std::thread::JoinHandle<i32>>,
}

/// Delivers the client's messages to a `LanguageServer`, from any thread.
#[derive(Clone)]
pub struct Connection {
    events: mpsc::Sender<Event>,
}

impl Connection {
    /// Delivers one message from the client: a JSON-RPC message, without framing. Returns false
    /// once the server has stopped.
    pub fn send(&self, message: &str) -> bool {
        let event = match Json::parse(message) {
            Ok(message) => Event::Message(message),
            Err(error) => Event::Malformed(error.to_string()),
        };
        self.events.send(event).is_ok()
    }

    /// Ends the client's stream, as a closed stdin ends `luanalysis lsp`.
    pub fn close(&self) {
        let _ = self.events.send(Event::Closed);
    }
}

impl LanguageServer {
    /// Starts a server for `inputs` and `options`, as `luanalysis lsp` takes them. `output` is
    /// called with each message for the client, in order, from the server's own thread.
    pub fn start(inputs: CheckInputs, options: ProjectCheckOptions, output: impl Fn(&str) + Send + Sync + 'static) -> Self {
        let options = server_options(options);
        let (events, received) = mpsc::channel();
        let server_events = events.clone();
        let output: Output = Arc::new(output);
        let server = std::thread::Builder::new()
            .name("luanalysis-lsp".into())
            .spawn(move || Server::new(inputs, options, server_events, output).serve(received))
            .expect("start the language server's thread");
        Self { connection: Connection { events }, server: Some(server) }
    }

    /// The connection delivering the client's messages.
    pub fn connection(&self) -> Connection {
        self.connection.clone()
    }

    /// Delivers one message from the client (`Connection::send`).
    pub fn send(&self, message: &str) -> bool {
        self.connection.send(message)
    }

    /// Waits for the server to end, after the client's `exit` notification or the end of its
    /// stream. The exit code is 0 after `shutdown` and `exit`, and 1 otherwise.
    pub fn wait(mut self) -> i32 {
        self.server.take().map_or(1, |server| server.join().unwrap_or(1))
    }

    /// Ends the client's stream and waits for the server (`Connection::close`, `wait`).
    pub fn stop(self) -> i32 {
        self.connection.close();
        self.wait()
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            self.connection.close();
            let _ = server.join();
        }
    }
}

/// Hover reads the described types of the last check, and an editor shows inspections. Checks
/// leave a core to the query thread, so a completion does not wait on a check.
fn server_options(mut options: ProjectCheckOptions) -> ProjectCheckOptions {
    options.describe_types = true;
    options.inspections = true;
    if options.jobs == 0 {
        options.jobs = std::thread::available_parallelism().map_or(1, |cores| cores.get().saturating_sub(1).max(1));
    }
    options
}

/// `luanalysis lsp`: the server over stdin and stdout, with LSP's Content-Length framing.
pub fn run_stdio(inputs: CheckInputs, options: ProjectCheckOptions) -> i32 {
    let server = LanguageServer::start(inputs, options, |message| {
        let mut out = io::stdout().lock();
        // A closed client ends the server through its input.
        let _ = write!(out, "Content-Length: {}\r\n\r\n{message}", message.len());
        let _ = out.flush();
    });
    let connection = server.connection();
    std::thread::spawn(move || {
        let mut input = BufReader::new(io::stdin().lock());
        while let Ok(Some(text)) = read_message(&mut input) {
            if !connection.send(&text) { return; }
        }
        connection.close();
    });
    server.wait()
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
    /// A message that is not JSON.
    Malformed(String),
    Closed,
    Checked(Box<Snapshot>),
    /// A response computed off the main thread.
    Answered(Json, Json),
}

struct Job {
    overlays: HashMap<PathBuf, String>,
}

/// A completion or signature help request, with the editor's buffers and the site in its
/// document.
struct Query {
    id: Json,
    path: PathBuf,
    overlays: HashMap<PathBuf, String>,
    /// The server's `disk_generation` when the query was made.
    disk: u64,
    site: Site,
    /// The document's text, which the site indexes.
    lines: LineIndex,
    encoding: Encoding,
}

enum Site {
    Completion(CompletionSite),
    Signature(SignatureSite),
    Hover(HoverSite),
    Navigate(NavigateSite),
}

/// A navigation from a member or type name, which the symbol index of locals and globals does
/// not answer.
struct NavigateSite {
    text: String,
    offset: usize,
    request: Navigate,
}

#[derive(Clone)]
enum Navigate {
    Definition,
    References { declaration: bool },
    Highlight,
    PrepareRename,
    Rename(String),
}

/// A hover in a document's current text, with what the last check knows of the type there.
struct HoverSite {
    text: String,
    offset: usize,
    ty: Option<String>,
    range: Option<Json>,
}

impl Query {
    /// The text checked in place of the document's.
    fn text(&self) -> &str {
        match &self.site {
            Site::Completion(site) => &site.text,
            Site::Signature(site) => &site.text,
            Site::Hover(site) => &site.text,
            Site::Navigate(site) => &site.text,
        }
    }

    /// The answer when none is computed, such as for a query a newer one superseded.
    fn unanswered(&self) -> Json {
        match &self.site {
            Site::Completion(_) => Json::object([("isIncomplete", Json::from(true)), ("items", Json::Array(Vec::new()))]),
            Site::Signature(_) | Site::Navigate(_) => Json::Null,
            // Without documentation, a hover still has the last check's type.
            Site::Hover(site) => match &site.ty {
                None => Json::Null,
                Some(ty) => {
                    let mut fields = vec![("contents".to_owned(), markdown(format!("```lua\n{ty}\n```")))];
                    if let Some(range) = &site.range { fields.push(("range".to_owned(), range.clone())); }
                    Json::Object(fields)
                }
            },
        }
    }

    fn same_kind(&self, other: &Query) -> bool {
        std::mem::discriminant(&self.site) == std::mem::discriminant(&other.site)
    }
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
    output: Output,
    jobs: Option<mpsc::Sender<Job>>,
    queries: Option<mpsc::Sender<Query>>,
    encoding: Encoding,
    initialized: bool,
    shutdown: bool,
    documents: HashMap<PathBuf, Document>,
    /// Advanced by every change to what a check reads.
    revision: u64,
    /// Advanced by every change a completion's cached project cannot see: files saved or
    /// changed on disk, and documents opened or closed.
    disk_generation: u64,
    /// The revision of the last job sent to the checker.
    requested: Option<u64>,
    checking: bool,
    snapshot: Option<Snapshot>,
    published: HashSet<PathBuf>,
}

impl Server {
    fn new(inputs: CheckInputs, options: ProjectCheckOptions, events: mpsc::Sender<Event>, output: Output) -> Self {
        Self {
            inputs: Some(inputs),
            options,
            events,
            output,
            jobs: None,
            queries: None,
            encoding: Encoding::Utf16,
            initialized: false,
            shutdown: false,
            documents: HashMap::new(),
            revision: 0,
            disk_generation: 0,
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
                    Event::Answered(id, result) => self.respond(id, result),
                    Event::Malformed(error) => self.respond_error(Json::Null, PARSE_ERROR, &format!("a message that is not JSON: {error}")),
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
            "textDocument/hover" => self.hover_request(id, params),
            "textDocument/definition" => match self.definition(params) {
                Json::Null => self.navigate(id, params, Navigate::Definition),
                result => self.respond(id, result),
            },
            "textDocument/references" => {
                let declaration = params.path(&["context", "includeDeclaration"]) == Some(&Json::Bool(true));
                match self.references(params, declaration) {
                    Json::Null => self.navigate(id, params, Navigate::References { declaration }),
                    result => self.respond(id, result),
                }
            }
            "textDocument/completion" => self.completion(id, params),
            "textDocument/signatureHelp" => self.signature_help(id, params),
            "textDocument/documentHighlight" => match self.highlights(params) {
                Json::Null => self.navigate(id, params, Navigate::Highlight),
                result => self.respond(id, result),
            },
            "textDocument/prepareRename" if self.symbol_at(params).is_none() => self.navigate(id, params, Navigate::PrepareRename),
            "textDocument/prepareRename" => match self.prepare_rename(params) {
                Ok(result) => self.respond(id, result),
                Err(error) => self.respond_error(id, REQUEST_FAILED, &error),
            },
            "textDocument/rename" if self.symbol_at(params).is_none() => {
                let name = params.get("newName").and_then(Json::as_str).unwrap_or_default().to_owned();
                let valid = name.split('.').all(|part| {
                    let mut bytes = part.bytes();
                    bytes.next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
                        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                });
                if valid { self.navigate(id, params, Navigate::Rename(name)) } else { self.respond_error(id, REQUEST_FAILED, &format!("{name:?} is not a name")) }
            }
            "textDocument/rename" => match self.rename(params) {
                Ok(result) => self.respond(id, result),
                Err(error) => self.respond_error(id, REQUEST_FAILED, &error),
            },
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
                    self.disk_generation += 1;
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
                    self.disk_generation += 1;
                }
            }
            "textDocument/didSave" | "workspace/didChangeWatchedFiles" => {
                self.revision += 1;
                self.disk_generation += 1;
            }
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
            let (queries, received) = mpsc::channel();
            let events = self.events.clone();
            let options = self.options.clone();
            let query_options = options.clone();
            let query_inputs = inputs.clone();
            std::thread::Builder::new()
                .name("luanalysis-lsp-query".into())
                .spawn(move || answer_queries(query_inputs, query_options, received, events))
                .expect("start the language server's query thread");
            self.queries = Some(queries);
            let (jobs, received) = mpsc::channel();
            let events = self.events.clone();
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
                ("renameProvider", Json::object([("prepareProvider", Json::from(true))])),
                ("completionProvider", Json::object([
                    ("triggerCharacters", Json::Array(vec![Json::from("."), Json::from(":")])),
                ])),
                ("signatureHelpProvider", Json::object([
                    ("triggerCharacters", Json::Array(vec![Json::from("("), Json::from(",")])),
                    ("retriggerCharacters", Json::Array(vec![Json::from(")")])),
                ])),
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
            let diagnostics = file.diagnostics.iter().map(|diagnostic| {
                // LSP severities: 1 error, 2 warning, 4 hint. Tag 1 shows unnecessary code faded.
                let severity = match diagnostic.category {
                    DiagnosticCategory::Error => 1usize,
                    DiagnosticCategory::Warning => 2,
                    DiagnosticCategory::Suggestion => 4,
                };
                let mut fields = vec![
                    ("range".to_owned(), file.range(diagnostic.start, diagnostic.end, self.encoding)),
                    ("severity".to_owned(), Json::from(severity)),
                    ("code".to_owned(), Json::from(diagnostic.code)),
                    ("source".to_owned(), Json::from("luanalysis")),
                    ("message".to_owned(), Json::from(diagnostic.message.as_str())),
                ];
                if diagnostic.unnecessary { fields.push(("tags".to_owned(), Json::Array(vec![Json::from(1usize)]))); }
                Json::Object(fields)
            }).collect();
            self.publish(path, Json::Array(diagnostics), &file.lines.text);
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

    /// A hover shows the last check's type at the position and, from the document's current
    /// text, what the documentation of the name there says.
    fn hover_request(&mut self, id: Json, params: &Json) {
        let checked = self.hover(params);
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else {
            return self.respond(id, checked);
        };
        let Some(document) = self.documents.get(&path) else { return self.respond(id, checked) };
        let lines = LineIndex::new(document.text.clone());
        let line = params.path(&["position", "line"]).and_then(Json::as_u64).unwrap_or(0) as usize;
        let character = params.path(&["position", "character"]).and_then(Json::as_u64).unwrap_or(0) as usize;
        let offset = lines.offset(line, character, self.encoding);
        // The checked type applies only while the document is as it was checked.
        let current = self.snapshot.as_ref().and_then(|snapshot| snapshot.files.get(&path))
            .is_some_and(|file| file.lines.text == document.text);
        let (ty, range) = match (&checked, current) {
            (Json::Object(_), true) => (
                checked.path(&["contents", "value"]).and_then(Json::as_str)
                    .map(|value| value.trim_start_matches("```lua\n").trim_end_matches("\n```").to_owned()),
                checked.get("range").cloned(),
            ),
            _ => (None, None),
        };
        let site = HoverSite { text: document.text.clone(), offset, ty, range };
        self.query(id, path, Site::Hover(site), lines);
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
        let Some((_, file, offset)) = self.file_at(params) else { return Json::Null };
        let definitions = self.symbol_at(params).map(|(index, symbol)| index.definitions(&symbol)).unwrap_or_default();
        if !definitions.is_empty() {
            return Json::Array(definitions.iter().filter_map(|definition| self.location(definition)).collect());
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

    /// Completions read the document's current text, not the last check's: the query thread
    /// checks what is needed of the text being typed.
    fn completion(&mut self, id: Json, params: &Json) {
        let empty = Json::object([("isIncomplete", Json::from(false)), ("items", Json::Array(Vec::new()))]);
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else {
            return self.respond(id, empty);
        };
        let Some(document) = self.documents.get(&path) else { return self.respond(id, empty) };
        let lines = LineIndex::new(document.text.clone());
        let line = params.path(&["position", "line"]).and_then(Json::as_u64).unwrap_or(0) as usize;
        let character = params.path(&["position", "character"]).and_then(Json::as_u64).unwrap_or(0) as usize;
        let offset = lines.offset(line, character, self.encoding);
        let Some(site) = language_service::completion_site(&lines.text, offset) else { return self.respond(id, empty) };
        self.query(id, path, Site::Completion(site), lines);
    }

    /// Navigation from a member or type name, answered from the document's current text.
    fn navigate(&mut self, id: Json, params: &Json, request: Navigate) {
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else {
            return self.respond(id, Json::Null);
        };
        let text = match self.documents.get(&path) {
            Some(document) => document.text.clone(),
            None => match self.snapshot.as_ref().and_then(|snapshot| snapshot.files.get(&path)) {
                Some(file) => file.lines.text.clone(),
                None => return self.respond(id, Json::Null),
            },
        };
        let lines = LineIndex::new(text.clone());
        let line = params.path(&["position", "line"]).and_then(Json::as_u64).unwrap_or(0) as usize;
        let character = params.path(&["position", "character"]).and_then(Json::as_u64).unwrap_or(0) as usize;
        let offset = lines.offset(line, character, self.encoding);
        self.query(id, path, Site::Navigate(NavigateSite { text, offset, request }), lines);
    }

    /// The signatures of the call the cursor is in, from the document's current text.
    fn signature_help(&mut self, id: Json, params: &Json) {
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else {
            return self.respond(id, Json::Null);
        };
        let Some(document) = self.documents.get(&path) else { return self.respond(id, Json::Null) };
        let lines = LineIndex::new(document.text.clone());
        let line = params.path(&["position", "line"]).and_then(Json::as_u64).unwrap_or(0) as usize;
        let character = params.path(&["position", "character"]).and_then(Json::as_u64).unwrap_or(0) as usize;
        let offset = lines.offset(line, character, self.encoding);
        let Some(site) = language_service::signature_site(&lines.text, offset) else { return self.respond(id, Json::Null) };
        self.query(id, path, Site::Signature(site), lines);
    }

    /// Answers a query on the query thread, with the editor's buffers and `site`'s text in place
    /// of its document's.
    fn query(&mut self, id: Json, path: PathBuf, site: Site, lines: LineIndex) {
        let mut overlays: HashMap<PathBuf, String> = self.documents.iter()
            .map(|(path, document)| (path.clone(), document.text.clone())).collect();
        let query = Query { id: id.clone(), path: path.clone(), overlays: HashMap::new(), disk: self.disk_generation, site, lines, encoding: self.encoding };
        overlays.insert(path, query.text().to_owned());
        let query = Query { overlays, ..query };
        let unanswered = query.unanswered();
        if self.queries.as_ref().is_none_or(|queries| queries.send(query).is_err()) {
            self.respond(id, unanswered);
        }
    }

    /// The symbol index of the last check, and the symbol at a request's position.
    fn symbol_at(&self, params: &Json) -> Option<(&SymbolIndex, crate::symbols::Symbol)> {
        let (path, file, offset) = self.file_at(params)?;
        let _ = file;
        let index = &self.snapshot.as_ref()?.symbols;
        let (symbol, _) = index.symbol_at(index.file(path)?, offset)?;
        Some((index, symbol))
    }

    fn location(&self, occurrence: &SymbolOccurrence) -> Option<Json> {
        let file = self.snapshot.as_ref()?.files.get(&occurrence.path)?;
        Some(Json::object([
            ("uri", Json::from(self.uri(&occurrence.path))),
            ("range", file.range(occurrence.span.start, occurrence.span.end, self.encoding)),
        ]))
    }

    /// The URI the client knows a path by: an open document's own, or the path's.
    fn uri(&self, path: &Path) -> String {
        self.documents.get(path).map_or_else(|| path_to_uri(path), |document| document.uri.clone())
    }

    /// Every occurrence, in every file, of the local or global under the position; its
    /// definitions only when `declaration` is set.
    fn references(&self, params: &Json, declaration: bool) -> Json {
        let Some((index, symbol)) = self.symbol_at(params) else { return Json::Null };
        Json::Array(index.occurrences(&symbol).iter()
            .filter(|occurrence| declaration || !occurrence.definition)
            .filter_map(|occurrence| self.location(occurrence))
            .collect())
    }

    /// The occurrences in this file of the local or global under the position: definitions as
    /// writes, the others as reads.
    fn highlights(&self, params: &Json) -> Json {
        let Some((path, file, _)) = self.file_at(params) else { return Json::Null };
        let Some((index, symbol)) = self.symbol_at(params) else { return Json::Null };
        Json::Array(index.occurrences(&symbol).iter()
            .filter(|occurrence| occurrence.path == path)
            .map(|occurrence| Json::object([
                ("range", file.range(occurrence.span.start, occurrence.span.end, self.encoding)),
                ("kind", Json::from(if occurrence.definition { 3usize } else { 2 })),
            ]))
            .collect())
    }

    fn prepare_rename(&self, params: &Json) -> Result<Json, String> {
        let Some((path, file, offset)) = self.file_at(params) else { return Ok(Json::Null) };
        let index = &self.snapshot.as_ref().expect("a checked file has a snapshot").symbols;
        let Some(id) = index.file(path) else { return Ok(Json::Null) };
        let (_, span) = index.prepare_rename(id, offset).map_err(|error| error.to_string())?;
        Ok(Json::object([
            ("range", file.range(span.start, span.end, self.encoding)),
            ("placeholder", Json::from(&file.lines.text[span.start..span.end])),
        ]))
    }

    /// The edits that rename the local or global under the position, in every file. A file
    /// edited since the last check is renamed after the next one, when its offsets are current.
    fn rename(&self, params: &Json) -> Result<Json, String> {
        let Some((path, _, offset)) = self.file_at(params) else { return Ok(Json::Null) };
        let name = params.get("newName").and_then(Json::as_str).unwrap_or_default();
        let snapshot = self.snapshot.as_ref().expect("a checked file has a snapshot");
        let Some(id) = snapshot.symbols.file(path) else { return Ok(Json::Null) };
        let occurrences = snapshot.symbols.rename(id, offset, name).map_err(|error| error.to_string())?;
        let mut changes: Vec<(String, Json)> = Vec::new();
        for occurrence in occurrences {
            let Some(file) = snapshot.files.get(&occurrence.path) else { continue };
            if self.documents.get(&occurrence.path).is_some_and(|document| document.text != file.lines.text) {
                return Err("the project is being checked; rename again when it has been".into());
            }
            let edit = Json::object([
                ("range", file.range(occurrence.span.start, occurrence.span.end, self.encoding)),
                ("newText", Json::from(name)),
            ]);
            let uri = self.uri(&occurrence.path);
            match changes.iter_mut().find(|(existing, _)| *existing == uri) {
                Some((_, Json::Array(edits))) => edits.push(edit),
                _ => changes.push((uri, Json::Array(vec![edit]))),
            }
        }
        Ok(Json::object([("changes", Json::Object(changes))]))
    }

    fn respond(&self, id: Json, result: Json) {
        self.write(&Json::object([("jsonrpc", Json::from("2.0")), ("id", id), ("result", result)]));
    }

    fn respond_error(&self, id: Json, code: i64, message: &str) {
        self.write(&Json::object([
            ("jsonrpc", Json::from("2.0")),
            ("id", id),
            ("error", Json::object([("code", Json::from(code)), ("message", Json::from(message))])),
        ]));
    }

    fn notify(&self, method: &str, params: Json) {
        self.write(&Json::object([("jsonrpc", Json::from("2.0")), ("method", Json::from(method)), ("params", params)]));
    }

    fn write(&self, message: &Json) {
        (self.output)(&message.to_string());
    }
}


fn check_jobs(inputs: CheckInputs, options: ProjectCheckOptions, jobs: mpsc::Receiver<Job>, events: mpsc::Sender<Event>) {
    let mut workspace = ProjectWorkspace::new();
    while let Ok(mut job) = jobs.recv() {
        // Only the latest revision is worth checking.
        while let Ok(newer) = jobs.try_recv() { job = newer; }
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&inputs, options.clone(), &mut workspace, &job)));
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

fn answer_queries(inputs: CheckInputs, options: ProjectCheckOptions, queries: mpsc::Receiver<Query>, events: mpsc::Sender<Event>) {
    let mut workspace = ProjectWorkspace::new();
    let mut loaded = None;
    while let Ok(first) = queries.recv() {
        let mut pending = vec![first];
        while let Ok(newer) = queries.try_recv() { pending.push(newer); }
        // Only the latest query of each kind is worth answering; one superseded by a newer one
        // of its kind is answered unchecked.
        let mut latest: Vec<Query> = Vec::new();
        for query in pending.into_iter().rev() {
            if latest.iter().any(|newer| newer.same_kind(&query)) {
                if events.send(Event::Answered(query.id.clone(), query.unanswered())).is_err() { return; }
            } else {
                latest.push(query);
            }
        }
        for query in latest.into_iter().rev() {
            let started = std::time::Instant::now();
            let mut answered = false;
            // The answer is sent as soon as it is known, before the session's state is dropped.
            let mut answer = |result: Json| {
                answered = true;
                if std::env::var_os("LUANALYSIS_PROFILE").is_some() {
                    eprintln!("luanalysis profile: query={:.6}s", started.elapsed().as_secs_f64());
                }
                let _ = events.send(Event::Answered(query.id.clone(), result));
            };
            let completed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &query.site {
                Site::Completion(site) => complete(&inputs, options.clone(), &mut workspace, &mut loaded, &query, site, &mut answer),
                Site::Signature(site) => signatures(&inputs, options.clone(), &mut workspace, &mut loaded, &query, site, &mut answer),
                Site::Hover(site) => hover(&inputs, options.clone(), &mut workspace, &mut loaded, &query, site, &mut answer),
                Site::Navigate(site) => navigate(&inputs, options.clone(), &mut workspace, &mut loaded, &query, site, &mut answer),
            }));
            if completed.is_err() {
                workspace = ProjectWorkspace::new();
                loaded = None;
            }
            if !answered && events.send(Event::Answered(query.id.clone(), query.unanswered())).is_err() { return; }
        }
    }
}

/// The project a completion last loaded, before any session registered sources in it.
struct LoadedProject {
    disk: u64,
    path: PathBuf,
    /// The other documents' texts it was loaded with.
    others: HashMap<PathBuf, String>,
    project: Project,
}

/// The project for a completion. Between completions usually only the document being typed
/// changes: when nothing on disk or in other documents did, and its requires name the same
/// modules, the last project is reused with its new text instead of being discovered, read and
/// resolved again.
fn load(inputs: &CheckInputs, workspace: &mut ProjectWorkspace, loaded: &mut Option<LoadedProject>, query: &Query) -> Option<Project> {
    let others: HashMap<PathBuf, String> = query.overlays.iter()
        .filter(|(path, _)| **path != query.path)
        .map(|(path, text)| (path.clone(), text.clone()))
        .collect();
    if let Some(last) = loaded.as_mut()
        && last.disk == query.disk && last.path == query.path && last.others == others
        && let Some(file) = last.project.file_by_path(&query.path).cloned()
    {
        let edited = luanalysis_project::SourceFile { source: query.text().to_owned(), ..file };
        let requests = workspace.static_require_requests(&edited);
        if last.project.replace_source(edited.id, edited.source, requests) {
            return Some(last.project.clone());
        }
    }
    let discovery = inputs.discover().ok()?;
    let resolver = inputs.resolver(&discovery).ok()?;
    let project = Project::load_plan_with_overlays(&resolver, with_documents(&discovery.plan, &query.overlays), &query.overlays, |source| {
        workspace.static_require_requests(source)
    });
    *loaded = Some(LoadedProject { disk: query.disk, path: query.path.clone(), others, project: project.clone() });
    Some(project)
}

fn complete(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, loaded: &mut Option<LoadedProject>, query: &Query, site: &CompletionSite, answer: &mut (dyn FnMut(Json) + Send)) {
    let Some(project) = load(inputs, workspace, loaded, query) else { return };
    let Some(file) = project.file_by_path(&query.path).map(|file| file.id) else { return };
    let range = query.lines.range(site.word.start, site.word.end, query.encoding);
    language_service::complete(project, options, workspace, file, site, |items| answer(Json::object([("isIncomplete", Json::from(false)), ("items", Json::Array(items.into_iter().map(|item| {
        let mut fields = vec![
            ("label".to_owned(), Json::from(item.label.as_str())),
            ("kind".to_owned(), Json::from(match item.kind {
                CompletionKind::Method => 2usize,
                CompletionKind::Function => 3,
                CompletionKind::Field => 5,
                CompletionKind::Variable => 6,
                CompletionKind::Keyword => 14,
            })),
            ("textEdit".to_owned(), Json::object([("range", range.clone()), ("newText", Json::from(item.label.as_str()))])),
        ];
        if let Some(detail) = item.detail { fields.push(("detail".to_owned(), Json::from(detail))); }
        if let Some(documentation) = item.documentation {
            fields.push(("documentation".to_owned(), markdown(documentation)));
        }
        Json::Object(fields)
    }).collect()))])));
}

fn signatures(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, loaded: &mut Option<LoadedProject>, query: &Query, site: &SignatureSite, answer: &mut (dyn FnMut(Json) + Send)) {
    let Some(project) = load(inputs, workspace, loaded, query) else { return };
    let Some(file) = project.file_by_path(&query.path).map(|file| file.id) else { return };
    // Parameter ranges are offsets in their label, in the position encoding.
    let offset = |label: &str, byte: usize| match query.encoding {
        Encoding::Utf8 => byte,
        Encoding::Utf16 => label[..byte].encode_utf16().count(),
    };
    language_service::signature_help(project, options, workspace, file, site, |help| answer(match help {
        None => Json::Null,
        Some(help) => Json::object([
            ("signatures", Json::Array(help.signatures.iter().map(|signature| {
                let mut fields = vec![
                    ("label".to_owned(), Json::from(signature.label.as_str())),
                    ("parameters".to_owned(), Json::Array(signature.parameters.iter().zip(&signature.parameter_documentation).map(|(range, documentation)| {
                        let mut fields = vec![("label".to_owned(), Json::Array(vec![Json::from(offset(&signature.label, range.start)), Json::from(offset(&signature.label, range.end))]))];
                        if let Some(documentation) = documentation { fields.push(("documentation".to_owned(), markdown(documentation.clone()))); }
                        Json::Object(fields)
                    }).collect())),
                    ("activeParameter".to_owned(), Json::from(signature.active_parameter)),
                ];
                if let Some(documentation) = &signature.documentation { fields.push(("documentation".to_owned(), markdown(documentation.clone()))); }
                Json::Object(fields)
            }).collect())),
            ("activeSignature", Json::from(help.active_signature)),
            ("activeParameter", Json::from(help.active_parameter)),
        ]),
    }));
}

fn navigate(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, loaded: &mut Option<LoadedProject>, query: &Query, site: &NavigateSite, answer: &mut (dyn FnMut(Json) + Send)) {
    let Some(project) = load(inputs, workspace, loaded, query) else { return };
    let Some(file) = project.file_by_path(&query.path).map(|file| file.id) else { return };
    let references = !matches!(site.request, Navigate::Definition | Navigate::PrepareRename);
    let encoding = query.encoding;
    language_service::navigate(project, options, workspace, file, site.offset, references, |navigation| {
        let Some(navigation) = navigation else { return answer(Json::Null) };
        let lines: HashMap<&PathBuf, LineIndex> = navigation.texts.iter().map(|(path, text)| (path, LineIndex::new(text.clone()))).collect();
        let range = |path: &PathBuf, span: luanalysis_syntax::Span| lines.get(path).map(|lines| lines.range(span.start, span.end, encoding));
        let location = |(path, span, _): &(PathBuf, luanalysis_syntax::Span, bool)| {
            Some(Json::object([("uri", Json::from(path_to_uri(path))), ("range", range(path, *span)?)]))
        };
        answer(match &site.request {
            Navigate::Definition => Json::Array(navigation.occurrences.iter().filter(|occurrence| occurrence.2).filter_map(location).collect()),
            Navigate::References { declaration } => Json::Array(navigation.occurrences.iter()
                .filter(|occurrence| *declaration || !occurrence.2).filter_map(location).collect()),
            Navigate::Highlight => Json::Array(navigation.occurrences.iter().filter(|occurrence| occurrence.0 == query.path)
                .filter_map(|(path, span, declaration)| Some(Json::object([
                    ("range", range(path, *span)?),
                    ("kind", Json::from(if *declaration { 3usize } else { 2 })),
                ]))).collect()),
            Navigate::PrepareRename => Json::object([
                ("range", query.lines.range(navigation.span.start, navigation.span.end, encoding)),
                ("placeholder", Json::from(navigation.name.as_str())),
            ]),
            // A member's name is one identifier; a type's may be dotted.
            Navigate::Rename(name) if name.contains('.') && !navigation.type_name => Json::Null,
            Navigate::Rename(name) => {
                let mut changes: Vec<(String, Json)> = Vec::new();
                for (path, span, _) in &navigation.occurrences {
                    let Some(range) = range(path, *span) else { continue };
                    let edit = Json::object([("range", range), ("newText", Json::from(name.as_str()))]);
                    let uri = path_to_uri(path);
                    match changes.iter_mut().find(|(existing, _)| *existing == uri) {
                        Some((_, Json::Array(edits))) => edits.push(edit),
                        _ => changes.push((uri, Json::Array(vec![edit]))),
                    }
                }
                Json::object([("changes", Json::Object(changes))])
            }
        });
    });
}

/// Markdown content, as LSP `MarkupContent`.
fn markdown(value: String) -> Json {
    Json::object([("kind", Json::from("markdown")), ("value", Json::from(value))])
}

fn hover(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, loaded: &mut Option<LoadedProject>, query: &Query, site: &HoverSite, answer: &mut (dyn FnMut(Json) + Send)) {
    let Some(project) = load(inputs, workspace, loaded, query) else { return };
    let Some(file) = project.file_by_path(&query.path).map(|file| file.id) else { return };
    language_service::hover_documentation(project, options, workspace, file, site.offset, |documentation| {
        let mut sections: Vec<String> = site.ty.iter().map(|ty| format!("```lua\n{ty}\n```")).collect();
        sections.extend(documentation);
        answer(if sections.is_empty() { Json::Null } else {
            let mut fields = vec![("contents".to_owned(), markdown(sections.join("\n\n---\n\n")))];
            if let Some(range) = &site.range { fields.push(("range".to_owned(), range.clone())); }
            Json::Object(fields)
        });
    });
}

fn check(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, job: &Job) -> Result<ProjectCheckReport, String> {
    let discovery = inputs.discover().map_err(|error| error.to_string())?;
    let resolver = inputs.resolver(&discovery).map_err(|error| error.to_string())?;
    let project = Project::load_plan_with_overlays(&resolver, with_documents(&discovery.plan, &job.overlays), &job.overlays, |source| {
        workspace.static_require_requests(source)
    });
    Ok(workspace.check_project(project, options))
}

/// The workspace's inputs and every open `.lua` or `.ttslua` document, including one outside
/// the workspace or with no file on disk (a host such as a game may serve scripts it keeps in
/// memory).
fn with_documents(plan: &ProjectLoadPlan, documents: &HashMap<PathBuf, String>) -> ProjectLoadPlan {
    let mut plan = plan.clone();
    let mut opened: Vec<_> = documents.keys()
        .filter(|path| !plan.entries.contains(path))
        .filter(|path| path.extension().is_some_and(|extension| extension == "lua" || extension == "ttslua"))
        .cloned().collect();
    opened.sort();
    plan.entries.extend(opened);
    plan
}

/// What hover, definition and diagnostics read from one completed check.
struct Snapshot {
    files: HashMap<PathBuf, CheckedFile>,
    symbols: SymbolIndex,
    error: Option<String>,
}

struct CheckedFile {
    /// The text that was checked, which the byte offsets below index.
    lines: LineIndex,
    diagnostics: Vec<CheckedDiagnostic>,
    bindings: Vec<CheckedBinding>,
    types: Vec<CheckedType>,
    requires: Vec<CheckedRequire>,
}

struct CheckedDiagnostic { start: usize, end: usize, code: &'static str, message: String, category: DiagnosticCategory, unnecessary: bool }
struct CheckedBinding { start: usize, end: usize, name: String, ty: String }
struct CheckedType { start: usize, end: usize, ty: String }
struct CheckedRequire { start: usize, end: usize, target: PathBuf }

impl Snapshot {
    fn failed(error: String) -> Self {
        Self { files: HashMap::new(), symbols: SymbolIndex::default(), error: Some(error) }
    }

    fn of(report: &ProjectCheckReport) -> Self {
        let mut files = HashMap::new();
        for checked in &report.files {
            let Some(source) = report.project.file(checked.file) else { continue };
            if source.origin != SourceOrigin::Filesystem { continue; }
            let result = &checked.report;
            let requires = report.project.dependencies.iter().filter(|dependency| dependency.from == checked.file).filter_map(|dependency| {
                let DependencyOutcome::Resolved(target) = dependency.outcome else { return None };
                Some(CheckedRequire {
                    start: dependency.request.span.start,
                    end: dependency.request.span.end,
                    target: report.project.file(target)?.path.clone(),
                })
            }).collect();
            files.insert(source.path.clone(), CheckedFile {
                lines: LineIndex::new(source.source.clone()),
                diagnostics: result.diagnostics.iter().map(|diagnostic| CheckedDiagnostic {
                    start: diagnostic.span.start, end: diagnostic.span.end, code: diagnostic.code, message: diagnostic.message.clone(),
                    category: diagnostic.category(), unnecessary: diagnostic.is_unnecessary(),
                }).collect(),
                bindings: result.bindings.iter().map(|binding| CheckedBinding {
                    start: binding.span.start, end: binding.span.end, name: binding.name.clone(), ty: binding.ty.clone(),
                }).collect(),
                types: result.types.iter().map(|ty| CheckedType { start: ty.span.start, end: ty.span.end, ty: ty.ty.clone() }).collect(),
                requires,
            });
        }
        Self { files, symbols: SymbolIndex::new(report), error: None }
    }
}

impl CheckedFile {
    fn range(&self, start: usize, end: usize, encoding: Encoding) -> Json {
        self.lines.range(start, end, encoding)
    }

    fn offset(&self, line: usize, character: usize, encoding: Encoding) -> usize {
        self.lines.offset(line, character, encoding)
    }
}

/// Byte offsets of a text and LSP positions in it.
struct LineIndex {
    text: String,
    line_starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: String) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
        Self { text, line_starts }
    }

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
        if bytes[index] == b'%' && index + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    // A Windows path in a URI follows a slash: file:///C:/Users.
    let path = match path.as_bytes() {
        [b'/', drive, b':', ..] if cfg!(windows) && drive.is_ascii_alphabetic() => PathBuf::from(&path[1..]),
        _ => PathBuf::from(path),
    };
    // Checked paths are canonical; an editor's may name a link to the same file.
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

fn path_to_uri(path: &Path) -> String {
    let mut path = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        // Canonical Windows paths are verbatim (\\?\C:\Users); URIs name C:/Users.
        if let Some(rest) = path.strip_prefix(r"\\?\UNC\") { path = format!(r"\\{rest}") }
        else if let Some(rest) = path.strip_prefix(r"\\?\") { path = rest.to_owned() }
        path = path.replace('\\', "/");
        if path.as_bytes().get(1) == Some(&b':') { path.insert(0, '/') }
    }
    // A UNC path (//server/share) names its host: file://server/share.
    let mut uri = String::from(if path.starts_with("//") { "file:" } else { "file://" });
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}
