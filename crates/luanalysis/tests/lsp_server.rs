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
    let mut path = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        if let Some(rest) = path.strip_prefix(r"\\?\") { path = rest.to_owned() }
        path = path.replace('\\', "/");
        if path.as_bytes().get(1) == Some(&b':') { path.insert(0, '/') }
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
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    messages: mpsc::Receiver<String>,
}

impl Server {
    fn start(arguments: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_luanalysis-cli"))
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
        // The client can tell which build it is connected to.
        assert!(response.contains(&format!(r#""serverInfo":{{"name":"luanalysis","version":"{}"}}"#, luanalysis::VERSION)), "{response}");
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

    fn rename(&mut self, id: u32, path: &Path, (line, character): (u32, u32), name: &str) -> String {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/rename","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{character}}},"newName":"{name}"}}}}"#,
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
    let mut server = Server::start(&["--no-stdlib", "--stdlib", "require"]);
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
fn a_watching_client_reports_changes_on_disk_and_others_are_rediscovered_at_each_check() {
    for watching in [true, false] {
        let root = fixture();
        let util = root.join("util.lua");
        fs::write(&util, "local M = {}\n---@param x number\nfunction M.double(x) return x * 2 end\nreturn M\n").unwrap();
        let main = root.join("main.lua");
        let text = "local util = require('util')\nreturn util.double('x')\n";
        fs::write(&main, text).unwrap();
        let mut server = Server::start(&["--no-stdlib", "--stdlib", "require"]);
        let capabilities = if watching { r#"{"workspace":{"didChangeWatchedFiles":{"dynamicRegistration":true}}}"# } else { "{}" };
        server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{}","capabilities":{capabilities}}}}}"#, uri(&root)));
        server.wait(|message| message.contains(r#""id":1"#));
        server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        if watching {
            // The server asks a client that can watch files to report changes to its sources.
            let registration = server.wait(|message| message.contains("client/registerCapability"));
            assert!(registration.contains("workspace/didChangeWatchedFiles") && registration.contains(r#""globPattern":"**/*.lua""#), "{registration}");
        }
        server.open(&main, text);
        server.diagnostics(&main, |message| message.contains("TS2345"));
        // An edit is checked with what it requires as it is on disk now.
        fs::write(&util, "local M = {}\n---@param x string\nfunction M.double(x) return x .. x end\nreturn M\n").unwrap();
        if watching {
            // The client reports the change; until then, edits reuse what was loaded.
            server.send(&format!(r#"{{"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{{"changes":[{{"uri":"{}","type":2}}]}}}}"#, uri(&util)));
        }
        server.change(&main, 2, "local util = require('util')\nreturn util.double('y')\n");
        server.diagnostics(&main, |message| message.contains(r#""version":2"#) && message.contains(r#""diagnostics":[]"#));
        // Edits that reuse the loaded project still check the document's new text.
        server.change(&main, 3, "local util = require('util')\nreturn util.double(1)\n");
        server.diagnostics(&main, |message| message.contains(r#""version":3"#) && message.contains("TS2345"));
        server.finish();
        fs::remove_dir_all(root).unwrap();
    }
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
    let mut server = Server::start(&["--no-stdlib", "--stdlib", "require"]);
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
    let mut server = Server::start(&["--no-stdlib", "--stdlib", "require"]);
    server.send(r#"{"jsonrpc":"2.0","id":7,"method":"textDocument/hover","params":{}}"#);
    let early = server.wait(|message| message.contains(r#""id":7"#));
    assert!(early.contains("-32002"), "{early}");
    server.initialize(&root);
    server.send(r#"{"jsonrpc":"2.0","id":8,"method":"textDocument/unsupportedRequest","params":{}}"#);
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
fn local_shadow_warning_uses_the_original_name_range_and_clears_when_renamed() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "-- 🦀\nlocal value = 1\nprint(value)\nlocal value = 2\nprint(value)\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    let published = server.diagnostics(&main, |message| message.contains("LUA4001"));
    assert!(published.contains(r#""range":{"start":{"line":3,"character":6},"end":{"line":3,"character":11}},"severity":2,"code":"LUA4001""#), "{published}");
    assert!(!published.contains(r#""tags":[1]"#) && !published.contains(r#""severity":1"#), "{published}");
    let corrected = source.replace("local value = 2\nprint(value)", "local other = 2\nprint(other)");
    server.change(&main, 2, &corrected);
    server.diagnostics(&main, |message| message.contains(r#""diagnostics":[]"#));
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

#[test]
fn result_count_inspections_are_unfaded_hints_and_clear_after_matching_the_destinations() {
    let root = fixture();
    let main = root.join("main.lua");
    let prefix = "---@return number, string\nlocal function pair() return 1, 'text' end\n";
    let source = format!("{prefix}local value = pair()\nprint(value)\n");
    fs::write(&main, &source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, &source);
    let published = server.diagnostics(&main, |message| message.contains("LUA4002"));
    assert!(published.contains(r#""range":{"start":{"line":2,"character":14},"end":{"line":2,"character":20}},"severity":4,"code":"LUA4002""#), "{published}");
    assert!(!published.contains(r#""tags":[1]"#) && !published.contains(r#""severity":1"#), "{published}");
    let missing = format!("{prefix}local first, second, missing = pair()\nprint(first, second, missing)\n");
    server.change(&main, 2, &missing);
    let published = server.diagnostics(&main, |message| message.contains("LUA4003"));
    assert!(published.contains(r#""severity":4,"code":"LUA4003""#), "{published}");
    assert!(!published.contains(r#""tags":[1]"#) && !published.contains(r#""severity":1"#), "{published}");
    let corrected = format!("{prefix}local first, second = pair()\nprint(first, second)\n");
    server.change(&main, 3, &corrected);
    server.diagnostics(&main, |message| message.contains(r#""diagnostics":[]"#));
    server.finish();
}

#[test]
fn an_open_document_with_no_file_on_disk_is_checked() {
    let root = fixture();
    // A host such as a game may serve a script it keeps in memory, outside the workspace.
    let script = root.join("memory").join("Global.lua");
    assert!(!script.exists());
    let mut server = Server::start(&["--no-stdlib", "--stdlib", "require"]);
    server.initialize(&root);
    server.open(&script, "---@type string\nlocal text = 42\nreturn text\n");
    let published = server.diagnostics(&script, |message| message.contains("TS2322"));
    assert!(published.contains(r#""range":{"start":{"line":1,"character":13}"#), "{published}");
    server.change(&script, 2, "---@type string\nlocal text = 'fixed'\nreturn text\n");
    server.diagnostics(&script, |message| message.contains(r#""diagnostics":[]"#));
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

/// The UTF-16 position of the `nth` occurrence of `needle` in `text`, as `(line, character)`.
fn position(text: &str, needle: &str, nth: usize) -> (u32, u32) {
    let offset = text.match_indices(needle).nth(nth).unwrap().0;
    let line = text[..offset].matches('\n').count();
    let start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    (line as u32, text[start..offset].encode_utf16().count() as u32)
}

#[test]
fn globals_are_defined_referenced_and_renamed_across_files() {
    let root = fixture();
    let util = root.join("util.lua");
    let util_text = "Counter = 0\nfunction bump() Counter = Counter + 1 end\n";
    fs::write(&util, util_text).unwrap();
    let main = root.join("main.lua");
    let text = "require('util')\nbump()\nprint(Counter, _G.Counter)\nlocal function f() local Counter = 1 return Counter end\n";
    fs::write(&main, text).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    let (line, character) = position(text, "Counter", 0);

    // A global is defined by each of its writes, in any file.
    let definition = server.request(2, "textDocument/definition", &main, line, character);
    assert_eq!(definition.matches(&uri(&util)).count(), 2, "{definition}");
    assert!(definition.contains(r#""range":{"start":{"line":0,"character":0},"end":{"line":0,"character":7}}"#), "{definition}");

    // Its references include `_G.Counter`, but not the local that shadows it.
    let references = |server: &mut Server, id: u32, include: bool| {
        server.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/references","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{character}}},"context":{{"includeDeclaration":{include}}}}}}}"#,
            uri(&main),
        ));
        server.wait(|message| message.contains(&format!(r#""id":{id},"#)))
    };
    let all = references(&mut server, 3, true);
    assert_eq!(all.matches(&uri(&util)).count(), 3, "{all}");
    assert_eq!(all.matches(&uri(&main)).count(), 2, "{all}");
    let reads = references(&mut server, 4, false);
    assert_eq!(reads.matches(r#""uri""#).count(), 3, "{reads}");
    let highlights = server.request(5, "textDocument/documentHighlight", &main, line, character);
    assert_eq!(highlights.matches(r#""kind":2"#).count(), 2, "{highlights}");

    // Renaming it edits every file.
    let rename = |server: &mut Server, id: u32, (line, character): (u32, u32), name: &str| {
        server.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/rename","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{character}}},"newName":"{name}"}}}}"#,
            uri(&main),
        ));
        server.wait(|message| message.contains(&format!(r#""id":{id},"#)))
    };
    let renamed = rename(&mut server, 6, (line, character), "Total");
    assert_eq!(renamed.matches(r#""newText":"Total""#).count(), 5, "{renamed}");
    assert!(renamed.contains(&uri(&util)) && renamed.contains(&uri(&main)), "{renamed}");
    let (_, field) = position(text, "Counter", 1);
    assert!(renamed.contains(&format!(r#""range":{{"start":{{"line":2,"character":{field}}},"end":{{"line":2,"character":{}}}}}"#, field + 7)), "{renamed}");

    // A local is renamed in its own scope only.
    let local = rename(&mut server, 7, position(text, "Counter", 2), "count");
    assert_eq!(local.matches(r#""newText":"count""#).count(), 2, "{local}");
    assert!(!local.contains(&uri(&util)), "{local}");

    // What cannot be renamed says why.
    let invalid = rename(&mut server, 8, (line, character), "end");
    assert!(invalid.contains("-32803") && invalid.contains("is not a Lua name"), "{invalid}");
    let (line, character) = position(text, "print", 0);
    let standard = server.request(9, "textDocument/prepareRename", &main, line, character);
    assert!(standard.contains("declared by a library"), "{standard}");
    let (line, character) = position(text, "bump", 0);
    let prepared = server.request(10, "textDocument/prepareRename", &main, line, character);
    assert!(prepared.contains(r#""placeholder":"bump""#), "{prepared}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completions_include_declared_fields_and_functions_of_a_library_class() {
    let root = fixture();
    let library = root.join("api.def.lua");
    fs::write(&library, "---@class tts__Vector\n---@field x number\n\n---@class tts__Object : userdata\n---@field guid string\n\n---@function tts__Object.getPosition(): tts__Vector\n\n---@function tts__Object:getName(): string\n").unwrap();
    let extra = root.join("extra.def.lua");
    fs::write(&extra, "---@field tts__Object.color string\n\n---@function tts__Object.highlight(color: string)\n").unwrap();
    let main = root.join("main.lua");
    let source = "---@param object tts__Object\nlocal function show(object)\n    return object.guid\nend\n";
    fs::write(&main, source).unwrap();
    let library_argument = library.to_str().unwrap().to_owned();
    let extra_argument = extra.to_str().unwrap().to_owned();
    let mut server = Server::start(&["--lua-version", "5.4", "--library", &library_argument, "--library", &extra_argument]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    let typed = source.replace("    return object.guid\n", "    object.\n    return object.guid\n");
    server.change(&main, 2, &typed);
    let response = server.request(3, "textDocument/completion", &main, 2, 11);
    for label in ["guid", "getPosition", "getName", "color", "highlight"] {
        assert!(response.contains(&format!(r#""label":"{label}""#)), "{label}: {response}");
    }
    let typed = source.replace("    return object.guid\n", "    object:\n    return object.guid\n");
    server.change(&main, 3, &typed);
    let response = server.request(4, "textDocument/completion", &main, 2, 11);
    assert!(response.contains(r#""label":"getName""#), "{response}");
    server.finish();
}

#[test]
fn signature_help_selects_the_overload_the_arguments_typed_so_far_resolve() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "---@overload fun(name: string): string\n---@overload fun(count: number, label: string): number\n---@param value string | number\n---@param label? string\n---@return string | number\nlocal function describe(value, label) return value end\n\n---@class Counter\nlocal Counter = {}\n\n---@param step number\n---@return number\nfunction Counter:add(step) return step end\n\nlocal total = 0\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    let mut version = 1;
    let mut help = |server: &mut Server, line: &str, id: u32| {
        version += 1;
        let text = format!("{source}{}\n", line.replace('|', ""));
        server.change(&main, version, &text);
        // The cursor is on the line after the source.
        let row = source.matches('\n').count() as u32;
        server.request(id, "textDocument/signatureHelp", &main, row, line.find('|').unwrap() as u32)
    };
    // With nothing typed, the first overload applies; a number selects the second.
    let response = help(&mut server, "describe(|", 3);
    assert!(response.contains(r#""label":"describe(name: string): string""#), "{response}");
    assert!(response.contains(r#""label":"describe(count: number, label: string): number""#), "{response}");
    assert!(response.contains(r#""activeSignature":0,"activeParameter":0"#), "{response}");
    let response = help(&mut server, "describe(1, |", 4);
    assert!(response.contains(r#""activeSignature":1,"activeParameter":1"#), "{response}");
    assert!(response.contains(r#""parameters":[{"label":[9,22]},{"label":[24,37]}],"activeParameter":1"#), "{response}");
    let response = help(&mut server, "describe(\"x\"|", 5);
    assert!(response.contains(r#""activeSignature":0,"activeParameter":0"#), "{response}");
    // A method call passes its receiver; the signature shows the arguments after it.
    let response = help(&mut server, "Counter:add(|", 6);
    assert!(response.contains(r#""label":"Counter:add(step: number): number""#), "{response}");
    // A rest parameter takes every argument after the others.
    let response = help(&mut server, "print(1, 2, |", 7);
    assert!(response.contains(r#""label":"print(...: any): nil""#) || response.contains(r#""label":"print(...:"#), "{response}");
    assert!(response.contains(r#""activeParameter":0"#), "{response}");
    // Outside a call there is no signature.
    let response = help(&mut server, "total = total + 1|", 8);
    assert!(response.contains(r#""result":null"#), "{response}");
    server.finish();
}

#[test]
fn hover_completion_and_signature_help_show_documentation() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "--- A point in the plane.\n---@class Point\n---@field x number the distance across\n---@field y number\n--- the distance up\nlocal Point = {}\n\n--- Scales a point.\n--- Both coordinates.\n---@param point Point the point to scale\n---@param factor number how much,\n--- as a ratio\n---@return Point scaled\nlocal function scale(point, factor) return point end\n\n--- How many points there are.\n---@type number\nlocal count = 0\n\n---@param point Point\nlocal function show(point)\n    return point.x + count\nend\nlocal scaled = scale(Point, 2)\n\n---@class Other\n---@field y string\n\n---@param either Point | Other\nlocal function pick(either) return either.y end\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    // A field's description, from its class's documentation.
    let (line, column) = position(source, "point.x", 0);
    let response = server.request(3, "textDocument/hover", &main, line, column + 7);
    assert!(response.contains("the distance across"), "{response}");
    // A local's, at a use of it.
    let (line, column) = position(source, "count\nend", 0);
    let response = server.request(4, "textDocument/hover", &main, line, column + 1);
    assert!(response.contains("How many points there are."), "{response}");
    assert!(response.contains("number"), "{response}");
    // A function's, with its parameters' and result's, at a call.
    let (line, column) = position(source, "scale(Point", 0);
    let response = server.request(5, "textDocument/hover", &main, line, column + 1);
    assert!(response.contains("Scales a point.") && response.contains("how much, as a ratio") && response.contains("scaled"), "{response}");
    // A member of a union, from the alternative that documents it.
    let (line, column) = position(source, "either.y", 0);
    let response = server.request(9, "textDocument/hover", &main, line, column + 7);
    assert!(response.contains("the distance up"), "{response}");
    // A parameter's, from its function's @param.
    let (line, column) = position(source, "point, factor)", 0);
    let response = server.request(6, "textDocument/hover", &main, line, column + 1);
    assert!(response.contains("the point to scale"), "{response}");
    // Completion items carry their declarations' documentation.
    let typed = source.replace("    return point.x + count\n", "    point.\n    return point.x + count\n");
    server.change(&main, 2, &typed);
    let (line, column) = position(&typed, "    point.\n", 0);
    let response = server.request(7, "textDocument/completion", &main, line, column + 10);
    assert!(response.contains("the distance across") && response.contains("the distance up"), "{response}");
    // Signature help describes the function and the parameter being typed.
    let typed = format!("{source}scale(Point, \n");
    server.change(&main, 3, &typed);
    let line = typed.matches('\n').count() as u32 - 1;
    let response = server.request(8, "textDocument/signatureHelp", &main, line, 13);
    assert!(response.contains("Scales a point.") && response.contains("how much,\\nas a ratio"), "{response}");
    server.finish();
}

#[test]
fn members_and_types_are_defined_referenced_highlighted_and_renamed() {
    let root = fixture();
    let shapes = root.join("shapes.lua");
    let shapes_source = "---@class Point\n---@field x number\nlocal Point = {}\n\n---@return number\nfunction Point:length() return self.x end\n\nreturn Point\n";
    fs::write(&shapes, shapes_source).unwrap();
    let main = root.join("main.lua");
    let source = "local Point = require('shapes')\n\n---@param point Point\nlocal function show(point)\n    return point.x + point:length()\nend\n\n---@generic T\n---@param value T\n---@return T\nlocal function same(value) return value end\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4", "--module-root", root.to_str().unwrap()]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    let shapes_uri = uri(&shapes);
    // A field's definition is its @field, in the class's file.
    let (line, column) = position(source, "point.x", 0);
    let response = server.request(3, "textDocument/definition", &main, line, column + 6);
    assert!(response.contains(&shapes_uri) && response.contains(r#""start":{"line":1,"character":10}"#), "{response}");
    // A value's type definition is its class's declaration.
    let response = server.request(12, "textDocument/typeDefinition", &main, line, column + 2);
    assert!(response.contains(&shapes_uri) && response.contains(r#""start":{"line":0,"character":10}"#), "{response}");
    // A method's is its function statement.
    let (line, column) = position(source, "length()", 0);
    let response = server.request(4, "textDocument/definition", &main, line, column + 1);
    assert!(response.contains(&shapes_uri) && response.contains(r#""start":{"line":5,"character":15}"#), "{response}");
    // References to a field, with its declaration, in both files.
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":5,"method":"textDocument/references","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{}}},"context":{{"includeDeclaration":true}}}}}}"#,
        uri(&main), position(source, "point.x", 0).1 + 6,
        line = position(source, "point.x", 0).0,
    ));
    let response = server.wait(|message| message.contains(r#""id":5,"#));
    assert_eq!(response.matches(r#""uri""#).count(), 3, "{response}");
    // A type name's definition is its @class, and a rename renames every use.
    let (line, column) = position(source, "point Point", 0);
    let response = server.request(6, "textDocument/definition", &main, line, column + 7);
    assert!(response.contains(&shapes_uri) && response.contains(r#""start":{"line":0,"character":10}"#), "{response}");
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":7,"method":"textDocument/rename","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{}}},"newName":"Vector"}}}}"#,
        uri(&main), column + 7,
    ));
    let response = server.wait(|message| message.contains(r#""id":7,"#));
    assert_eq!(response.matches(r#""newText":"Vector""#).count(), 2, "{response}");
    // A type name's hover shows the tag that declares it.
    let response = server.request(11, "textDocument/hover", &main, line, column + 7);
    assert!(response.contains("---@class Point"), "{response}");
    // A generic parameter's definition is its @generic, in its own block.
    let (line, column) = position(source, "value T", 0);
    let response = server.request(8, "textDocument/definition", &main, line, column + 6);
    assert!(response.contains(r#""start":{"line":7,"character":12}"#), "{response}");
    // A member's uses in the document are highlighted.
    let (line, column) = position(source, "point.x", 0);
    let response = server.request(9, "textDocument/documentHighlight", &main, line, column + 6);
    assert_eq!(response.matches(r#""range""#).count(), 1, "{response}");
    // Renaming a member renames its declaration and its uses.
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":10,"method":"textDocument/rename","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{}}},"newName":"left"}}}}"#,
        uri(&main), column + 6,
    ));
    let response = server.wait(|message| message.contains(r#""id":10,"#));
    assert_eq!(response.matches(r#""newText":"left""#).count(), 3, "{response}");
    server.finish();
}

#[test]
fn annotations_complete_type_names_and_calls_complete_their_parameters() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "--- A point.\n---@class Point\nlocal Point = {}\n\n---@param x number\n---@param y? number\nlocal function move(x, y) end\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{}","capabilities":{{"textDocument":{{"completion":{{"completionItem":{{"snippetSupport":true}}}}}}}}}}}}"#, uri(&root)));
    server.wait(|message| message.contains(r#""id":1"#));
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    // A type in an annotation completes the project's types, with their documentation.
    let typed = format!("{source}---@param p Po\nlocal function f(p) end\n");
    server.change(&main, 2, &typed);
    let line = source.matches('\n').count() as u32;
    let response = server.request(3, "textDocument/completion", &main, line, 14);
    assert!(response.contains(r#""label":"Point","kind":7"#) && response.contains("A point."), "{response}");
    assert!(response.contains(r#""label":"number""#), "{response}");
    // A tag's name after `---@`.
    let typed = format!("{source}---@par\n");
    server.change(&main, 3, &typed);
    let response = server.request(4, "textDocument/completion", &main, line, 7);
    assert!(response.contains(r#""label":"param""#), "{response}");
    // A function inserts a call with its required parameters as placeholders.
    let typed = format!("{source}mov\n");
    server.change(&main, 4, &typed);
    let response = server.request(5, "textDocument/completion", &main, line, 3);
    assert!(response.contains(r#""newText":"move(${1:x})$0""#) && response.contains(r#""insertTextFormat":2"#), "{response}");
    // Unless the call is already written.
    let typed = format!("{source}mov(1)\n");
    server.change(&main, 5, &typed);
    let response = server.request(6, "textDocument/completion", &main, line, 3);
    assert!(response.contains(r#""newText":"move""#), "{response}");
    server.finish();
}

#[test]
fn documents_and_the_workspace_list_their_symbols() {
    let root = fixture();
    let shapes = root.join("shapes.lua");
    fs::write(&shapes, "---@class Point\n---@field x number\nlocal Point = {}\n\nfunction Point:length() return 0 end\n\nreturn Point\n").unwrap();
    let main = root.join("main.lua");
    let source = "local function show() end\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    let response = server.request(3, "textDocument/documentSymbol", &main, 0, 0);
    assert!(response.contains(r#""name":"show","kind":12"#), "{response}");
    // The workspace's symbols, by a query whose characters a name contains in order.
    server.send(r#"{"jsonrpc":"2.0","id":4,"method":"workspace/symbol","params":{"query":"plen"}}"#);
    let response = server.wait(|message| message.contains(r#""id":4,"#));
    assert!(response.contains(r#""name":"length","kind":6"#) && response.contains(r#""containerName":"Point""#) && response.contains(&uri(&shapes)), "{response}");
    assert!(!response.contains(r#""name":"show""#), "{response}");
    server.finish();
}

#[test]
fn completion_edits_preserve_string_quotes_close_attributes_and_insert_metadata_text() {
    let root = fixture();
    let main = root.join("main.lua");
    fs::write(&main, "").unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, "");
    server.diagnostics(&main, |_| true);

    server.change(&main, 2, "local value <");
    let response = server.request(2, "textDocument/completion", &main, 0, 13);
    assert!(response.contains(r#""label":"const","kind":14,"textEdit":{"range":{"start":{"line":0,"character":13},"end":{"line":0,"character":13}},"newText":"const>"}"#), "{response}");
    assert!(response.contains(r#""label":"close""#), "{response}");
    server.change(&main, 3, "local value <const> = 1\nreturn value\n");
    server.diagnostics(&main, |message| message.contains(r#""diagnostics":[]"#));

    let prefix = "---@param value string\nlocal function take(value) return value end\ntake(\"a'b\\\\c\")\n";
    server.change(&main, 4, &format!("{prefix}take('hi')\n"));
    let response = server.request(3, "textDocument/completion", &main, 3, 7);
    assert!(response.contains(r#""label":"a'b\\c","kind":12,"textEdit":{"range":{"start":{"line":3,"character":6},"end":{"line":3,"character":8}},"newText":"a\\'b\\\\c"}"#), "{response}");
    server.change(&main, 5, &format!("{prefix}take('a\\'b\\\\c')\n"));
    server.diagnostics(&main, |message| message.contains(r#""diagnostics":[]"#));

    let prefix = "---@param callback (fun(event:string):string) | `function(event) return event end`\nlocal function listen(callback) return callback('event') end\n";
    server.change(&main, 6, &format!("{prefix}listen()\n"));
    let response = server.request(4, "textDocument/completion", &main, 2, 7);
    assert!(response.contains(r#""label":"function(event) return event end","kind":15,"textEdit":{"range":{"start":{"line":2,"character":7},"end":{"line":2,"character":7}},"newText":"function(event) return event end"}"#), "{response}");
    server.change(&main, 7, &format!("{prefix}listen(function(event) return event end)\n"));
    server.diagnostics(&main, |message| message.contains(r#""diagnostics":[]"#));

    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn requires_link_and_documents_fold_and_select() {
    let root = fixture();
    fs::write(root.join("shapes.lua"), "return {}\n").unwrap();
    let main = root.join("main.lua");
    let source = "local shapes = require('shapes')\nlocal function f()\n    return shapes\nend\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4", "--module-root", root.to_str().unwrap()]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    let response = server.request(3, "textDocument/documentLink", &main, 0, 0);
    assert!(response.contains(&uri(&root.join("shapes.lua"))), "{response}");
    let response = server.request(4, "textDocument/foldingRange", &main, 0, 0);
    assert!(response.contains(r#"{"startLine":1,"endLine":2}"#), "{response}");
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":5,"method":"textDocument/selectionRange","params":{{"textDocument":{{"uri":"{}"}},"positions":[{{"line":2,"character":12}}]}}}}"#, uri(&main)));
    let response = server.wait(|message| message.contains(r#""id":5,"#));
    assert!(response.contains(r#""range":{"start":{"line":2,"character":11},"end":{"line":2,"character":17}}"#) && response.contains(r#""parent""#), "{response}");
    server.finish();
}

#[test]
fn a_require_string_links_only_through_the_standard_loader() {
    let root = fixture();
    fs::write(root.join("shapes.lua"), "return {}\n").unwrap();
    let main = root.join("main.lua");
    // An alias of the loader is the loader until it is reassigned.
    let source = "local loader = require\nlocal a = loader('shapes')\nloader = function(name) return name end\nlocal b = loader('shapes')\n";
    fs::write(&main, source).unwrap();
    let replaced = root.join("replaced.lua");
    // A replaced global `require` is an ordinary call to its replacement.
    let replacing = "require = function(name) return name end\nlocal c = require('shapes')\n";
    fs::write(&replaced, replacing).unwrap();
    let shapes = uri(&root.join("shapes.lua"));
    let mut server = Server::start(&["--lua-version", "5.2", "--stdlib", "require", "--module-root", root.to_str().unwrap()]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    let response = server.request(2, "textDocument/definition", &main, 1, 19);
    assert!(response.contains(&shapes), "{response}");
    let response = server.request(3, "textDocument/definition", &main, 3, 19);
    assert!(response.contains(r#""result":null"#), "{response}");
    let response = server.request(4, "textDocument/documentLink", &main, 0, 0);
    assert_eq!(response.matches(&shapes).count(), 1, "{response}");
    server.open(&replaced, replacing);
    server.diagnostics(&replaced, |_| true);
    let response = server.request(5, "textDocument/definition", &replaced, 1, 20);
    assert!(response.contains(r#""result":null"#), "{response}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_require_resolves_by_its_path_suffix_as_the_project_configuration_and_settings_say() {
    let root = fixture();
    fs::create_dir_all(root.join("lib/ge_tts")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    let utils = root.join("lib/ge_tts/Utils.ttslua");
    fs::write(&utils, "return {}\n").unwrap();
    let main = root.join("src/main.ttslua");
    let source = "local Utils = require('ge_tts.Utils')\nreturn Utils\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    let published = server.diagnostics(&main, |_| true);
    assert!(!published.contains("TS2307"), "{published}");
    let response = server.request(2, "textDocument/definition", &main, 0, 25);
    assert!(response.contains(&uri(&utils)), "{response}");
    let response = server.request(3, "textDocument/documentLink", &main, 0, 0);
    assert!(response.contains(&uri(&utils)), "{response}");
    // A changed configuration applies at once.
    let config = root.join(".luarc.json");
    fs::write(&config, r#"{ "runtime": { "pathStrict": true } }"#).unwrap();
    server.send(&format!(r#"{{"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{{"changes":[{{"uri":"{}","type":1}}]}}}}"#, uri(&config)));
    server.diagnostics(&main, |message| message.contains("TS2307"));
    let response = server.request(4, "textDocument/documentLink", &main, 0, 0);
    assert!(!response.contains(&uri(&utils)), "{response}");
    // The client's settings override it.
    server.send(r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"luanalysis":{"pathStrict":false}}}}"#);
    server.diagnostics(&main, |message| !message.contains("TS2307"));
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_open_declaration_file_is_a_declaration() {
    let root = fixture();
    let hand = root.join("Hand.def.lua");
    let stub = "---@class Hand\nlocal Hand\n\n---@return string\nfunction Hand.getValue() end\n";
    fs::write(&hand, stub).unwrap();
    let main = root.join("main.lua");
    let source = "---@param hand Hand\nlocal function value(hand)\n    ---@type number\n    local wrong = hand.getValue()\nend\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&hand, stub);
    server.open(&main, source);
    // A result is published once, in no particular order among the files.
    let (mut checked, mut declared) = (None, None);
    while checked.is_none() || declared.is_none() {
        let published = server.wait(|message| message.contains("textDocument/publishDiagnostics"));
        if published.contains(&uri(&main)) && published.contains("TS2322") { checked = Some(published) }
        else if published.contains(&uri(&hand)) { declared = Some(published) }
    }
    let published = checked.unwrap();
    assert!(published.contains("is not assignable to type 'number'"), "{published}");
    // Its stubs are neither a nil value nor bodies that should return.
    let published = declared.unwrap();
    assert!(!published.contains("TS7053") && !published.contains("TS2355") && !published.contains("conflict"), "{published}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inlay_hints_show_inferred_types_and_literal_arguments_parameters() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "---@param x number\n---@param label string\n---@return number\nlocal function move(x, label) return x end\n\nlocal moved = move(1, 'a')\nlocal count = 0\nfor index, value in ipairs({ 'a' }) do end\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":3,"method":"textDocument/inlayHint","params":{{"textDocument":{{"uri":"{}"}},"range":{{"start":{{"line":0,"character":0}},"end":{{"line":9,"character":0}}}}}}}}"#, uri(&main)));
    let response = server.wait(|message| message.contains(r#""id":3,"#));
    // The type of a local whose initializer does not show it, and of loop variables.
    assert!(response.contains(r#"{"position":{"line":5,"character":11},"label":": number","kind":1}"#), "{response}");
    assert!(response.contains(r#"{"position":{"line":7,"character":9},"label":": number","kind":1}"#), "{response}");
    assert!(response.contains(r#"{"position":{"line":7,"character":16},"label":": \"a\"","kind":1}"#), "{response}");
    // Not of one initialized with a literal.
    assert!(!response.contains(r#""position":{"line":6,"#), "{response}");
    // The parameter names of literal arguments.
    assert!(response.contains(r#"{"position":{"line":5,"character":19},"label":"x:","kind":2,"paddingRight":true}"#), "{response}");
    assert!(response.contains(r#""label":"label:""#), "{response}");
    // Not of a table argument.
    assert!(!response.contains(r#""label":"t:""#), "{response}");
    server.finish();
}

#[test]
fn semantic_tokens_are_encoded_relative_to_one_another() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "local count = 1\nprint(count)\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    let response = server.request(3, "textDocument/semanticTokens/full", &main, 0, 0);
    // `count` (a variable's declaration), `print` (a standard global function) and `count`.
    assert!(response.contains(r#""data":[0,6,5,5,1,1,0,5,7,12,0,6,5,5,0]"#), "{response}");
    server.finish();
}

#[test]
fn quick_fixes_correct_misspellings_and_mark_unused_names() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "local config = { port = 8080 }\nlocal count = 1\nprint(config.portt, cuont)\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    let published = server.diagnostics(&main, |message| message.contains("TS2339"));
    assert!(published.contains("TS2304"), "{published}");
    let diagnostic = |code: &str, character: u32, length: u32| format!(
        r#"{{"range":{{"start":{{"line":2,"character":{character}}},"end":{{"line":2,"character":{}}}}},"code":"{code}","message":""}}"#,
        character + length,
    );
    let unused = r#"{"range":{"start":{"line":1,"character":6},"end":{"line":1,"character":11}},"code":"TS6133","message":""}"#;
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"textDocument/codeAction","params":{{"textDocument":{{"uri":"{}"}},"range":{{"start":{{"line":2,"character":0}},"end":{{"line":2,"character":0}}}},"context":{{"diagnostics":[{},{},{unused}]}}}}}}"#,
        uri(&main), diagnostic("TS2339", 13, 5), diagnostic("TS2304", 20, 5),
    ));
    let response = server.wait(|message| message.contains(r#""id":3,"#));
    assert!(response.contains(r#""title":"Change to 'port'""#) && response.contains(r#""newText":"port""#), "{response}");
    assert!(response.contains(r#""title":"Change to 'count'""#), "{response}");
    assert!(response.contains(r#""title":"Prefix 'count' with '_' to mark it unused""#) && response.contains(r#""newText":"_""#), "{response}");
    server.finish();
}

#[test]
fn call_and_type_hierarchies_follow_calls_and_bases() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "---@class Animal\n---@class Dog : Animal\n\nlocal function helper() end\nlocal function run() helper() end\nrun()\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    let response = server.request(3, "textDocument/prepareCallHierarchy", &main, 3, 16);
    assert!(response.contains(r#""name":"helper","kind":12"#), "{response}");
    let item = |name: &str, line: u32, character: u32| format!(
        r#"{{"name":"{name}","kind":12,"uri":"{}","range":{{"start":{{"line":{line},"character":0}},"end":{{"line":{line},"character":0}}}},"selectionRange":{{"start":{{"line":{line},"character":{character}}},"end":{{"line":{line},"character":{character}}}}}}}"#,
        uri(&main),
    );
    // helper is called by run; run calls helper, and the main chunk calls run.
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":4,"method":"callHierarchy/incomingCalls","params":{{"item":{}}}}}"#, item("helper", 3, 16)));
    let response = server.wait(|message| message.contains(r#""id":4,"#));
    assert!(response.contains(r#""from":{"name":"run""#) && response.contains(r#""fromRanges":[{"start":{"line":4,"character":21}"#), "{response}");
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":5,"method":"callHierarchy/outgoingCalls","params":{{"item":{}}}}}"#, item("run", 4, 16)));
    let response = server.wait(|message| message.contains(r#""id":5,"#));
    assert!(response.contains(r#""to":{"name":"helper""#), "{response}");
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":6,"method":"callHierarchy/incomingCalls","params":{{"item":{}}}}}"#, item("run", 4, 16)));
    let response = server.wait(|message| message.contains(r#""id":6,"#));
    assert!(response.contains(r#""from":{"name":"main.lua","kind":2"#), "{response}");
    // Dog extends Animal.
    let response = server.request(7, "textDocument/prepareTypeHierarchy", &main, 1, 11);
    assert!(response.contains(r#""name":"Dog","kind":5"#), "{response}");
    let type_item = |name: &str, line: u32, character: u32| item(name, line, character).replace(r#""kind":12"#, r#""kind":5"#);
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":8,"method":"typeHierarchy/supertypes","params":{{"item":{}}}}}"#, type_item("Dog", 1, 11)));
    let response = server.wait(|message| message.contains(r#""id":8,"#));
    assert!(response.contains(r#""name":"Animal""#), "{response}");
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":9,"method":"typeHierarchy/subtypes","params":{{"item":{}}}}}"#, type_item("Animal", 0, 11)));
    let response = server.wait(|message| message.contains(r#""id":9,"#));
    assert!(response.contains(r#""name":"Dog""#), "{response}");
    server.finish();
}

#[test]
fn settings_change_what_is_checked() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "local first = unpack({ 1 })\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&[]);
    // Lua 5.1 has a global unpack.
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{}","capabilities":{{}},"initializationOptions":{{"luaVersion":"5.1"}}}}}}"#, uri(&root)));
    server.wait(|message| message.contains(r#""id":1"#));
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    server.open(&main, source);
    let published = server.diagnostics(&main, |_| true);
    assert!(!published.contains("TS2304"), "{published}");
    // Lua 5.4 does not.
    server.send(r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"luanalysis":{"luaVersion":"5.4"}}}}"#);
    let published = server.diagnostics(&main, |message| message.contains("TS2304"));
    assert!(published.contains("Cannot find name 'unpack'"), "{published}");
    server.finish();
}

#[test]
fn strict_nil_settings_recheck_unchanged_open_buffers() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "---@type number\nlocal value = nil\nreturn value\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&[]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |message| message.contains("TS2322"));
    server.send(r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"luanalysis":{"strictNil":false}}}}"#);
    server.diagnostics(&main, |message| message.contains(r#""diagnostics":[]"#));
    server.send(r#"{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"luanalysis":{"strictNil":true}}}}"#);
    server.diagnostics(&main, |message| message.contains("TS2322"));
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn documents_change_incrementally_and_requests_can_be_cancelled() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "local count = 1\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    server.diagnostics(&main, |_| true);
    // Replace `1` with `count.nope`: only the range changes.
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{}","version":2}},"contentChanges":[{{"range":{{"start":{{"line":0,"character":14}},"end":{{"line":0,"character":15}}}},"text":"nil\nlocal other = count.nope"}}]}}}}"#,
        uri(&main),
    ));
    let published = server.diagnostics(&main, |message| message.contains("nope"));
    assert!(published.contains(r#""start":{"line":1,"character":20}"#), "{published}");
    // A query waiting behind another is answered as cancelled.
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":3,"method":"textDocument/signatureHelp","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":1,"character":0}}}}}}"#, uri(&main)));
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":4,"method":"textDocument/completion","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":1,"character":0}}}}}}"#, uri(&main)));
    server.send(r#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":4}}"#);
    let response = server.wait(|message| message.contains(r#""id":4,"#));
    assert!(response.contains("-32800") || response.contains(r#""result""#), "{response}");
    server.finish();
}

#[test]
fn checks_show_progress_and_every_workspace_folder_is_checked() {
    let root = fixture();
    let (first, second) = (root.join("first"), root.join("second"));
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    fs::write(first.join("a.lua"), "local a = undefinedFirst\n").unwrap();
    fs::write(second.join("b.lua"), "local b = undefinedSecond\n").unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"capabilities":{{"window":{{"workDoneProgress":true}}}},"workspaceFolders":[{{"uri":"{}","name":"first"}},{{"uri":"{}","name":"second"}}]}}}}"#,
        uri(&first), uri(&second),
    ));
    server.wait(|message| message.contains(r#""id":1"#));
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    let create = server.wait(|message| message.contains("window/workDoneProgress/create"));
    let token = create.split(r#""token":""#).nth(1).unwrap().split('"').next().unwrap().to_owned();
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":"{token}","result":null}}"#));
    // Progress begins and ends, and both folders' files are reported, in whatever order.
    let mut seen: Vec<String> = Vec::new();
    let found = |seen: &[String], wanted: &dyn Fn(&str) -> bool| seen.iter().any(|message| wanted(message));
    let begun = |message: &str| message.contains("$/progress") && message.contains("begin") && message.contains(&token);
    let ended = |message: &str| message.contains("$/progress") && message.contains("\"end\"");
    let first_reported = |message: &str| message.contains("publishDiagnostics") && message.contains("undefinedFirst");
    let second_reported = |message: &str| message.contains("publishDiagnostics") && message.contains("undefinedSecond");
    while !(found(&seen, &begun) && found(&seen, &ended) && found(&seen, &first_reported) && found(&seen, &second_reported)) {
        seen.push(server.messages.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| panic!("{seen:#?}")));
    }
    server.finish();
}

#[test]
fn a_members_origins_and_documentation_are_its_owners() {
    let root = fixture();
    let library = root.join("api.def.lua");
    // Members declared apart from their classes: every declaration's candidates include each
    // same-spelled one, of whichever owner.
    fs::write(&library, "---@class Color\n---@field r number\n\n--- Sets the colour's components.\n---@function Color:set(r: number, g: number, b: number): void\n\n---@class Vector\n---@field x number\n\n--- Sets the vector's components.\n---@param x number # New X.\n---@function Vector:set(x: number): void\n\n---@class Object\n---@field name string\n\n--- This object's type.\n---@deprecated Use type instead.\n---@field Object.tag string\n\n---@shape ButtonElement\n---@field tag \"Button\"\n").unwrap();
    let main = root.join("main.lua");
    let text = "---@param vector Vector\nlocal function move(vector)\n    vector:set(1)\nend\n---@param element ButtonElement\nlocal function probe(element)\n    local kind = element.tag\n    local other = element.\nend\n";
    fs::write(&main, text).unwrap();
    let library_argument = library.to_str().unwrap().to_owned();
    let mut server = Server::start(&["--lua-version", "5.2", "--library", &library_argument]);
    server.initialize(&root);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    let hover = server.request(2, "textDocument/hover", &main, 2, 12);
    assert!(hover.contains("Sets the vector's components.") && hover.contains("New X.") && !hover.contains("colour"), "{hover}");
    let signature = server.request(3, "textDocument/signatureHelp", &main, 2, 15);
    assert!(signature.contains("Sets the vector's components.") && !signature.contains("colour"), "{signature}");
    let definition = server.request(4, "textDocument/definition", &main, 2, 12);
    assert_eq!(definition.matches(r#""uri""#).count(), 1, "{definition}");
    assert!(definition.contains(r#""start":{"line":11,"#), "{definition}");
    // Another class's deprecated field is not this shape's.
    let definition = server.request(5, "textDocument/definition", &main, 6, 26);
    assert_eq!(definition.matches(r#""uri""#).count(), 1, "{definition}");
    assert!(definition.contains(r#""start":{"line":21,"#), "{definition}");
    let completion = server.request(6, "textDocument/completion", &main, 7, 26);
    assert!(completion.contains(r#""label":"tag""#) && completion.contains(r#""detail":"\"Button\"""#), "{completion}");
    assert!(!completion.contains("Deprecated") && !completion.contains(r#""tags":[1]"#), "{completion}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn signature_help_documents_bodyless_global_and_member_functions() {
    let root = fixture();
    let library = root.join("api.def.lua");
    fs::write(&library, "---@class Methods\n\n--- Moves a member.\n---@function Methods.move(distance: number): boolean\n---@param distance number The spaces to move.\n\n--- Moves a global.\n---@param distance number The spaces it moves.\n---@function compact(distance: number): boolean\n\n---@type Methods\nMethods = nil\n\n--- Moves an ambient function.\n---@param distance number The ambient distance.\n---@return boolean Whether it moved.\nfunction ambientMove(distance) end\n").unwrap();
    let main = root.join("main.lua");
    let text = "Methods.move(1)\ncompact(1)\nambientMove(1)\n";
    fs::write(&main, text).unwrap();
    let library_argument = library.to_str().unwrap().to_owned();
    let mut server = Server::start(&["--lua-version", "5.2", "--library", &library_argument]);
    server.initialize(&root);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    // Written heads and ambient function statements both retain their own documentation.
    let member = server.request(2, "textDocument/signatureHelp", &main, 0, 13);
    assert!(member.contains("Moves a member.") && member.contains("The spaces to move."), "{member}");
    let global = server.request(3, "textDocument/signatureHelp", &main, 1, 8);
    assert!(global.contains("Moves a global.") && global.contains("The spaces it moves."), "{global}");
    let ambient = server.request(4, "textDocument/signatureHelp", &main, 2, 12);
    assert!(ambient.contains("Moves an ambient function.") && ambient.contains("The ambient distance.") && ambient.contains("Whether it moved."), "{ambient}");
    let hover = server.request(5, "textDocument/hover", &main, 2, 1);
    assert!(hover.contains("Moves an ambient function.") && hover.contains("The ambient distance."), "{hover}");
    assert_eq!(hover.matches("fun(distance: number): boolean").count(), 1, "{hover}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn signature_help_and_hover_keep_each_global_overloads_documentation() {
    let root = fixture();
    let library = root.join("api.def.lua");
    fs::write(&library, "--- Looks up a label.\n---@param value string The textual label.\n---@function auditPick(value: string): string\n\n--- Looks up a numeric count and label.\n---@param count number The numeric count.\n---@param label string The count label.\n---@function auditPick(count: number, label: string): number\n\n--- Shared fallback description.\n---@param value string The fallback value.\n---@function fallback(value: string): string\n\n---@function fallback(count: number): number\n").unwrap();
    let main = root.join("main.lua");
    let text = "auditPick(1, \"x\")\nlocal alias = auditPick\nalias(1, \"x\")\nfallback(1)\n";
    fs::write(&main, text).unwrap();
    let library_argument = library.to_str().unwrap().to_owned();
    let mut server = Server::start(&["--lua-version", "5.2", "--library", &library_argument]);
    server.initialize(&root);
    server.open(&main, text);
    let diagnostics = server.diagnostics(&main, |_| true);
    assert!(diagnostics.contains(r#""diagnostics":[]"#), "{diagnostics}");
    for (id, row, name) in [(2, 0, "auditPick"), (3, 2, "alias")] {
        let response = server.request(id, "textDocument/signatureHelp", &main, row, name.len() as u32 + 3);
        let heads: Vec<_> = response.split(r#"{"label":""#).skip(1).collect();
        assert_eq!(heads.len(), 2, "{response}");
        assert!(heads[0].starts_with(&format!("{name}(value: string): string")), "{response}");
        assert!(heads[0].contains("Looks up a label.") && heads[0].contains("The textual label."), "{response}");
        assert!(!heads[0].contains("numeric count") && !heads[0].contains("The count label."), "{response}");
        assert!(heads[1].starts_with(&format!("{name}(count: number, label: string): number")), "{response}");
        assert!(heads[1].contains("Looks up a numeric count and label.") && heads[1].contains("The numeric count.") && heads[1].contains("The count label."), "{response}");
        assert!(!heads[1].contains("The textual label.") && !heads[1].contains("Looks up a label."), "{response}");
        assert!(response.contains(r#""activeSignature":1,"activeParameter":1"#), "{response}");
    }
    let hover = server.request(4, "textDocument/hover", &main, 0, 1);
    assert!(hover.contains("Looks up a label.") && hover.contains("Looks up a numeric count and label."), "{hover}");
    assert!(hover.contains("The textual label.") && hover.contains("The numeric count.") && hover.contains("The count label."), "{hover}");
    assert!(hover.contains("fun(value: string): string") && hover.contains("fun(count: number, label: string): number"), "{hover}");
    // A declaration without prose uses the callable's documented description, while parameter
    // descriptions still match by name instead of leaking into a different parameter.
    let fallback = server.request(5, "textDocument/signatureHelp", &main, 3, 9);
    let heads: Vec<_> = fallback.split(r#"{"label":""#).skip(1).collect();
    assert_eq!(heads.len(), 2, "{fallback}");
    assert!(heads.iter().all(|head| head.contains("Shared fallback description.")), "{fallback}");
    assert!(heads[0].contains("The fallback value.") && !heads[1].contains("The fallback value."), "{fallback}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inherited_generic_member_overloads_keep_their_documentation_through_aliases() {
    let root = fixture();
    let library = root.join("api.def.lua");
    fs::write(&library, "---@class Box<T>\n\n--- Reads one stored value.\n---@param value T The stored value.\n---@function Box:pick(value: T): T\n\n--- Reads by count and label.\n---@param count number The member count.\n---@param label string The member label.\n---@function Box:pick(count: number, label: string): T\n\n---@class Derived: Box<string>\n\n---@alias Alias Derived\n").unwrap();
    let main = root.join("main.lua");
    let text = "---@param box Alias\nlocal function use(box)\n    box:pick(1, \"x\")\n    box[\"pick\"](box, 1, \"x\")\nend\nreturn use\n";
    fs::write(&main, text).unwrap();
    let library_argument = library.to_str().unwrap().to_owned();
    let mut server = Server::start(&["--lua-version", "5.2", "--library", &library_argument]);
    server.initialize(&root);
    server.open(&main, text);
    let diagnostics = server.diagnostics(&main, |_| true);
    assert!(diagnostics.contains(r#""diagnostics":[]"#), "{diagnostics}");
    let response = server.request(2, "textDocument/signatureHelp", &main, 2, 16);
    let heads: Vec<_> = response.split(r#"{"label":""#).skip(1).collect();
    assert_eq!(heads.len(), 2, "{response}");
    assert!(heads[0].starts_with("box:pick(value: string): string"), "{response}");
    assert!(heads[0].contains("Reads one stored value.") && heads[0].contains("The stored value."), "{response}");
    assert!(!heads[0].contains("The member count."), "{response}");
    assert!(heads[1].starts_with("box:pick(count: number, label: string): string"), "{response}");
    assert!(heads[1].contains("Reads by count and label.") && heads[1].contains("The member count.") && heads[1].contains("The member label."), "{response}");
    assert!(!heads[1].contains("The stored value."), "{response}");
    assert!(response.contains(r#""activeSignature":1,"activeParameter":1"#), "{response}");
    let hover = server.request(3, "textDocument/hover", &main, 2, 10);
    assert!(hover.contains("Reads one stored value.") && hover.contains("Reads by count and label."), "{hover}");
    assert!(hover.contains("The stored value.") && hover.contains("The member count.") && hover.contains("The member label."), "{hover}");
    let indexed = server.request(4, "textDocument/hover", &main, 3, 10);
    assert!(indexed.contains("Reads one stored value.") && indexed.contains("Reads by count and label."), "{indexed}");
    let signature = server.request(5, "textDocument/signatureHelp", &main, 3, 24);
    assert!(signature.contains("The stored value.") && signature.contains("The member count.") && signature.contains("The member label."), "{signature}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn host_callbacks_complete_as_implementations() {
    let root = fixture();
    let library = root.join("api.def.lua");
    fs::write(&library, "---@alias tts__PlayerColor \"Red\" | \"Blue\"\n---@class tts__Object\n\n--- Called after an object is dropped.\n---@type nil | fun(player_color: tts__PlayerColor, object: tts__Object): void\n_G.onObjectDrop = nil\n\n---@type nil | fun(): string | nil\n_G.onSave = nil\n\n---@param value string\nfunction print2(value) end\n").unwrap();
    let main = root.join("main.lua");
    let initial = "function onSave() return nil end\n";
    fs::write(&main, initial).unwrap();
    let library_argument = library.to_str().unwrap().to_owned();
    let start = |snippets: bool| {
        let mut server = Server::start(&["--lua-version", "5.2", "--library", &library_argument]);
        server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{}","capabilities":{{"textDocument":{{"completion":{{"completionItem":{{"snippetSupport":{snippets}}}}}}}}}}}}}"#, uri(&root)));
        server.wait(|message| message.contains(r#""id":1"#));
        server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        server.open(&main, initial);
        server.diagnostics(&main, |_| true);
        server
    };
    let mut server = start(true);
    let mut version = 1;
    let mut complete = |server: &mut Server, text: &str, line: u32, character: u32| {
        version += 1;
        server.change(&main, version, text);
        server.request(version + 10, "textDocument/completion", &main, line, character)
    };
    // After `function`, a callback's implementation.
    let response = complete(&mut server, &format!("function onObjectDr\n{initial}"), 0, 19);
    assert!(response.contains(r#""label":"onObjectDrop","kind":15,"textEdit":{"range":{"start":{"line":0,"character":9},"end":{"line":0,"character":19}},"newText":"onObjectDrop(player_color, object)\n\t$0\nend"},"filterText":"onObjectDrop","insertTextFormat":2,"sortText":"00000","detail":"Implement onObjectDrop(player_color, object)""#), "{response}");
    assert!(response.contains("Called after an object is dropped."), "{response}");
    // Not for a callback already implemented, nor for a library function.
    assert!(!response.contains(r#""label":"onSave""#) && !response.contains(r#""label":"print2""#), "{response}");
    // A statement of its own inserts `function` too.
    let response = complete(&mut server, &format!("onObjectDr\n{initial}"), 0, 10);
    assert!(response.contains(r#""range":{"start":{"line":0,"character":0},"end":{"line":0,"character":10}},"newText":"function onObjectDrop(player_color, object)\n\t$0\nend""#), "{response}");
    // Not in an expression, a local function or before a written parameter list.
    let response = complete(&mut server, &format!("local handler = onObjectDr\n{initial}"), 0, 26);
    assert!(!response.contains(r#""kind":15"#), "{response}");
    let response = complete(&mut server, &format!("local function onObjectDr\n{initial}"), 0, 25);
    assert!(!response.contains(r#""kind":15"#), "{response}");
    let response = complete(&mut server, &format!("function onObjectDr(a, b) end\n{initial}"), 0, 19);
    assert!(response.contains(r#""label":"onObjectDrop""#) && !response.contains(r#""kind":15"#), "{response}");
    server.finish();
    // A client without snippets receives plain text.
    let mut server = start(false);
    let response = complete(&mut server, &format!("function onObjectDr\n{initial}"), 0, 19);
    assert!(response.contains(r#""newText":"onObjectDrop(player_color, object)\n\t\nend""#) && !response.contains("insertTextFormat"), "{response}");
    server.finish();
}

#[test]
fn deprecated_declarations_are_marked_and_explained() {
    let root = fixture();
    let library = root.join("api.def.lua");
    fs::write(&library, "--- Summary explaining the old API and its replacement.\n---@deprecated Use newApi instead.\n---@function oldApi(value: string): boolean\n\n--- The new API.\n---@function newApi(value: string): boolean\n").unwrap();
    let main = root.join("main.lua");
    let source = "local ok = oldApi('x')\n";
    fs::write(&main, source).unwrap();
    let library_argument = library.to_str().unwrap().to_owned();
    for (capability, marked) in [(r#"{"tagSupport":{"valueSet":[1]}}"#, r#""tags":[1]"#), (r#"{"deprecatedSupport":true}"#, r#""deprecated":true"#)] {
        let mut server = Server::start(&["--lua-version", "5.4", "--library", &library_argument]);
        server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{}","capabilities":{{"textDocument":{{"completion":{{"completionItem":{capability}}}}}}}}}}}"#, uri(&root)));
        server.wait(|message| message.contains(r#""id":1"#));
        server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        server.open(&main, source);
        server.diagnostics(&main, |_| true);
        server.change(&main, 2, &format!("{source}local other = old\n"));
        let response = server.request(3, "textDocument/completion", &main, 1, 17);
        let old_item = response.split(r#"{"label":"oldApi""#).nth(1).unwrap_or_default();
        let old_item = &old_item[..old_item.find("{\"label\"").unwrap_or(old_item.len())];
        assert!(old_item.contains(marked) && old_item.contains("**Deprecated:** Use newApi instead.") && old_item.contains("Summary explaining"), "{response}");
        let new_item = response.split(r#"{"label":"newApi""#).nth(1).unwrap_or_default();
        let new_item = &new_item[..new_item.find("{\"label\"").unwrap_or(new_item.len())];
        assert!(!new_item.contains(marked) && new_item.contains("The new API."), "{response}");
        server.finish();
    }
}

/// The lines of a rename's edits, in one file.
fn edited_lines(response: &str) -> Vec<u32> {
    response.match_indices(r#""range":{"start":{"line":"#).map(|(at, prefix)| {
        let rest = &response[at + prefix.len()..];
        rest[..rest.find(',').unwrap()].parse().unwrap()
    }).collect()
}

#[test]
fn table_fields_are_followed_through_aliases_and_modules_and_kept_apart() {
    let root = fixture();
    let module = root.join("counter.lua");
    let module_text = "local M = {}\nM.value = 1\nfunction M.bump() M.value = M.value + 1 end\nreturn M\n";
    fs::write(&module, module_text).unwrap();
    let main = root.join("main.lua");
    let text = "local record = { count = 1 }\nlocal alias = record\nprint(record.count, alias.count)\nrecord.count = 2\nprint(record[\"count\"])\n\nlocal other = { count = 7 }\nprint(other.count)\nlocal counter = require('counter')\ncounter.bump()\nprint(counter.value)\n";
    fs::write(&main, text).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4", "--module-root", root.to_str().unwrap()]);
    server.initialize(&root);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    // An alias's field is the original table's.
    let (line, character) = position(text, "count)", 0);
    let definition = server.request(2, "textDocument/definition", &main, line, character);
    assert!(definition.contains(r#""range":{"start":{"line":0,"character":17},"end":{"line":0,"character":22}}"#), "{definition}");
    // Its references are the constructor key, reads through either name, the write and the
    // constant-string index; not the other table's.
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"textDocument/references","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{character}}},"context":{{"includeDeclaration":true}}}}}}"#,
        uri(&main),
    ));
    let references = server.wait(|message| message.contains(r#""id":3,"#));
    assert_eq!(edited_lines(&references), [0, 2, 2, 3, 4], "{references}");
    let renamed = server.rename(4, &main, (line, character), "total");
    assert_eq!(edited_lines(&renamed), [0, 2, 2, 3, 4], "{renamed}");
    assert!(renamed.contains(r#""range":{"start":{"line":4,"character":14},"end":{"line":4,"character":19}}"#), "{renamed}");
    // The other table's field is its own.
    let other = server.rename(5, &main, position(text, "count = 7", 0), "total");
    assert_eq!(edited_lines(&other), [6, 7], "{other}");
    // A module's export is the module table's field, in both files.
    let exported = server.rename(6, &main, position(text, "value)", 0), "amount");
    assert_eq!(exported.matches(r#""newText":"amount""#).count(), 4, "{exported}");
    assert!(exported.contains(&uri(&module)), "{exported}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn renames_that_would_merge_or_capture_names_are_refused() {
    let root = fixture();
    let main = root.join("main.lua");
    let text = "local first = 1\nlocal second = 2\nprint(first, second)\nlocal function shadowed()\n    local print = 0\n    return Shared + print\nend\nShared = 1\nOther = 2\nlocal point = { x = 1, y = 2 }\nprint(point.x)\n\n---@param flag boolean\nlocal function pick(flag)\n    local a, b = { n = 1 }, { n = 2 }\n    local either = flag and a or b\n    return either.n, a.n\nend\n\n---@class Box\n---@field width number\n---@field height number\n\n---@class Crate\n\n---@generic T\n---@param value T\n---@return T | Crate\nlocal function keep(value) return value end\n";
    fs::write(&main, text).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    let refused = |response: &str, reason: &str| response.contains("-32803") && response.contains(reason);
    // A local renamed to another's name would refer to that one at `print(first, ...)`.
    let response = server.rename(2, &main, position(text, "first", 0), "second");
    assert!(refused(&response, "would change what the name at"), "{response}");
    // A global renamed to a local's name would be captured by it, and to another global's
    // would merge the two.
    let response = server.rename(3, &main, position(text, "Shared", 1), "print");
    assert!(refused(&response, "already exists") || refused(&response, "would change"), "{response}");
    let response = server.rename(4, &main, position(text, "Shared", 1), "Other");
    assert!(refused(&response, "a global named Other already exists"), "{response}");
    // Renaming a name to what it is, or to a fresh name, is fine.
    let response = server.rename(5, &main, position(text, "first", 0), "initial");
    assert_eq!(response.matches(r#""newText":"initial""#).count(), 2, "{response}");
    // A table's field renamed to another of its fields would merge them.
    let response = server.rename(6, &main, position(text, "x)", 0), "y");
    assert!(refused(&response, "already has a member named y"), "{response}");
    // A use that may be either of two tables' fields cannot be renamed for one of them.
    let response = server.rename(7, &main, position(text, "n = 1", 0), "count");
    assert!(refused(&response, "may be another table's n too"), "{response}");
    // A class's field renamed to another of its fields; a type to a built-in or another type;
    // a generic to a name its block uses.
    let response = server.rename(8, &main, position(text, "width", 0), "height");
    assert!(refused(&response, "Box already has a member named height"), "{response}");
    let response = server.rename(9, &main, position(text, "Box", 0), "number");
    assert!(refused(&response, "number is a built-in type"), "{response}");
    let response = server.rename(10, &main, position(text, "Box", 0), "Crate");
    assert!(refused(&response, "a type named Crate is already declared"), "{response}");
    let response = server.rename(11, &main, position(text, "T\n", 0), "Crate");
    assert!(refused(&response, "would change what the type name at"), "{response}");
    let response = server.rename(12, &main, position(text, "T\n", 0), "Value");
    assert_eq!(response.matches(r#""newText":"Value""#).count(), 3, "{response}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn library_declarations_are_navigable_but_not_renamed() {
    let root = fixture();
    let library = root.join("api.def.lua");
    fs::write(&library, "---@class tts__Vector\n---@field x number\n\n---@class tts__Object\n---@field tag string\n\n---@function tts__Object.getPosition(): tts__Vector\n\n---@param guid string\n---@return tts__Object\nfunction getObjectFromGUID(guid) end\n\n---@type nil | fun(player_color: string, object: tts__Object): void\n_G.onObjectDrop = nil\n").unwrap();
    let main = root.join("main.lua");
    let text = "function onObjectDrop(colour, dropped)\n    local position = dropped.getPosition()\n    print(position.x, dropped.tag)\n    local found = getObjectFromGUID('abc')\nend\n\nlocal function lookup()\n    local getObjectFromGUID = 1\n    return getObjectFromGUID\nend\n";
    fs::write(&main, text).unwrap();
    let library_argument = library.to_string_lossy().into_owned();
    let mut server = Server::start(&["--lua-version", "5.2", "--library", &library_argument]);
    server.initialize(&root);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    // A bodyless `---@function`'s definition is its declaration in the library.
    let (line, character) = position(text, "getPosition", 0);
    let definition = server.request(2, "textDocument/definition", &main, line, character + 1);
    assert!(definition.contains(&uri(&library)) && definition.contains(r#""start":{"line":6,"character":25}"#), "{definition}");
    // The library's members, types, functions and callbacks are not the project's to rename.
    let refused = |response: &str| response.contains("-32803") && response.contains("declared by a library");
    for (id, needle) in [(3, "getPosition"), (4, "tag)"), (5, "x,"), (6, "getObjectFromGUID('"), (7, "onObjectDrop")] {
        let (line, character) = position(text, needle, 0);
        let prepared = server.request(id, "textDocument/prepareRename", &main, line, character + 1);
        assert!(refused(&prepared), "{needle}: {prepared}");
        let renamed = server.rename(id + 10, &main, (line, character + 1), "renamed");
        assert!(refused(&renamed), "{needle}: {renamed}");
    }
    // A local that shadows a library's name is the project's own.
    let renamed = server.rename(30, &main, position(text, "getObjectFromGUID = 1", 0), "found");
    assert_eq!(renamed.matches(r#""newText":"found""#).count(), 2, "{renamed}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn expected_values_complete_function_implementations_and_literals() {
    let root = fixture();
    let library = root.join("api.def.lua");
    fs::write(&library, concat!(
        "---@class tts__Object\n\n",
        "---@shape tts__Callback<T>\n---@field callback_function nil | T\n\n",
        "---@alias tts__ObjectCallbackFunction fun(object: tts__Object): void\n\n",
        "---@shape tts__SpawnObjectDataParams : tts__Callback<tts__ObjectCallbackFunction>\n---@field data table\n\n",
        "---@param params tts__SpawnObjectDataParams\n---@return tts__Object\nfunction spawnObjectData(params) end\n\n",
        "---@class tts__Timer\n---@field onTick nil | fun(elapsed: number, timer: tts__Timer): boolean\n\n",
        "---@alias Colour \"Red\" | \"Blue\" | \"Light Grey\"\n\n",
        "---@param colour Colour\n---@param callback fun(id: number)\n---@param delay number\nfunction schedule(colour, callback, delay) end\n",
    )).unwrap();
    let main = root.join("main.lua");
    fs::write(&main, "").unwrap();
    let library_argument = library.to_str().unwrap().to_owned();
    let mut server = Server::start(&["--lua-version", "5.2", "--library", &library_argument]);
    // Call snippets are turned off; implementations still place the caret in their body.
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{}","capabilities":{{"textDocument":{{"completion":{{"completionItem":{{"snippetSupport":true}}}}}}}},"initializationOptions":{{"completion":{{"callSnippets":false}}}}}}}}"#, uri(&root)));
    server.wait(|message| message.contains(r#""id":1"#));
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    server.open(&main, "");
    server.diagnostics(&main, |_| true);
    let mut version = 1;
    let mut complete = |server: &mut Server, text: &str, needle: &str| {
        version += 1;
        server.change(&main, version, text);
        let (line, character) = position(text, needle, 0);
        server.request(version + 10, "textDocument/completion", &main, line, character + needle.encode_utf16().count() as u32)
    };
    let implements = |response: &str, label: &str, detail: &str| {
        response.contains(&format!(r#""label":"{label}","kind":15"#))
            && response.contains(&format!(r#""newText":"{label}\n\t$0\nend"}},"filterText":"{label}","insertTextFormat":2,"sortText":"0"#))
            && response.contains(&format!(r#""detail":"{detail}"#))
    };
    // A table field's function type, inherited from a generic shape through an alias.
    let response = complete(&mut server, "local object = spawnObjectData({\n    data = {\n        Name = \"rpg_BEAR\",\n    },\n    callback_function = \n})\n", "callback_function = ");
    assert!(implements(&response, "function(object)", "fun(object: tts__Object): void"), "{response}");
    // A field assigned: a class's, and a table's inferred one.
    let response = complete(&mut server, "---@type tts__Timer\nlocal timer\ntimer.onTick = \n", "onTick = ");
    assert!(implements(&response, "function(elapsed, timer)", "fun(elapsed: number, timer: tts__Timer): boolean"), "{response}");
    let response = complete(&mut server, "local t = { onTick = function(elapsed) return elapsed end }\nt.onTick = \n", "t.onTick = ");
    assert!(response.contains(r#""label":"function(elapsed)""#), "{response}");
    // An argument the call cannot take yet: its parameter's function type, or its literals, first.
    let response = complete(&mut server, "schedule(\"Red\", \n", "\"Red\", ");
    assert!(implements(&response, "function(id)", "fun(id: number)"), "{response}");
    let response = complete(&mut server, "schedule(\n", "schedule(");
    for literal in ["Red", "Blue", "Light Grey"] {
        assert!(response.contains(&format!(r#""label":"\"{literal}\"","kind":12,"textEdit":{{"range":{{"start":{{"line":0,"character":9}},"end":{{"line":0,"character":9}}}},"newText":"\"{literal}\""}},"sortText":"0000"#)), "{response}");
    }
    // Nothing for a value expected to be a number.
    let response = complete(&mut server, "schedule(\"Red\", function() end, \n", "end, ");
    assert!(!response.contains(r#""kind":15"#) && !response.contains(r#""kind":12"#), "{response}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn opening_documents_warms_queries_without_answering_anything() {
    let root = fixture();
    let main = root.join("main.lua");
    let other = root.join("other.lua");
    fs::write(&main, "local value = 1\n").unwrap();
    fs::write(&other, "return 2\n").unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    // Each open warms the query thread; only requests are answered.
    server.open(&main, "local value = 1\n");
    server.open(&other, "return 2\n");
    let response = server.request(2, "textDocument/completion", &main, 1, 0);
    assert!(response.contains(r#""label":"value""#), "{response}");
    server.send(r#"{"jsonrpc":"2.0","id":99,"method":"shutdown"}"#);
    loop {
        let message = server.messages.recv_timeout(Duration::from_secs(60)).expect("a language server message");
        assert!(!message.contains(r#""id":null"#) && !message.contains(r#""id":2,"#), "{message}");
        if message.contains(r#""id":99"#) { break; }
    }
    server.send(r#"{"jsonrpc":"2.0","method":"exit"}"#);
    assert!(server.child.wait().unwrap().success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_details_are_resolved_for_the_item_shown() {
    let root = fixture();
    let main = root.join("main.lua");
    let text = "--- How many there are.\n---@deprecated Use total.\nlocal count = 1\n---@param by number\nlocal function grow(by) return by end\n\n";
    fs::write(&main, text).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    // A client that resolves an item's detail and documentation gets a list without them.
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{}","capabilities":{{"textDocument":{{"completion":{{"completionItem":{{"snippetSupport":true,"tagSupport":{{"valueSet":[1]}},"resolveSupport":{{"properties":["documentation","detail"]}}}}}}}}}}}}}}"#, uri(&root)));
    let initialized = server.wait(|message| message.contains(r#""id":1"#));
    assert!(initialized.contains(r#""resolveProvider":true"#), "{initialized}");
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    let list = server.request(2, "textDocument/completion", &main, 5, 0);
    let item = |label: &str| {
        let start = list.find(&format!(r#"{{"label":"{label}""#)).unwrap_or_else(|| panic!("{label}: {list}"));
        let mut depth = 0;
        let end = start + list[start..].char_indices().find(|&(_, c)| { depth += match c { '{' => 1, '}' => -1, _ => 0 }; depth == 0 }).unwrap().0 + 1;
        list[start..end].to_owned()
    };
    let count = item("count");
    assert!(!count.contains(r#""detail""#) && !count.contains(r#""documentation""#), "{count}");
    // What the list needs stays: the deprecation tag, and a function's call.
    assert!(count.contains(r#""tags":[1]"#) && count.contains(r#""data":{"uri""#), "{count}");
    assert!(item("grow").contains(r#""newText":"grow(${1:by})$0""#), "{list}");
    // Resolving the item fills its details in.
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":3,"method":"completionItem/resolve","params":{count}}}"#));
    let resolved = server.wait(|message| message.contains(r#""id":3,"#));
    assert!(resolved.contains(r#""label":"count""#) && resolved.contains(r#""detail":"number""#), "{resolved}");
    assert!(resolved.contains("**Deprecated:** Use total.") && resolved.contains("How many there are."), "{resolved}");
    assert!(resolved.contains(r#""tags":[1]"#) && !resolved.contains(r#""data""#), "{resolved}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_and_hover_answer_in_a_file_with_syntax_errors_elsewhere() {
    let root = fixture();
    let main = root.join("main.lua");
    // Another line is being typed (`string.`), and the chunk has a statement after its return.
    let text = "local count = 1\nlocal half = string.\nlocal value = \nreturn count\nlocal after = 2\n";
    fs::write(&main, text).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, text);
    // The syntax errors are reported, and nothing else.
    let diagnostics = server.diagnostics(&main, |_| true);
    assert!(diagnostics.contains("LUA1001") && !diagnostics.contains("TS"), "{diagnostics}");
    let (line, character) = position(text, "value = ", 0);
    let completion = server.request(2, "textDocument/completion", &main, line, character + 8);
    assert!(completion.contains(r#""label":"count""#) && completion.contains(r#""label":"half""#), "{completion}");
    let (line, character) = position(text, "count", 1);
    let hover = server.request(3, "textDocument/hover", &main, line, character + 1);
    assert!(hover.contains("number"), "{hover}");
    let members = server.request(4, "textDocument/completion", &main, 1, 20);
    assert!(members.contains(r#""label":"format""#), "{members}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_host_module_map_resolves_requires_for_queries() {
    let root = fixture();
    let main = root.join("main.lua");
    let text = "local Mod = require('Virtual Module')\nlocal value = Mod.\nlocal upper, lower = require('Case'), require('case')\n";
    fs::write(&main, text).unwrap();
    fs::write(root.join("m1.lua"), "return { answer = 42, name = 'x' }\n").unwrap();
    fs::write(root.join("m2.lua"), "return { other = 1 }\n").unwrap();
    let manifest = root.join("map.json");
    let map = |module| format!(r#"{{"version": 1, "sources": [{{"file": "main.lua", "imports": {{"Virtual Module": "{module}", "Case": "m1.lua", "case": "m2.lua"}}}}]}}"#);
    fs::write(&manifest, map("m1.lua")).unwrap();
    let manifest_argument = manifest.to_string_lossy().into_owned();
    let mut server = Server::start(&["--lua-version", "5.2", "--stdlib", "require", "--module-map", &manifest_argument]);
    server.initialize(&root);
    server.open(&main, text);
    server.diagnostics(&main, |_| true);
    let response = server.request(2, "textDocument/completion", &main, 1, 18);
    assert!(response.contains(r#""label":"answer""#) && response.contains(r#""label":"name""#), "{response}");
    // Names differing only in case are different modules.
    let response = server.request(3, "textDocument/definition", &main, 2, 31);
    assert!(response.contains(&uri(&root.join("m1.lua"))), "{response}");
    let response = server.request(4, "textDocument/definition", &main, 2, 47);
    assert!(response.contains(&uri(&root.join("m2.lua"))), "{response}");
    // A host that rewrites its manifest need not also report it as a watched file: queries
    // read the current one.
    fs::write(&manifest, map("m2.lua")).unwrap();
    let response = server.request(5, "textDocument/completion", &main, 1, 18);
    assert!(response.contains(r#""label":"other""#) && !response.contains(r#""label":"answer""#), "{response}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_unicode_members_are_completed_at_utf16_positions_and_replace_typed_unicode() {
    let root = fixture();
    let main = root.join("unicode.lua");
    let original = include_str!("../../luanalysis-fixtures/tests/legacy/embedded/ParsingTest/test_parse_chinese_non_java_characters/test_parse_chinese_characters.lua");
    let text = original.replace("--[[caret]]", "");
    fs::write(&main, &text).unwrap();
    let mut server = Server::start(&["--lua-version", "5.3"]);
    server.initialize(&root);
    server.open(&main, &text);
    let needle = "天晓得我是什么.";
    let (line, column) = position(&text, needle, 0);
    let column = column + needle.encode_utf16().count() as u32;
    let response = server.request(2, "textDocument/completion", &main, line, column);
    assert!(response.contains(r#""label":"名字""#), "{response}");
    let typed = text.replacen(needle, &format!("{needle}名字"), 1);
    server.change(&main, 2, &typed);
    let response = server.request(3, "textDocument/completion", &main, line, column + 1);
    assert!(response.contains(r#""label":"名字""#), "{response}");
    assert!(response.contains(&format!(r#""start":{{"line":{line},"character":{column}}}"#)), "{response}");
    assert!(response.contains(&format!(r#""end":{{"line":{line},"character":{}}}"#, column + 2)), "{response}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn captured_writes_clear_stale_condition_diagnostics_and_hover_narrowing() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "local ready=false\nlocal function enable() ready=true end\nenable()\nif ready then print('taken') end\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    let published = server.diagnostics(&main, |message| message.contains(r#""version":1"#));
    assert!(published.contains(r#""diagnostics":[]"#), "{published}");
    let hover = server.request(2, "textDocument/hover", &main, 3, 4);
    assert!(hover.contains("boolean"), "{hover}");
    let unchanged = source.replace("ready=true end", "print('unchanged') end");
    server.change(&main, 2, &unchanged);
    let published = server.diagnostics(&main, |message| message.contains(r#""version":2"#));
    assert!(published.contains("LUA2873"), "{published}");
    server.change(&main, 3, source);
    let published = server.diagnostics(&main, |message| message.contains(r#""version":3"#));
    assert!(published.contains(r#""diagnostics":[]"#), "{published}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn escaped_literal_fields_widen_live_and_readonly_names_are_hinted() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "local state={ready=false}\n---@param s {ready: boolean}\nlocal function enable(s) s.ready=true end\nenable(state)\nif state.ready then print('taken') end\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    let published = server.diagnostics(&main, |message| message.contains(r#""version":1"#));
    assert!(published.contains(r#""diagnostics":[]"#), "{published}");
    let hover = server.request(2, "textDocument/hover", &main, 4, 10);
    assert!(hover.contains("boolean"), "{hover}");
    // A read-only view cannot assign the field, which stays false.
    let readonly = source.replace("{ready: boolean}", "{readonly ready: boolean}").replace("s.ready=true", "print(s.ready)");
    server.change(&main, 2, &readonly);
    let published = server.diagnostics(&main, |message| message.contains(r#""version":2"#));
    assert!(published.contains("LUA2873"), "{published}");
    let reserved = "---@class Named\n---@field readonly boolean\nlocal Named={readonly=false}\nreturn Named\n";
    server.change(&main, 3, reserved);
    let published = server.diagnostics(&main, |message| message.contains(r#""version":3"#));
    assert!(published.contains("LUA4004"), "{published}");
    server.change(&main, 4, source);
    let published = server.diagnostics(&main, |message| message.contains(r#""version":4"#));
    assert!(published.contains(r#""diagnostics":[]"#), "{published}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_typeof_name_goes_to_its_variable_and_hovers_its_type() {
    let root = fixture();
    let main = root.join("main.lua");
    let source = "local Colors = { Red = 1 }\n---@type typeof Colors\nlocal copy = { Red = 1 }\nreturn copy\n";
    fs::write(&main, source).unwrap();
    let mut server = Server::start(&["--lua-version", "5.4"]);
    server.initialize(&root);
    server.open(&main, source);
    let published = server.diagnostics(&main, |message| message.contains(r#""version":1"#));
    assert!(published.contains(r#""diagnostics":[]"#), "{published}");
    let definition = server.request(2, "textDocument/definition", &main, 1, 17);
    assert!(definition.contains(r#""range":{"start":{"line":0,"character":6},"end":{"line":0,"character":12}}"#), "{definition}");
    let hover = server.request(3, "textDocument/hover", &main, 1, 17);
    assert!(hover.contains(r#"{ Red: 1 }"#), "{hover}");
    server.finish();
    fs::remove_dir_all(root).unwrap();
}
