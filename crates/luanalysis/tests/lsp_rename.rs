//! Rename must use the editor's current buffers even before their next project check.
use luanalysis::{
    cli::CheckInputs,
    lsp::LanguageServer,
    project_check::ProjectCheckOptions,
    standard_environment::StandardEnvironmentProfile,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "luanalysis lsp rename {} {}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn uri(path: &Path) -> String {
    let mut path = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        if let Some(rest) = path.strip_prefix(r"\\?\") { path = rest.to_owned(); }
        path = path.replace('\\', "/");
        if path.as_bytes().get(1) == Some(&b':') { path.insert(0, '/'); }
    }
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")
}

struct DiagnosticGate {
    uri: String,
    version: u32,
    resume: mpsc::Receiver<()>,
}

fn diagnostics_match(message: &str, uri: &str, version: u32) -> bool {
    message.contains(r#""method":"textDocument/publishDiagnostics""#)
        && message.contains(&format!(r#""uri":"{uri}""#))
        && message.contains(&format!(r#""version":{version}"#))
}

impl DiagnosticGate {
    fn accepts(&self, message: &str) -> bool {
        diagnostics_match(message, &self.uri, self.version)
    }
}

/// Also releases the server when an assertion unwinds, so dropping it cannot deadlock.
struct Resume(mpsc::Sender<()>);

impl Drop for Resume {
    fn drop(&mut self) { let _ = self.0.send(()); }
}

struct Server {
    server: Option<LanguageServer>,
    messages: mpsc::Receiver<String>,
    gate: Arc<Mutex<Option<DiagnosticGate>>>,
}

impl Server {
    fn start(root: &Path) -> Self {
        let inputs = CheckInputs {
            files: vec![root.to_path_buf()], libraries: Vec::new(), module_roots: Vec::new(),
            package_path: None, module_map: None,
        };
        let gate = Arc::new(Mutex::new(None::<DiagnosticGate>));
        let output_gate = Arc::clone(&gate);
        let (sender, messages) = mpsc::channel();
        let server = LanguageServer::start(inputs, ProjectCheckOptions {
            jobs: 2, environment: StandardEnvironmentProfile::Lua54, ..ProjectCheckOptions::default()
        }, move |message| {
            let resume = {
                let mut gate = output_gate.lock().unwrap();
                if gate.as_ref().is_some_and(|gate| gate.accepts(message)) {
                    gate.take().map(|gate| gate.resume)
                } else { None }
            };
            let _ = sender.send(message.to_owned());
            if let Some(resume) = resume { let _ = resume.recv(); }
        });
        let server = Self { server: Some(server), messages, gate };
        server.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":null,"capabilities":{}}}"#);
        let initialized = server.response(1);
        assert!(initialized.contains(r#""renameProvider":{"prepareProvider":true}"#), "{initialized}");
        server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        server
    }

    fn send(&self, message: &str) {
        assert!(self.server.as_ref().unwrap().send(message), "server stopped before {message}");
    }

    fn wait(&self, accept: impl Fn(&str) -> bool) -> String {
        loop {
            let message = self.messages.recv_timeout(Duration::from_secs(60)).expect("a language server message");
            if accept(&message) { return message; }
        }
    }

    fn response(&self, id: u32) -> String {
        self.wait(|message| message.contains(&format!(r#""id":{id},"#)))
    }

    fn open(&self, path: &Path, text: &str) {
        self.open_uri(&uri(path), text);
    }

    fn open_uri(&self, uri: &str, text: &str) {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"lua","version":1,"text":"{}"}}}}}}"#,
            uri, escape(text),
        ));
    }

    fn change(&self, path: &Path, version: u32, text: &str) {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{}","version":{version}}},"contentChanges":[{{"text":"{}"}}]}}}}"#,
            uri(path), escape(text),
        ));
    }

    fn rename(&self, id: u32, path: &Path, text: &str, name: &str, nth: usize, new_name: &str) {
        let offset = text.match_indices(name).nth(nth).unwrap().0;
        let line = text[..offset].matches('\n').count();
        let start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
        let character = text[start..offset].encode_utf16().count();
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/rename","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{character}}},"newName":"{}"}}}}"#,
            uri(path), escape(new_name),
        ));
    }

    fn diagnostics(&self, path: &Path, version: u32) -> String {
        let uri = uri(path);
        self.wait(|message| diagnostics_match(message, &uri, version))
    }

    /// Stop inside publication of the given check's diagnostics. Once they arrive, the client
    /// can enqueue changes and rename before releasing the server. It then installs that
    /// snapshot and drains those queued messages before scheduling another check, so these
    /// regressions cannot accidentally pass because the checker won a scheduling race.
    fn hold_diagnostics(&self, path: &Path, version: u32) -> Resume {
        let (resume, wait) = mpsc::channel();
        let mut gate = self.gate.lock().unwrap();
        assert!(gate.is_none());
        *gate = Some(DiagnosticGate { uri: uri(path), version, resume: wait });
        Resume(resume)
    }

    fn finish(mut self) {
        self.send(r#"{"jsonrpc":"2.0","id":99,"method":"shutdown"}"#);
        let stopped = self.response(99);
        assert!(stopped.contains(r#""result":null"#), "{stopped}");
        self.send(r#"{"jsonrpc":"2.0","method":"exit"}"#);
        assert_eq!(self.server.take().unwrap().wait(), 0);
    }
}

#[test]
fn inline_preview_restoration_can_be_renamed_before_its_check() {
    let fixture = Fixture::new();
    let text = "---@param name string\n---@return string\nlocal function greet(name) return name end\nlocal message = greet(\"world\")\nreturn greet(message)\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    server.diagnostics(&main, 1);

    let preview = text.replace("local message = greet", "local message = welcome");
    let resume = server.hold_diagnostics(&main, 2);
    server.change(&main, 2, &preview);
    let checked = server.diagnostics(&main, 2);
    assert!(checked.contains("TS2304"), "{checked}");
    server.change(&main, 3, text);
    server.rename(2, &main, text, "greet", 1, "welcome");
    drop(resume);

    let renamed = server.response(2);
    assert!(!renamed.contains(r#""error""#), "{renamed}");
    assert_eq!(renamed.matches(r#""newText":"welcome""#).count(), 3, "{renamed}");
    assert!(renamed.contains(r#""range":{"start":{"line":3,"character":16},"end":{"line":3,"character":21}}"#), "{renamed}");
    server.finish();
}

#[test]
fn rename_moves_an_occurrence_in_another_unsaved_open_file() {
    let fixture = Fixture::new();
    let definition = "Counter = 0\n";
    let util = fixture.file("util.lua", definition);
    let text = "require('util')\nreturn Counter\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    // Percent-encoding an ordinary letter is equivalent for loading, but the edit must use
    // the URI the client opened, as the snapshot rename path already does.
    let util_uri = uri(&util).replace("util.lua", "%75til.lua");
    server.open_uri(&util_uri, definition);
    server.open(&main, text);
    server.diagnostics(&main, 1);
    server.change(&util, 2, &format!("-- unsaved line\n{definition}"));
    server.rename(2, &main, text, "Counter", 0, "Total");
    drop(resume);

    let renamed = server.response(2);
    assert!(!renamed.contains(r#""error""#), "{renamed}");
    assert_eq!(renamed.matches(r#""newText":"Total""#).count(), 2, "{renamed}");
    assert!(renamed.contains(&util_uri) && renamed.contains(&uri(&main)), "{renamed}");
    assert!(renamed.contains(r#""range":{"start":{"line":1,"character":0},"end":{"line":1,"character":7}}"#), "{renamed}");
    server.finish();
}

#[test]
fn current_overlay_rename_refuses_a_new_local_capture() {
    let fixture = Fixture::new();
    let text = "local first = 1\nprint(first)\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open(&main, text);
    server.diagnostics(&main, 1);
    let edited = "local first = 1\nlocal second = 2\nprint(first, second)\n";
    server.change(&main, 2, edited);
    server.rename(2, &main, edited, "first", 0, "second");
    drop(resume);

    let refused = server.response(2);
    assert!(refused.contains("-32803") && refused.contains("would change what the name at"), "{refused}");
    server.finish();
}

#[test]
fn current_overlay_rename_refuses_a_global_added_in_another_open_file() {
    let fixture = Fixture::new();
    let definition = "Shared = 1\n";
    let util = fixture.file("util.lua", definition);
    let text = "require('util')\nreturn Shared\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open(&util, definition);
    server.open(&main, text);
    server.diagnostics(&main, 1);
    server.change(&util, 2, "Shared = 1\nOther = 2\n");
    server.rename(2, &main, text, "Shared", 0, "Other");
    drop(resume);

    let refused = server.response(2);
    assert!(refused.contains("-32803") && refused.contains("a global named Other already exists"), "{refused}");
    server.finish();
}

#[test]
fn current_overlay_rename_refuses_capture_by_a_local_added_in_another_open_file() {
    let fixture = Fixture::new();
    let definition = "Shared = 1\nreturn Shared\n";
    let util = fixture.file("util.lua", definition);
    let text = "require('util')\nreturn Shared\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open(&util, definition);
    server.open(&main, text);
    server.diagnostics(&main, 1);
    server.change(&util, 2, "local total = 0\nShared = 1\nreturn Shared + total\n");
    server.rename(2, &main, text, "Shared", 0, "total");
    drop(resume);

    let refused = server.response(2);
    assert!(refused.contains("-32803") && refused.contains("would change what the name at"), "{refused}");
    server.finish();
}

#[test]
fn current_overlay_rename_preserves_library_ownership_checks() {
    let fixture = Fixture::new();
    let text = "return print\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open(&main, text);
    server.diagnostics(&main, 1);
    let edited = format!("-- unsaved line\n{text}");
    server.change(&main, 2, &edited);
    server.rename(2, &main, &edited, "print", 0, "output");
    drop(resume);

    let refused = server.response(2);
    assert!(refused.contains("-32803") && refused.contains("declared by a library"), "{refused}");
    server.finish();
}

#[test]
fn current_overlay_rename_refuses_lua_keywords() {
    let fixture = Fixture::new();
    let text = "local first = 1\nreturn first\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open(&main, text);
    server.diagnostics(&main, 1);
    let edited = format!("-- unsaved line\n{text}");
    server.change(&main, 2, &edited);
    server.rename(2, &main, &edited, "first", 0, "end");
    drop(resume);

    let refused = server.response(2);
    assert!(refused.contains("-32803") && refused.contains("is not a Lua name"), "{refused}");
    server.finish();
}
