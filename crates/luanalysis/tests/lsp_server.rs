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
    server.send(r#"{"jsonrpc":"2.0","id":8,"method":"textDocument/formatting","params":{}}"#);
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
    assert!(standard.contains("declared by the standard library"), "{standard}");
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
    assert!(response.contains(r#""name":"Point:length","kind":6"#) && response.contains(&uri(&shapes)), "{response}");
    assert!(!response.contains(r#""name":"show""#), "{response}");
    server.finish();
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
    let source = "local config = { port = 8080 }\nlocal count = 1\nprint(config.prot, cuont)\n";
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
        uri(&main), diagnostic("TS2339", 13, 4), diagnostic("TS2304", 19, 5),
    ));
    let response = server.wait(|message| message.contains(r#""id":3,"#));
    assert!(response.contains(r#""title":"Change to 'port'""#) && response.contains(r#""newText":"port""#), "{response}");
    assert!(response.contains(r#""title":"Change to 'count'""#), "{response}");
    assert!(response.contains(r#""title":"Prefix 'count' with '_' to mark it unused""#) && response.contains(r#""newText":"_""#), "{response}");
    server.finish();
}
