//! `luanalysis lsp` over stdio: diagnostics follow the editor's buffers, and
//! hover and go-to-definition answer at UTF-16 positions.
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn fixture() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "luanalysis lsp server {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir_all(&path).unwrap();
    fs::canonicalize(path).unwrap()
}

fn uri(path: &Path) -> String {
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

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    messages: mpsc::Receiver<String>,
}

impl Server {
    fn start(arguments: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_luanalysis"))
            .arg("lsp")
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || loop {
            let mut length = 0;
            loop {
                let mut line = String::new();
                if stdout.read_line(&mut line).unwrap_or(0) == 0 { return; }
                let line = line.trim_end();
                if line.is_empty() { break; }
                if let Some(value) = line.strip_prefix("Content-Length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            if stdout.read_exact(&mut body).is_err() { return; }
            if sender.send(String::from_utf8(body).unwrap()).is_err() { return; }
        });
        Self { child, stdin, messages }
    }

    fn send(&mut self, message: &str) {
        write!(self.stdin, "Content-Length: {}\r\n\r\n{message}", message.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    /// The next message that satisfies `accept`; others are skipped.
    fn wait(&self, accept: impl Fn(&str) -> bool) -> String {
        loop {
            let message = self.messages.recv_timeout(Duration::from_secs(60)).expect("a language server message");
            if accept(&message) { return message; }
        }
    }

    fn initialize(&mut self, root: &Path) {
        self.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{}","capabilities":{{}}}}}}"#, uri(root)));
        let response = self.wait(|message| message.contains(r#""id":1"#));
        assert!(response.contains(r#""positionEncoding":"utf-16""#), "{response}");
        assert!(response.contains(r#""hoverProvider":true"#) && response.contains(r#""definitionProvider":true"#), "{response}");
        self.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    }

    fn open(&mut self, path: &Path, text: &str) {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"lua","version":1,"text":"{}"}}}}}}"#,
            uri(path), escape(text),
        ));
    }

    fn change(&mut self, path: &Path, version: u32, text: &str) {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{}","version":{version}}},"contentChanges":[{{"text":"{}"}}]}}}}"#,
            uri(path), escape(text),
        ));
    }

    fn diagnostics(&self, path: &Path, accept: impl Fn(&str) -> bool) -> String {
        let uri = uri(path);
        self.wait(|message| message.contains("textDocument/publishDiagnostics") && message.contains(&uri) && accept(message))
    }

    fn request(&mut self, id: u32, method: &str, path: &Path, line: u32, character: u32) -> String {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{character}}}}}}}"#,
            uri(path),
        ));
        self.wait(|message| message.contains(&format!(r#""id":{id},"#)) && !message.contains("\"method\""))
    }

    fn finish(mut self) {
        self.send(r#"{"jsonrpc":"2.0","id":99,"method":"shutdown"}"#);
        let response = self.wait(|message| message.contains(r#""id":99"#));
        assert!(response.contains(r#""result":null"#), "{response}");
        self.send(r#"{"jsonrpc":"2.0","method":"exit"}"#);
        assert!(self.child.wait().unwrap().success());
    }
}

#[test]
fn diagnostics_follow_unsaved_edits_and_clear_when_fixed() {
    let root = fixture();
    let util = root.join("util.lua");
    fs::write(&util, "local M = {}\n---@param x number\nfunction M.double(x) return x * 2 end\nreturn M\n").unwrap();
    let main = root.join("main.lua");
    let wrong = "local util = require('util')\nlocal n = util.double(2)\n---@type string\nlocal s = n\nreturn s\n";
    // The editor's buffer is checked, not the file on disk.
    fs::write(&main, "return nil\n").unwrap();
    let mut server = Server::start(&["--no-stdlib"]);
    server.initialize(&root);
    server.open(&main, wrong);
    let published = server.diagnostics(&main, |message| message.contains("TS2322"));
    assert!(published.contains(r#""range":{"start":{"line":3,"character":10},"end":{"line":3,"character":11}}"#), "{published}");
    assert!(published.contains(r#""version":1"#), "{published}");
    server.change(&main, 2, &wrong.replace("---@type string", "---@type number"));
    let cleared = server.diagnostics(&main, |message| message.contains(r#""diagnostics":[]"#));
    assert!(!cleared.contains("TS2322"), "{cleared}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn hover_and_definition_answer_at_utf16_positions() {
    let root = fixture();
    let util = root.join("util.lua");
    fs::write(&util, "local M = {}\n---@param x number\nfunction M.double(x) return x * 2 end\nreturn M\n").unwrap();
    let main = root.join("main.lua");
    // `n` follows an emoji: two UTF-16 code units, four UTF-8 bytes.
    let text = "local util = require('util')\nlocal s = '\u{1F600}' local n = util.double(2)\nlocal m = n + #s\nreturn m\n";
    fs::write(&main, text).unwrap();
    let mut server = Server::start(&["--no-stdlib"]);
    server.initialize(&root);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    let hover = server.request(2, "textDocument/hover", &main, 1, 21);
    assert!(hover.contains("n: number"), "{hover}");
    assert!(hover.contains(r#""range":{"start":{"line":1,"character":21},"end":{"line":1,"character":22}}"#), "{hover}");
    let local = server.request(3, "textDocument/definition", &main, 2, 10);
    assert!(local.contains(&uri(&main)), "{local}");
    assert!(local.contains(r#""range":{"start":{"line":1,"character":21},"end":{"line":1,"character":22}}"#), "{local}");
    let module = server.request(4, "textDocument/definition", &main, 0, 23);
    assert!(module.contains(&uri(&util)), "{module}");
    let nothing = server.request(5, "textDocument/hover", &main, 40, 0);
    assert!(nothing.contains(r#""result":null"#), "{nothing}");
    // `n` is declared once and read once.
    for (id, include, count) in [(6, true, 2), (7, false, 1)] {
        server.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/references","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":2,"character":10}},"context":{{"includeDeclaration":{include}}}}}}}"#,
            uri(&main),
        ));
        let references = server.wait(|message| message.contains(&format!(r#""id":{id},"#)));
        assert_eq!(references.matches(r#""uri""#).count(), count, "{references}");
        assert!(references.contains(r#""range":{"start":{"line":2,"character":10},"end":{"line":2,"character":11}}"#), "{references}");
    }
    let highlights = server.request(8, "textDocument/documentHighlight", &main, 1, 21);
    assert_eq!(highlights.matches(r#""range""#).count(), 2, "{highlights}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn requests_before_initialize_and_unknown_requests_are_errors() {
    let root = fixture();
    let mut server = Server::start(&["--no-stdlib"]);
    server.send(r#"{"jsonrpc":"2.0","id":7,"method":"textDocument/hover","params":{}}"#);
    let early = server.wait(|message| message.contains(r#""id":7"#));
    assert!(early.contains("-32002"), "{early}");
    server.initialize(&root);
    server.send(r#"{"jsonrpc":"2.0","id":8,"method":"textDocument/rename","params":{}}"#);
    let unknown = server.wait(|message| message.contains(r#""id":8"#));
    assert!(unknown.contains("-32601"), "{unknown}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completions_answer_from_the_unsaved_text_being_typed() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "---@class Point\n---@field x number\n---@field label string\nlocal Point = {}\n\n---@param point Point\nlocal function show(point)\n    return point.x\nend\n\n---@param each fun(point: Point)\nlocal function visit(each) end\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);

    // A member at statement position, which does not parse until it is completed.
    let typed = source.replace("    return point.x\n", "    point.la\n    return point.x\n");
    server.change(&main, 2, &typed);
    let response = server.request(3, "textDocument/completion", &main, 7, 12);
    assert!(response.contains(r#""label":"x""#) && response.contains(r#""label":"label""#), "{response}");
    assert!(response.contains(r#""detail":"string""#), "{response}");
    // The completion replaces the word at the cursor.
    assert!(response.contains(r#""range":{"start":{"line":7,"character":10},"end":{"line":7,"character":12}}"#), "{response}");

    // A callback's parameter has its contextual type.
    let typed = format!("{source}visit(function(point)\n    point.\nend)\n");
    server.change(&main, 3, &typed);
    let response = server.request(4, "textDocument/completion", &main, 13, 10);
    assert!(response.contains(r#""label":"label""#), "{response}");

    // Names: locals in scope, globals and keywords.
    let typed = format!("{source}local count = 1\nprint(co)\n");
    server.change(&main, 4, &typed);
    let response = server.request(5, "textDocument/completion", &main, 13, 8);
    for label in ["count", "show", "visit", "print", "string", "local"] {
        assert!(response.contains(&format!(r#""label":"{label}""#)), "{label}: {response}");
    }
    assert!(!response.contains(r#""label":"point""#), "{response}");

    // String methods after a colon.
    let typed = format!("{source}local name = 'text'\nname:\n");
    server.change(&main, 5, &typed);
    let response = server.request(6, "textDocument/completion", &main, 13, 5);
    assert!(response.contains(r#""label":"upper""#) && response.contains(r#""kind":2"#), "{response}");
    server.finish();
}

#[test]
fn inspections_are_published_as_warnings_and_faded_hints() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "---@param count number\nlocal function show(count)\n    if count == 'none' then print(count) end\n    local unused = 1\nend\nshow(1)\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    let published = server.diagnostics(&main, |message| message.contains("TS6133"));
    assert!(published.contains(r#""severity":2,"code":"TS2367""#), "{published}");
    assert!(published.contains(r#""severity":4,"code":"TS6133""#) && published.contains(r#""tags":[1]"#), "{published}");
    assert!(!published.contains(r#""severity":1"#), "{published}");
    server.finish();
}
