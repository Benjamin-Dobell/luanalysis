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
const REQUEST_CANCELLED: i64 = -32800;
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
    /// A request the client cancelled before it was answered.
    Cancelled(Json),
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
    /// Whether the client inserts completion snippets.
    snippets: bool,
}

enum Site {
    Hierarchy(HierarchySite),
    Actions(ActionSite),
    Completion(CompletionSite),
    Signature(SignatureSite),
    Hover(HoverSite),
    Navigate(NavigateSite),
    Hints(HintSite),
}

/// A call or type hierarchy request at a position of a document.
struct HierarchySite {
    text: String,
    offset: usize,
    request: Hierarchy,
}

#[derive(Clone, Copy)]
enum Hierarchy {
    Calls(language_service::CallDirection),
    /// None prepares; true asks for supertypes, false for subtypes.
    Types(Option<bool>),
}

/// Quick fixes for a document's diagnostics: those known without checking, and the
/// diagnostics (with their codes and spans) whose spelling fixes need a check.
struct ActionSite {
    text: String,
    actions: Vec<Json>,
    diagnostics: Vec<(Json, String, luanalysis_syntax::Span)>,
}

/// Inlay hints for a range of a document: the type hints the last check gives, to which the
/// parameter names of the range's calls are added.
struct HintSite {
    text: String,
    range: std::ops::Range<usize>,
    types: Vec<Json>,
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
    TypeDefinition,
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
            Site::Hints(site) => &site.text,
            Site::Actions(site) => &site.text,
            Site::Hierarchy(site) => &site.text,
        }
    }

    /// The answer when none is computed, such as for a query a newer one superseded.
    fn unanswered(&self) -> Json {
        match &self.site {
            Site::Completion(_) => Json::object([("isIncomplete", Json::from(true)), ("items", Json::Array(Vec::new()))]),
            Site::Signature(_) | Site::Navigate(_) => Json::Null,
            Site::Hints(site) => Json::Array(site.types.clone()),
            Site::Actions(site) => Json::Array(site.actions.clone()),
            Site::Hierarchy(_) => Json::Null,
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
    /// Whether the client inserts completion snippets.
    snippets: bool,
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
    /// The checked paths and options before the client's settings, and what the settings make of
    /// them and of the editor's features.
    base: Option<(CheckInputs, ProjectCheckOptions)>,
    settings: Json,
    preferences: Preferences,
    /// The requests the client cancelled, which the query thread answers as cancelled.
    cancelled: Arc<std::sync::Mutex<HashSet<String>>>,
    /// Whether the client shows work-done progress; the requests made to create a token, and the
    /// token of the progress shown while a check runs.
    progress: bool,
    next_request: u64,
    progress_token: Option<(String, bool)>,
    /// Each file's outline, by the text it was made from.
    symbols: HashMap<PathBuf, (String, Vec<language_service::DocumentSymbol>)>,
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
            snippets: false,
            initialized: false,
            shutdown: false,
            documents: HashMap::new(),
            revision: 0,
            disk_generation: 0,
            requested: None,
            checking: false,
            snapshot: None,
            published: HashSet::new(),
            base: None,
            settings: Json::Null,
            preferences: Preferences::default(),
            cancelled: Arc::new(std::sync::Mutex::new(HashSet::new())),
            progress: false,
            next_request: 0,
            progress_token: None,
            symbols: HashMap::new(),
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
                    Event::Cancelled(id) => self.respond_error(id, REQUEST_CANCELLED, "the request was cancelled"),
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
            self.begin_progress();
        }
    }

    fn message(&mut self, message: Json) -> Option<i32> {
        let method = message.get("method").and_then(Json::as_str).map(str::to_owned);
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        match (method, message.get("id").cloned()) {
            (Some(method), Some(id)) => self.request(&method, id, &params),
            (Some(method), None) => return self.notification(&method, &params),
            // A response to a request of ours: a progress token the client created.
            (None, Some(id)) => {
                if let Some((token, begun)) = &mut self.progress_token
                    && id.as_str() == Some(token.as_str()) && !*begun
                {
                    *begun = true;
                    let token = token.clone();
                    self.notify("$/progress", Json::object([("token", Json::from(token)), ("value", Json::object([
                        ("kind", Json::from("begin")),
                        ("title", Json::from("Checking Lua")),
                    ]))]));
                    // A check that ended before the token was created ends its progress at once.
                    if !self.checking { self.end_progress(); }
                }
            }
            (None, None) => {}
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
            "textDocument/typeDefinition" => self.navigate(id, params, Navigate::TypeDefinition),
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
            "textDocument/documentSymbol" => {
                let result = self.document_symbols(params);
                self.respond(id, result);
            }
            "workspace/symbol" => {
                let result = self.workspace_symbols(params);
                self.respond(id, result);
            }
            "textDocument/inlayHint" => self.inlay_hints(id, params),
            "textDocument/codeAction" => self.code_actions(id, params),
            "textDocument/prepareCallHierarchy" => self.hierarchy(id, params, Hierarchy::Calls(language_service::CallDirection::Prepare)),
            "callHierarchy/incomingCalls" => self.hierarchy(id, params, Hierarchy::Calls(language_service::CallDirection::Incoming)),
            "callHierarchy/outgoingCalls" => self.hierarchy(id, params, Hierarchy::Calls(language_service::CallDirection::Outgoing)),
            "textDocument/prepareTypeHierarchy" => self.hierarchy(id, params, Hierarchy::Types(None)),
            "typeHierarchy/supertypes" => self.hierarchy(id, params, Hierarchy::Types(Some(true))),
            "typeHierarchy/subtypes" => self.hierarchy(id, params, Hierarchy::Types(Some(false))),
            "textDocument/semanticTokens/full" => {
                let result = self.semantic_tokens(params);
                self.respond(id, result);
            }
            "textDocument/foldingRange" => {
                let result = self.folding_ranges(params);
                self.respond(id, result);
            }
            "textDocument/selectionRange" => {
                let result = self.selection_ranges(params);
                self.respond(id, result);
            }
            "textDocument/documentLink" => {
                let result = self.document_links(params);
                self.respond(id, result);
            }
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
                if let Some(uri) = uri && let Some(path) = uri_to_path(uri) {
                    let encoding = self.encoding;
                    let document = self.documents.entry(path).or_insert_with(|| Document { uri: uri.to_owned(), text: String::new(), version: 0 });
                    // Incremental synchronization: each change replaces a range, or without one, the whole text.
                    for change in params.get("contentChanges").and_then(Json::as_array).unwrap_or(&[]) {
                        let Some(text) = change.get("text").and_then(Json::as_str) else { continue };
                        match change.get("range") {
                            Some(range) => {
                                let lines = LineIndex::new(std::mem::take(&mut document.text));
                                let offset = |name: &str| lines.offset(
                                    range.path(&[name, "line"]).and_then(Json::as_u64).unwrap_or(0) as usize,
                                    range.path(&[name, "character"]).and_then(Json::as_u64).unwrap_or(0) as usize,
                                    encoding,
                                );
                                let (start, end) = (offset("start"), offset("end"));
                                let mut edited = lines.text;
                                edited.replace_range(start..end.max(start), text);
                                document.text = edited;
                            }
                            None => document.text = text.to_owned(),
                        }
                    }
                    if let Some(version) = version { document.version = version as i64; }
                    self.revision += 1;
                }
            }
            "workspace/didChangeConfiguration" => {
                let settings = params.get("settings").cloned().unwrap_or(Json::Null);
                if let Err(error) = self.configure(&settings) { self.log(1, &format!("luanalysis: settings: {error}")); }
            }
            "workspace/didChangeWorkspaceFolders" => {
                let folders = |kind: &str| -> Vec<PathBuf> { params.path(&["event", kind]).and_then(Json::as_array).unwrap_or(&[]).iter()
                    .filter_map(|folder| folder.get("uri").and_then(Json::as_str).and_then(uri_to_path)).collect() };
                let (added, removed) = (folders("added"), folders("removed"));
                if let Some((inputs, _)) = &mut self.base {
                    inputs.files.retain(|file| !removed.contains(file));
                    for folder in added {
                        if !inputs.files.contains(&folder) { inputs.files.push(folder); }
                    }
                }
                // The settings already applied apply to the new folders too.
                let settings = self.settings.clone();
                if let Err(error) = self.configure(&settings) { self.log(1, &format!("luanalysis: settings: {error}")); }
            }
            "$/cancelRequest" => {
                if let Some(id) = params.get("id") {
                    self.cancelled.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string());
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

    /// Applies the client's settings (`initializationOptions`, or `workspace/didChangeConfiguration`'s,
    /// either whole or under `luanalysis`) to the checked paths and options, and restarts checking
    /// and queries with them.
    fn configure(&mut self, settings: &Json) -> Result<(), String> {
        self.settings = settings.clone();
        let Some((inputs, options)) = self.base.clone() else { return Ok(()) };
        let settings = settings.get("luanalysis").unwrap_or(settings);
        let (inputs, options, preferences) = apply_settings(inputs, options, settings)?;
        self.preferences = preferences;
        self.start_workers(inputs, options);
        Ok(())
    }

    /// Starts the threads that check and answer queries, ending any running with older settings.
    fn start_workers(&mut self, inputs: CheckInputs, options: ProjectCheckOptions) {
        let (queries, received) = mpsc::channel();
        let events = self.events.clone();
        let query_options = options.clone();
        let query_inputs = inputs.clone();
        let cancelled = Arc::clone(&self.cancelled);
        std::thread::Builder::new()
            .name("luanalysis-lsp-query".into())
            .spawn(move || answer_queries(query_inputs, query_options, received, events, cancelled))
            .expect("start the language server's query thread");
        self.queries = Some(queries);
        let (jobs, received) = mpsc::channel();
        let events = self.events.clone();
        std::thread::Builder::new()
            .name("luanalysis-lsp-check".into())
            .spawn(move || check_jobs(inputs, options, received, events))
            .expect("start the language server's checking thread");
        self.jobs = Some(jobs);
        // A new checker checks everything again, and a new query thread loads the project again.
        self.requested = None;
        self.checking = false;
        self.revision += 1;
        self.disk_generation += 1;
    }

    fn log(&self, level: usize, message: &str) {
        self.notify("window/logMessage", Json::object([("type", Json::from(level)), ("message", Json::from(message))]));
    }

    /// Shows that the workspace is being checked, when the client shows progress: a token is
    /// created first, and progress begins when the client answers.
    fn begin_progress(&mut self) {
        if !self.progress || self.progress_token.is_some() { return; }
        self.next_request += 1;
        let token = format!("luanalysis/check/{}", self.next_request);
        self.write(&Json::object([
            ("jsonrpc", Json::from("2.0")),
            ("id", Json::from(token.as_str())),
            ("method", Json::from("window/workDoneProgress/create")),
            ("params", Json::object([("token", Json::from(token.as_str()))])),
        ]));
        self.progress_token = Some((token, false));
    }

    fn end_progress(&mut self) {
        if let Some((token, true)) = self.progress_token.take() {
            self.notify("$/progress", Json::object([("token", Json::from(token)), ("value", Json::object([("kind", Json::from("end"))]))]));
        }
    }

    fn initialize(&mut self, params: &Json) -> Json {
        let encodings = params.path(&["capabilities", "general", "positionEncodings"]).and_then(Json::as_array).unwrap_or(&[]);
        self.encoding = if encodings.iter().any(|encoding| encoding.as_str() == Some("utf-8")) { Encoding::Utf8 } else { Encoding::Utf16 };
        self.snippets = params.path(&["capabilities", "textDocument", "completion", "completionItem", "snippetSupport"]) == Some(&Json::Bool(true));
        self.progress = params.path(&["capabilities", "window", "workDoneProgress"]) == Some(&Json::Bool(true));
        if let Some(mut inputs) = self.inputs.take() {
            if inputs.files.is_empty() {
                // Every workspace folder, or failing that the root.
                let folders: Vec<PathBuf> = params.get("workspaceFolders").and_then(Json::as_array).unwrap_or(&[]).iter()
                    .filter_map(|folder| folder.get("uri").and_then(Json::as_str).and_then(uri_to_path)).collect();
                if folders.is_empty() {
                    let root = params.get("rootUri").and_then(Json::as_str).and_then(uri_to_path)
                        .or_else(|| params.get("rootPath").and_then(Json::as_str).map(PathBuf::from))
                        .or_else(|| std::env::current_dir().ok());
                    inputs.files.extend(root);
                } else {
                    inputs.files.extend(folders);
                }
            }
            self.base = Some((inputs, self.options.clone()));
            let settings = params.get("initializationOptions").cloned().unwrap_or(Json::Null);
            if let Err(error) = self.configure(&settings) { self.log(1, &format!("luanalysis: settings: {error}")); }
        }
        self.initialized = true;
        Json::object([
            ("capabilities", Json::object([
                ("positionEncoding", Json::from(match self.encoding { Encoding::Utf8 => "utf-8", Encoding::Utf16 => "utf-16" })),
                ("textDocumentSync", Json::object([
                    ("openClose", Json::from(true)),
                    ("change", Json::from(2usize)),
                    ("save", Json::object([("includeText", Json::from(false))])),
                ])),
                ("hoverProvider", Json::from(true)),
                ("definitionProvider", Json::from(true)),
                ("typeDefinitionProvider", Json::from(true)),
                ("referencesProvider", Json::from(true)),
                ("documentHighlightProvider", Json::from(true)),
                ("documentSymbolProvider", Json::from(true)),
                ("workspaceSymbolProvider", Json::from(true)),
                ("foldingRangeProvider", Json::from(true)),
                ("inlayHintProvider", Json::from(true)),
                ("codeActionProvider", Json::object([("codeActionKinds", Json::Array(vec![Json::from("quickfix")]))])),
                ("callHierarchyProvider", Json::from(true)),
                ("workspace", Json::object([("workspaceFolders", Json::object([
                    ("supported", Json::from(true)),
                    ("changeNotifications", Json::from(true)),
                ]))])),
                ("typeHierarchyProvider", Json::from(true)),
                ("semanticTokensProvider", Json::object([
                    ("legend", Json::object([
                        ("tokenTypes", Json::Array(["class", "interface", "type", "typeParameter", "parameter", "variable", "property", "function", "method"].into_iter().map(Json::from).collect())),
                        ("tokenModifiers", Json::Array(["declaration", "readonly", "defaultLibrary", "global"].into_iter().map(Json::from).collect())),
                    ])),
                    ("full", Json::from(true)),
                ])),
                ("selectionRangeProvider", Json::from(true)),
                ("documentLinkProvider", Json::object([("resolveProvider", Json::from(false))])),
                ("renameProvider", Json::object([("prepareProvider", Json::from(true))])),
                ("completionProvider", Json::object([
                    ("triggerCharacters", Json::Array(vec![Json::from("."), Json::from(":"), Json::from("@")])),
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
        if self.requested == Some(self.revision) { self.end_progress(); }
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

    /// The outline of a document's current text.
    fn document_symbols(&mut self, params: &Json) -> Json {
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else { return Json::Null };
        let Some(text) = self.file_text(&path) else { return Json::Null };
        let lines = LineIndex::new(text.clone());
        let encoding = self.encoding;
        fn convert(symbol: &language_service::DocumentSymbol, lines: &LineIndex, encoding: Encoding) -> Json {
            let mut fields = vec![
                ("name".to_owned(), Json::from(symbol.name.as_str())),
                ("kind".to_owned(), Json::from(symbol_kind(symbol.kind))),
                ("range".to_owned(), lines.range(symbol.range.start, symbol.range.end, encoding)),
                ("selectionRange".to_owned(), lines.range(symbol.selection.start, symbol.selection.end, encoding)),
            ];
            if let Some(detail) = &symbol.detail { fields.push(("detail".to_owned(), Json::from(detail.as_str()))); }
            if !symbol.children.is_empty() {
                fields.push(("children".to_owned(), Json::Array(symbol.children.iter().map(|child| convert(child, lines, encoding)).collect())));
            }
            Json::Object(fields)
        }
        Json::Array(self.outline(&path, text).iter().map(|symbol| convert(symbol, &lines, encoding)).collect())
    }

    /// The symbols of every checked or open file whose names contain the query's characters in
    /// order, ignoring case, as editors match them.
    fn workspace_symbols(&mut self, params: &Json) -> Json {
        let query: Vec<char> = params.get("query").and_then(Json::as_str).unwrap_or_default().to_lowercase().chars().filter(|ch| !ch.is_whitespace()).collect();
        let mut paths: Vec<PathBuf> = self.documents.keys().cloned().collect();
        if let Some(snapshot) = &self.snapshot {
            paths.extend(snapshot.files.keys().filter(|path| !self.documents.contains_key(*path)).cloned());
        }
        paths.sort();
        let matches = |name: &str| {
            let mut wanted = query.iter().peekable();
            for ch in name.to_lowercase().chars() {
                if wanted.peek() == Some(&&ch) { wanted.next(); }
            }
            wanted.peek().is_none()
        };
        let mut results = Vec::new();
        for path in paths {
            let Some(text) = self.file_text(&path) else { continue };
            let lines = LineIndex::new(text.clone());
            let mut pending: Vec<(&language_service::DocumentSymbol, Option<String>)> = Vec::new();
            let outline = self.outline(&path, text).clone();
            pending.extend(outline.iter().map(|symbol| (symbol, None)));
            while let Some((symbol, container)) = pending.pop() {
                pending.extend(symbol.children.iter().map(|child| (child, Some(symbol.name.clone()))));
                if !matches(&symbol.name) { continue; }
                let mut fields = vec![
                    ("name".to_owned(), Json::from(symbol.name.as_str())),
                    ("kind".to_owned(), Json::from(symbol_kind(symbol.kind))),
                    ("location".to_owned(), Json::object([
                        ("uri", Json::from(path_to_uri(&path))),
                        ("range", lines.range(symbol.selection.start, symbol.selection.end, self.encoding)),
                    ])),
                ];
                if let Some(container) = container { fields.push(("containerName".to_owned(), Json::from(container))); }
                results.push(Json::Object(fields));
                if results.len() >= 500 { return Json::Array(results); }
            }
        }
        Json::Array(results)
    }

    /// Inlay hints for a range of a document: the types of its locals declared without one, from
    /// the last check while the document is as it was checked, and the parameter names of its
    /// calls' literal arguments.
    fn inlay_hints(&mut self, id: Json, params: &Json) {
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else {
            return self.respond(id, Json::Null);
        };
        let Some(text) = self.file_text(&path) else { return self.respond(id, Json::Null) };
        let lines = LineIndex::new(text.clone());
        let position = |name: &str| {
            let line = params.path(&["range", name, "line"]).and_then(Json::as_u64).unwrap_or(0) as usize;
            let character = params.path(&["range", name, "character"]).and_then(Json::as_u64).unwrap_or(0) as usize;
            lines.offset(line, character, self.encoding)
        };
        let range = position("start")..position("end");
        let mut types = Vec::new();
        if self.preferences.type_hints && let Some(file) = self.snapshot.as_ref().and_then(|snapshot| snapshot.files.get(&path)).filter(|file| file.lines.text == text) {
            for span in language_service::untyped_declarations(&text) {
                if !range.contains(&span.start) { continue; }
                let Some(binding) = file.bindings.iter().find(|binding| binding.start == span.start && binding.end == span.end) else { continue };
                let ty = if binding.ty.chars().count() > 60 { format!("{}…", binding.ty.chars().take(59).collect::<String>()) } else { binding.ty.clone() };
                types.push(Json::object([
                    ("position", lines.position(span.end, self.encoding)),
                    ("label", Json::from(format!(": {ty}"))),
                    ("kind", Json::from(1usize)),
                ]));
            }
        }
        if !self.preferences.parameter_hints { return self.respond(id, Json::Array(types)); }
        self.query(id, path, Site::Hints(HintSite { text, range, types }), lines);
    }

    /// A call or type hierarchy: prepared at a document's position, or followed from an item that
    /// a preparation returned, by its name's position.
    fn hierarchy(&mut self, id: Json, params: &Json, request: Hierarchy) {
        let (uri, position) = match params.get("item") {
            Some(item) => (item.get("uri"), item.path(&["selectionRange", "start"])),
            None => (params.path(&["textDocument", "uri"]), params.get("position")),
        };
        let Some(path) = uri.and_then(Json::as_str).and_then(uri_to_path) else { return self.respond(id, Json::Null) };
        let Some(text) = self.file_text(&path) else { return self.respond(id, Json::Null) };
        let lines = LineIndex::new(text.clone());
        let line = position.and_then(|position| position.get("line")).and_then(Json::as_u64).unwrap_or(0) as usize;
        let character = position.and_then(|position| position.get("character")).and_then(Json::as_u64).unwrap_or(0) as usize;
        let offset = lines.offset(line, character, self.encoding);
        self.query(id, path, Site::Hierarchy(HierarchySite { text, offset, request }), lines);
    }

    /// Quick fixes for the diagnostics the editor gives: an unused name (TS6133) is prefixed with
    /// `_`, and a misspelled member or name is changed to the nearest one.
    fn code_actions(&mut self, id: Json, params: &Json) {
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else {
            return self.respond(id, Json::Null);
        };
        let Some(text) = self.file_text(&path) else { return self.respond(id, Json::Null) };
        let lines = LineIndex::new(text.clone());
        let uri = path_to_uri(&path);
        let offset = |position: Option<&Json>| {
            let line = position.and_then(|position| position.get("line")).and_then(Json::as_u64).unwrap_or(0) as usize;
            let character = position.and_then(|position| position.get("character")).and_then(Json::as_u64).unwrap_or(0) as usize;
            lines.offset(line, character, self.encoding)
        };
        let mut actions = Vec::new();
        let mut diagnostics = Vec::new();
        for diagnostic in params.path(&["context", "diagnostics"]).and_then(Json::as_array).unwrap_or(&[]) {
            let code = diagnostic.get("code").and_then(Json::as_str).unwrap_or_default().to_owned();
            let span = luanalysis_syntax::Span {
                start: offset(diagnostic.path(&["range", "start"])),
                end: offset(diagnostic.path(&["range", "end"])),
            };
            let name = &text[span.start.min(text.len())..span.end.min(text.len())];
            match code.as_str() {
                "TS6133" if name.starts_with(|ch: char| ch.is_alphabetic()) && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') => {
                    actions.push(quick_fix(format!("Prefix '{name}' with '_' to mark it unused"), diagnostic, &uri,
                        lines.range(span.start, span.start, self.encoding), "_".to_owned()));
                }
                "TS2339" | "TS2304" => diagnostics.push((diagnostic.clone(), code, span)),
                _ => {}
            }
        }
        if diagnostics.is_empty() { return self.respond(id, Json::Array(actions)); }
        self.query(id, path, Site::Actions(ActionSite { text, actions, diagnostics }), lines);
    }

    /// A document's semantic tokens, relative to one another as LSP encodes them.
    fn semantic_tokens(&self, params: &Json) -> Json {
        use language_service::TokenType;
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else { return Json::Null };
        let Some(text) = self.file_text(&path) else { return Json::Null };
        let lines = LineIndex::new(text.clone());
        let units = |slice: &str| match self.encoding { Encoding::Utf8 => slice.len(), Encoding::Utf16 => slice.encode_utf16().count() };
        let mut data = Vec::new();
        let (mut previous_line, mut previous_start) = (0, 0);
        for token in language_service::semantic_tokens(&text) {
            let line = lines.line(token.span.start);
            let start = units(&text[lines.line_starts[line]..token.span.start]);
            let delta_start = if line == previous_line { start - previous_start } else { start };
            let kind = match token.kind {
                TokenType::Class => 0usize, TokenType::Interface => 1, TokenType::Type => 2, TokenType::TypeParameter => 3,
                TokenType::Parameter => 4, TokenType::Variable => 5, TokenType::Property => 6, TokenType::Function => 7, TokenType::Method => 8,
            };
            data.extend([line - previous_line, delta_start, units(&text[token.span.start..token.span.end]), kind, token.modifiers as usize].map(Json::from));
            (previous_line, previous_start) = (line, start);
        }
        Json::object([("data", Json::Array(data))])
    }

    /// The ranges a document folds: a fold hides the lines after its first, up to the line before
    /// its last, so a block's `end` (or a branch's `else`) stays visible.
    fn folding_ranges(&self, params: &Json) -> Json {
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else { return Json::Null };
        let Some(text) = self.file_text(&path) else { return Json::Null };
        let lines = LineIndex::new(text.clone());
        Json::Array(language_service::folding_ranges(&text).into_iter().filter_map(|fold| {
            let start = lines.line(fold.start);
            let last = lines.line(fold.end.saturating_sub(1).max(fold.start));
            // A comment or region folds whole; a block keeps its closing line.
            let end = if fold.kind.is_some() { last } else { last.saturating_sub(1) };
            (end > start).then(|| {
                let mut fields = vec![("startLine".to_owned(), Json::from(start)), ("endLine".to_owned(), Json::from(end))];
                match fold.kind {
                    Some(language_service::FoldKind::Comment) => fields.push(("kind".to_owned(), Json::from("comment"))),
                    Some(language_service::FoldKind::Region) => fields.push(("kind".to_owned(), Json::from("region"))),
                    None => {}
                }
                Json::Object(fields)
            })
        }).collect())
    }

    /// For each position, the ranges a selection grows through, innermost first.
    fn selection_ranges(&self, params: &Json) -> Json {
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else { return Json::Null };
        let Some(text) = self.file_text(&path) else { return Json::Null };
        let lines = LineIndex::new(text.clone());
        let positions = params.get("positions").and_then(Json::as_array).unwrap_or(&[]);
        let offsets: Vec<usize> = positions.iter().map(|position| {
            let line = position.get("line").and_then(Json::as_u64).unwrap_or(0) as usize;
            let character = position.get("character").and_then(Json::as_u64).unwrap_or(0) as usize;
            lines.offset(line, character, self.encoding)
        }).collect();
        Json::Array(language_service::selection_ranges(&text, &offsets).into_iter().zip(&offsets).map(|(chain, &offset)| {
            let mut range: Option<Json> = None;
            for span in chain.iter().rev() {
                let mut fields = vec![("range".to_owned(), lines.range(span.start, span.end, self.encoding))];
                if let Some(parent) = range.take() { fields.push(("parent".to_owned(), parent)); }
                range = Some(Json::Object(fields));
            }
            range.unwrap_or_else(|| Json::object([("range", lines.range(offset, offset, self.encoding))]))
        }).collect())
    }

    /// Each resolved `require` of a document, as a link to the module's file, while the document
    /// is as it was checked.
    fn document_links(&self, params: &Json) -> Json {
        let Some(path) = params.path(&["textDocument", "uri"]).and_then(Json::as_str).and_then(uri_to_path) else { return Json::Null };
        let Some(file) = self.snapshot.as_ref().and_then(|snapshot| snapshot.files.get(&path)) else { return Json::Null };
        if self.documents.get(&path).is_some_and(|document| document.text != file.lines.text) { return Json::Array(Vec::new()); }
        Json::Array(file.requires.iter().map(|require| Json::object([
            ("range", file.range(require.start, require.end, self.encoding)),
            ("target", Json::from(path_to_uri(&require.target))),
        ])).collect())
    }

    /// A file's text: its open document's, or the last check's.
    fn file_text(&self, path: &Path) -> Option<String> {
        self.documents.get(path).map(|document| document.text.clone())
            .or_else(|| self.snapshot.as_ref()?.files.get(path).map(|file| file.lines.text.clone()))
    }

    /// A file's outline, made again only when its text changes.
    fn outline(&mut self, path: &Path, text: String) -> &Vec<language_service::DocumentSymbol> {
        let stale = self.symbols.get(path).is_none_or(|(made, _)| *made != text);
        if stale {
            let symbols = language_service::document_symbols(&text);
            self.symbols.insert(path.to_path_buf(), (text, symbols));
        }
        &self.symbols[path].1
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
        let query = Query { id: id.clone(), path: path.clone(), overlays: HashMap::new(), disk: self.disk_generation, site, lines, encoding: self.encoding, snippets: self.snippets && self.preferences.call_snippets };
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

fn answer_queries(inputs: CheckInputs, options: ProjectCheckOptions, queries: mpsc::Receiver<Query>, events: mpsc::Sender<Event>, cancelled: Arc<std::sync::Mutex<HashSet<String>>>) {
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
            // A query the client cancelled before it began is not answered.
            if cancelled.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&query.id.to_string()) {
                if events.send(Event::Cancelled(query.id.clone())).is_err() { return; }
                continue;
            }
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
                Site::Hints(site) => hints(&inputs, options.clone(), &mut workspace, &mut loaded, &query, site, &mut answer),
                Site::Actions(site) => actions(&inputs, options.clone(), &mut workspace, &mut loaded, &query, site, &mut answer),
                Site::Hierarchy(site) => hierarchy(&inputs, options.clone(), &mut workspace, &mut loaded, &query, site, &mut answer),
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
    let called = query.lines.text[site.word.end..].trim_start_matches([' ', '\t']).starts_with(['(', '"', '\'', '{']);
    language_service::complete(project, options, workspace, file, site, |items| answer(Json::object([("isIncomplete", Json::from(false)), ("items", Json::Array(items.into_iter().map(|item| {
        let mut fields = vec![
            ("label".to_owned(), Json::from(item.label.as_str())),
            ("kind".to_owned(), Json::from(match item.kind {
                CompletionKind::Method => 2usize,
                CompletionKind::Function => 3,
                CompletionKind::Field => 5,
                CompletionKind::Variable => 6,
                CompletionKind::Keyword => 14,
                CompletionKind::Class => 7,
                CompletionKind::TypeParameter => 25,
            })),
            ("textEdit".to_owned(), Json::object([("range", range.clone()), ("newText", Json::from(item.label.as_str()))])),
        ];
        // A callable inserts a call with its required parameters as placeholders, unless a call
        // already follows.
        if let Some((parameters, more)) = item.parameters.filter(|_| query.snippets && !called) {
            let arguments: Vec<String> = parameters.iter().enumerate()
                .map(|(index, name)| format!("${{{}:{}}}", index + 1, name.replace(['$', '}', '\\'], "")))
                .collect();
            // With no required parameter, the cursor goes between the parentheses if there are
            // optional ones to type, and after them if there are none.
            let snippet = match (arguments.is_empty(), more) {
                (true, true) => format!("{}($0)", item.label),
                (true, false) => format!("{}()$0", item.label),
                (false, _) => format!("{}({})$0", item.label, arguments.join(", ")),
            };
            fields[2] = ("textEdit".to_owned(), Json::object([("range", range.clone()), ("newText", Json::from(snippet))]));
            fields.push(("insertTextFormat".to_owned(), Json::from(2usize)));
        }
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
    let references = !matches!(site.request, Navigate::Definition | Navigate::TypeDefinition | Navigate::PrepareRename);
    let encoding = query.encoding;
    let respond = |navigation: Option<language_service::Navigation>| {
        let Some(navigation) = navigation else { return answer(Json::Null) };
        let lines: HashMap<&PathBuf, LineIndex> = navigation.texts.iter().map(|(path, text)| (path, LineIndex::new(text.clone()))).collect();
        let range = |path: &PathBuf, span: luanalysis_syntax::Span| lines.get(path).map(|lines| lines.range(span.start, span.end, encoding));
        let location = |(path, span, _): &(PathBuf, luanalysis_syntax::Span, bool)| {
            Some(Json::object([("uri", Json::from(path_to_uri(path))), ("range", range(path, *span)?)]))
        };
        answer(match &site.request {
            Navigate::Definition | Navigate::TypeDefinition => Json::Array(navigation.occurrences.iter().filter(|occurrence| occurrence.2).filter_map(location).collect()),
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
    };
    match site.request {
        Navigate::TypeDefinition => language_service::type_definition(project, options, workspace, file, site.offset, respond),
        _ => language_service::navigate(project, options, workspace, file, site.offset, references, respond),
    }
}

/// An outline symbol's LSP `SymbolKind`.
fn symbol_kind(kind: language_service::SymbolKind) -> usize {
    use language_service::SymbolKind;
    match kind {
        SymbolKind::Module => 2,
        SymbolKind::Class => 5,
        SymbolKind::Method => 6,
        SymbolKind::Field => 8,
        SymbolKind::Interface => 11,
        SymbolKind::Function => 12,
        SymbolKind::Variable => 13,
        SymbolKind::Constant => 14,
        SymbolKind::Alias => 26,
    }
}

fn hints(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, loaded: &mut Option<LoadedProject>, query: &Query, site: &HintSite, answer: &mut (dyn FnMut(Json) + Send)) {
    let Some(project) = load(inputs, workspace, loaded, query) else { return };
    let Some(file) = project.file_by_path(&query.path).map(|file| file.id) else { return };
    language_service::parameter_hints(project, options, workspace, file, site.range.clone(), |names| {
        let mut hints = site.types.clone();
        hints.extend(names.into_iter().map(|(offset, name)| Json::object([
            ("position", query.lines.position(offset, query.encoding)),
            ("label", Json::from(format!("{name}:"))),
            ("kind", Json::from(2usize)),
            ("paddingRight", Json::from(true)),
        ])));
        answer(Json::Array(hints));
    });
}

fn hierarchy(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, loaded: &mut Option<LoadedProject>, query: &Query, site: &HierarchySite, answer: &mut (dyn FnMut(Json) + Send)) {
    let Some(project) = load(inputs, workspace, loaded, query) else { return };
    let Some(file) = project.file_by_path(&query.path).map(|file| file.id) else { return };
    let encoding = query.encoding;
    let item = |name: &str, kind: language_service::SymbolKind, path: &PathBuf, span: luanalysis_syntax::Span, selection: luanalysis_syntax::Span, lines: &LineIndex| Json::object([
        ("name", Json::from(name)),
        ("kind", Json::from(symbol_kind(kind))),
        ("uri", Json::from(path_to_uri(path))),
        ("range", lines.range(span.start, span.end, encoding)),
        ("selectionRange", lines.range(selection.start, selection.end, encoding)),
    ]);
    match site.request {
        Hierarchy::Calls(direction) => language_service::call_hierarchy(project, options, workspace, file, site.offset, direction, |hierarchy| {
            let Some(hierarchy) = hierarchy else { return answer(Json::Null) };
            let lines: HashMap<&PathBuf, LineIndex> = hierarchy.texts.iter().map(|(path, text)| (path, LineIndex::new(text.clone()))).collect();
            let call_item = |call: &language_service::CallItem| lines.get(&call.path).map(|lines| item(&call.name, call.kind, &call.path, call.span, call.selection, lines));
            answer(match direction {
                language_service::CallDirection::Prepare => Json::Array(hierarchy.item.iter().filter_map(call_item).collect()),
                _ => {
                    let target = hierarchy.item.as_ref().map(|item| item.path.clone());
                    Json::Array(hierarchy.calls.iter().filter_map(|(other, spans)| {
                        // Incoming calls are in the caller's file, outgoing ones in the target's.
                        let calls_in = if direction == language_service::CallDirection::Incoming { Some(&other.path) } else { target.as_ref() };
                        let lines = lines.get(calls_in?)?;
                        let ranges = Json::Array(spans.iter().map(|span| lines.range(span.start, span.end, encoding)).collect());
                        let key = if direction == language_service::CallDirection::Incoming { "from" } else { "to" };
                        Some(Json::object([(key, call_item(other)?), ("fromRanges", ranges)]))
                    }).collect())
                }
            });
        }),
        Hierarchy::Types(direction) => language_service::type_hierarchy(project, options, workspace, file, site.offset, direction, |related| {
            let Some((items, texts)) = related else { return answer(Json::Null) };
            let lines: HashMap<&PathBuf, LineIndex> = texts.iter().map(|(path, text)| (path, LineIndex::new(text.clone()))).collect();
            answer(Json::Array(items.iter().filter_map(|related| {
                Some(item(&related.name, related.kind, &related.path, related.span, related.selection, lines.get(&related.path)?))
            }).collect()));
        }),
    }
}

/// A quick fix that replaces `range` of the document at `uri`, resolving `diagnostic`.
fn quick_fix(title: String, diagnostic: &Json, uri: &str, range: Json, text: String) -> Json {
    Json::object([
        ("title", Json::from(title)),
        ("kind", Json::from("quickfix")),
        ("diagnostics", Json::Array(vec![diagnostic.clone()])),
        ("isPreferred", Json::from(true)),
        ("edit", Json::object([("changes", Json::Object(vec![(uri.to_owned(), Json::Array(vec![Json::object([("range", range), ("newText", Json::from(text))])]))]))])),
    ])
}

fn actions(inputs: &CheckInputs, options: ProjectCheckOptions, workspace: &mut ProjectWorkspace, loaded: &mut Option<LoadedProject>, query: &Query, site: &ActionSite, answer: &mut (dyn FnMut(Json) + Send)) {
    let Some(project) = load(inputs, workspace, loaded, query) else { return };
    let Some(file) = project.file_by_path(&query.path).map(|file| file.id) else { return };
    let requested = site.diagnostics.iter().map(|(_, code, span)| (code.clone(), *span)).collect();
    let uri = path_to_uri(&query.path);
    language_service::spelling_fixes(project, options, workspace, file, requested, |fixes| {
        let mut actions = site.actions.clone();
        actions.extend(fixes.into_iter().map(|fix| quick_fix(fix.title, &site.diagnostics[fix.diagnostic].0, &uri,
            query.lines.range(fix.span.start, fix.span.end, query.encoding), fix.replacement)));
        answer(Json::Array(actions));
    });
}

/// What the client's settings ask of the editor's features.
#[derive(Clone, Copy)]
struct Preferences {
    type_hints: bool,
    parameter_hints: bool,
    call_snippets: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { type_hints: true, parameter_hints: true, call_snippets: true }
    }
}

/// The checked paths and options with the client's settings applied, as the command line's
/// options would apply them: `luaVersion`, `libraries`, `moduleRoots`, `packagePath`, `stdlib`,
/// `noStdlib` and `noImplicitAny`; and the editor preferences `inlayHints.types`,
/// `inlayHints.parameterNames` and `completion.callSnippets`. A relative path is the workspace's.
fn apply_settings(mut inputs: CheckInputs, mut options: ProjectCheckOptions, settings: &Json) -> Result<(CheckInputs, ProjectCheckOptions, Preferences), String> {
    let root = inputs.files.iter().find(|file| file.is_dir()).cloned();
    let strings = |name: &str| -> Vec<String> {
        match settings.get(name) {
            Some(Json::Array(values)) => values.iter().filter_map(Json::as_str).map(str::to_owned).collect(),
            Some(value) => value.as_str().map(|value| value.split(',').map(|part| part.trim().to_owned()).filter(|part| !part.is_empty()).collect()).unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let path = |value: String| {
        let path = PathBuf::from(value);
        match &root { Some(root) if path.is_relative() => root.join(path), _ => path }
    };
    let flag = |path: &[&str]| settings.path(path).and_then(|value| match value { Json::Bool(value) => Some(*value), _ => None });
    if let Some(version) = settings.get("luaVersion").and_then(Json::as_str) {
        options.environment = crate::cli::lua_version(version)
            .ok_or_else(|| format!("luaVersion {version:?} is not one of 5.0, 5.1, 5.2, 5.3 and 5.4"))?;
    }
    inputs.libraries.extend(strings("libraries").into_iter().map(path));
    inputs.module_roots.extend(strings("moduleRoots").into_iter().map(path));
    if let Some(package_path) = settings.get("packagePath").and_then(Json::as_str) { inputs.package_path = Some(package_path.to_owned()); }
    let stdlib = strings("stdlib");
    if !stdlib.is_empty() {
        options.standard_library = Some(crate::standard_environment::StandardLibraryAllowList::parse(&stdlib.join(",")));
    } else if flag(&["noStdlib"]) == Some(true) {
        options.standard_library = Some(crate::standard_environment::StandardLibraryAllowList::default());
    }
    // A selected standard library is a whole Lua's, as on the command line.
    if options.standard_library.is_some() && !options.environment.is_full_lua() {
        options.environment = crate::standard_environment::StandardEnvironmentProfile::Lua54;
    }
    if let Some(list) = &options.standard_library { list.validate(options.environment).map_err(|error| format!("stdlib: {error}"))?; }
    options.loader_policy = crate::cli::loader_policy(options.environment);
    if let Some(enabled) = flag(&["noImplicitAny"]) { options.no_implicit_any = enabled; }
    let preferences = Preferences {
        type_hints: flag(&["inlayHints", "types"]).unwrap_or(true),
        parameter_hints: flag(&["inlayHints", "parameterNames"]).unwrap_or(true),
        call_snippets: flag(&["completion", "callSnippets"]).unwrap_or(true),
    };
    Ok((inputs, options, preferences))
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

    /// The zero-based line of an offset.
    fn line(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= self.floor(offset)) - 1
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
