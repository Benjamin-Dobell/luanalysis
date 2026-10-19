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
    utf8: bool,
}

impl Server {
    fn start(root: &Path) -> Self {
        Self::start_with_options(root, &[], None)
    }

    fn start_with_options(root: &Path, libraries: &[PathBuf], encoding: Option<&str>) -> Self {
        let inputs = CheckInputs {
            files: vec![root.to_path_buf()], libraries: libraries.to_vec(), module_roots: Vec::new(),
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
        let server = Self { server: Some(server), messages, gate, utf8: encoding == Some("utf-8") };
        let capabilities = encoding.map_or_else(|| "{}".to_owned(), |encoding| {
            format!(r#"{{"general":{{"positionEncodings":["{encoding}"]}}}}"#)
        });
        server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":null,"capabilities":{capabilities}}}}}"#));
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
        self.change_uri(&uri(path), version, text);
    }

    fn change_uri(&self, uri: &str, version: u32, text: &str) {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{}","version":{version}}},"contentChanges":[{{"text":"{}"}}]}}}}"#,
            uri, escape(text),
        ));
    }

    fn rename(&self, id: u32, path: &Path, text: &str, name: &str, nth: usize, new_name: &str) {
        self.rename_uri(id, &uri(path), text, name, nth, new_name);
    }

    fn position(&self, text: &str, name: &str, nth: usize) -> (usize, usize) {
        let offset = text.match_indices(name).nth(nth).unwrap().0;
        let line = text[..offset].matches('\n').count();
        let start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
        let character = if self.utf8 { offset - start } else { text[start..offset].encode_utf16().count() };
        (line, character)
    }

    fn rename_uri(&self, id: u32, uri: &str, text: &str, name: &str, nth: usize, new_name: &str) {
        let (line, character) = self.position(text, name, nth);
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/rename","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{character}}},"newName":"{}"}}}}"#,
            uri, escape(new_name),
        ));
    }

    fn prepare_rename(&self, id: u32, path: &Path, text: &str, name: &str, nth: usize) {
        let (line, character) = self.position(text, name, nth);
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/prepareRename","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":{line},"character":{character}}}}}}}"#,
            uri(path),
        ));
    }

    fn diagnostics(&self, path: &Path, version: u32) -> String {
        self.diagnostics_uri(&uri(path), version)
    }

    fn diagnostics_uri(&self, uri: &str, version: u32) -> String {
        self.wait(|message| diagnostics_match(message, uri, version))
    }

    /// Stop inside publication of the given check's diagnostics. Once they arrive, the client
    /// can enqueue changes and rename before releasing the server. It then installs that
    /// snapshot and drains those queued messages before scheduling another check, so these
    /// regressions cannot accidentally pass because the checker won a scheduling race.
    fn hold_diagnostics(&self, path: &Path, version: u32) -> Resume {
        self.hold_diagnostics_uri(&uri(path), version)
    }

    fn hold_diagnostics_uri(&self, uri: &str, version: u32) -> Resume {
        let (resume, wait) = mpsc::channel();
        let mut gate = self.gate.lock().unwrap();
        assert!(gate.is_none());
        *gate = Some(DiagnosticGate { uri: uri.to_owned(), version, resume: wait });
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

/// Apply the ranges returned by the server, translating LSP UTF-16 positions back to byte
/// offsets. In particular, a range after an emoji must not count either UTF-8 bytes or scalars.
fn apply_rename(response: &str, path: &Path, text: &str, old: &str, new: &str) -> String {
    apply_rename_to(response, path, text, old, new, &text.replace(old, new))
}

fn apply_rename_to(response: &str, path: &Path, text: &str, old: &str, new: &str, expected: &str) -> String {
    assert!(!response.contains(r#""error""#), "{response}");
    let key = format!(r#""{}":["#, uri(path));
    let file_edits = response.split_once(&key).expect(response).1.split(']').next().unwrap();
    let replacements = file_edits.matches(&format!(r#""newText":"{}""#, escape(new))).count();
    assert!(replacements > 0, "{response}");

    fn number(rest: &mut &str) -> usize {
        let length = rest.bytes().take_while(u8::is_ascii_digit).count();
        let value = rest[..length].parse().unwrap();
        *rest = &rest[length..];
        value
    }
    fn position(text: &str, line: usize, character: usize) -> usize {
        let start = if line == 0 { 0 } else { text.match_indices('\n').nth(line - 1).unwrap().0 + 1 };
        let contents = text[start..].split('\n').next().unwrap();
        let mut units = 0;
        for (offset, ch) in contents.char_indices() {
            if units == character { return start + offset; }
            units += ch.len_utf16();
        }
        assert_eq!(units, character, "invalid UTF-16 position {line}:{character}");
        start + contents.len()
    }

    let mut edits = Vec::new();
    for mut rest in file_edits.split(r#""range":{"start":{"line":"#).skip(1) {
        let start_line = number(&mut rest);
        // JSON here is emitted by the server in this fixed range shape.
        rest = rest.strip_prefix(r#","character":"#).unwrap();
        let start_character = number(&mut rest);
        rest = rest.strip_prefix(r#"},"end":{"line":"#).unwrap();
        let end_line = number(&mut rest);
        rest = rest.strip_prefix(r#","character":"#).unwrap();
        let end_character = number(&mut rest);
        let start = position(text, start_line, start_character);
        let end = position(text, end_line, end_character);
        assert_eq!(&text[start..end], old, "{response}");
        edits.push((start, end));
    }
    assert_eq!(edits.len(), replacements, "{response}");
    edits.sort_unstable();
    assert!(edits.windows(2).all(|pair| pair[0].1 <= pair[1].0), "{response}");
    let mut renamed = text.to_owned();
    for (start, end) in edits.into_iter().rev() { renamed.replace_range(start..end, new); }
    assert_eq!(renamed, expected);
    let parsed = luanalysis_syntax::parse(&renamed);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    renamed
}

fn check_renamed(server: &Server, path: &Path, version: u32, text: &str) {
    server.change(path, version, text);
    assert_clean_diagnostics(server, path, version);
}

fn assert_clean_diagnostics(server: &Server, path: &Path, version: u32) {
    let diagnostics = server.diagnostics(path, version);
    assert!(diagnostics.contains(r#""diagnostics":[]"#), "{diagnostics}");
}

fn assert_rename_error(response: &str, reason: &str) {
    assert!(response.contains(r#""code":-32803"#), "{response}");
    let data = response.split_once(r#""data":"#).expect(response).1;
    assert!(data.contains(r#""kind":"rename""#), "{response}");
    assert!(data.contains(&format!(r#""reason":"{reason}""#)), "{response}");
}

fn assert_related_location(response: &str, uri: &str, line: u32, start: u32, end: u32, kind: &str, role: &str) {
    let location = format!(
        r#""location":{{"uri":"{uri}","range":{{"start":{{"line":{line},"character":{start}}},"end":{{"line":{line},"character":{end}}}}}}}"#,
    );
    assert!(response.contains(&location), "{response}");
    assert_eq!(response.matches(r#""location":{"uri":"#).count(), 1, "{response}");
    assert!(response.contains(r#""origin":{"#), "{response}");
    assert!(response.contains(&format!(r#""kind":"{kind}""#)), "{response}");
    assert!(response.contains(&format!(r#""role":"{role}""#)), "{response}");
}

#[test]
fn checked_rename_capture_error_locates_the_original_affected_read() {
    let fixture = Fixture::new();
    let text = "print(1)\nlocal total = 1\nlocal occupied = 2\nprint(total, occupied)\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);
    server.rename(2, &main, text, "total", 0, "occupied");

    let refused = server.response(2);
    assert!(refused.contains("would change what the name at"), "{refused}");
    assert_rename_error(&refused, "capture");
    // The conflict is the original five-character total, before the longer replacement.
    assert_related_location(&refused, &uri(&main), 3, 6, 11, "filesystem", "implementation");
    server.finish();
}

fn dirty_capture_location(encoding: Option<&str>, start: u32, end: u32) {
    let fixture = Fixture::new();
    let text = "print(1)\r\nlocal total = 1\r\nlocal occupied = 2\r\nprint(total, occupied)\r\n";
    let main = fixture.file("main.lua", text);
    let alias = uri(&main).replace("main.lua", "%6dain.lua");
    let server = Server::start_with_options(&fixture.0, &[], encoding);
    let resume = server.hold_diagnostics_uri(&alias, 1);
    server.open_uri(&alias, text);
    let diagnostics = server.diagnostics_uri(&alias, 1);
    assert!(diagnostics.contains(r#""diagnostics":[]"#), "{diagnostics}");
    let edited = "-- unsaved\r\nprint(1)\r\nlocal café = 1\r\nlocal occupied = 2\r\nprint(\"😀\", café, occupied)\r\n";
    server.change_uri(&alias, 2, edited);
    server.rename_uri(2, &alias, edited, "café", 0, "occupied");
    drop(resume);

    let refused = server.response(2);
    assert!(refused.contains("would change what the name at"), "{refused}");
    assert_rename_error(&refused, "capture");
    assert_related_location(&refused, &alias, 4, start, end, "filesystem", "implementation");
    // A refused rename must leave the current overlay usable for the next request.
    server.rename_uri(3, &alias, edited, "café", 1, "résultat");
    let renamed = server.response(3);
    assert!(!renamed.contains(r#""error""#), "{renamed}");
    assert!(renamed.contains(&alias), "{renamed}");
    assert_eq!(renamed.matches(r#""newText":"résultat""#).count(), 2, "{renamed}");
    server.finish();
}

#[test]
fn dirty_rename_capture_error_preserves_alias_uri_and_utf16_crlf_positions() {
    dirty_capture_location(None, 12, 16);
}

#[test]
fn dirty_rename_capture_error_uses_negotiated_utf8_positions() {
    dirty_capture_location(Some("utf-8"), 14, 19);
}

#[test]
fn dirty_cross_file_global_conflict_error_locates_the_unsaved_target_definition() {
    let fixture = Fixture::new();
    let definition = "Shared = 1\nreturn Shared\n";
    let util = fixture.file("util.lua", definition);
    let text = "require('util')\nreturn Shared\n";
    let main = fixture.file("main.lua", text);
    let alias = uri(&util).replace("util.lua", "%75til.lua");
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open_uri(&alias, definition);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);
    let edited = "-- unsaved\r\nShared = 1\r\nprint(\"😀\"); 目标 = 2\r\nreturn Shared + 目标\r\n";
    server.change_uri(&alias, 2, edited);
    server.rename(2, &main, text, "Shared", 0, "目标");
    drop(resume);

    let refused = server.response(2);
    assert!(refused.contains("a global named 目标 already exists"), "{refused}");
    assert_rename_error(&refused, "nameConflict");
    assert_related_location(&refused, &alias, 2, 13, 15, "filesystem", "implementation");
    server.finish();
}

#[test]
fn native_member_and_type_conflict_errors_locate_original_source_sites() {
    let fixture = Fixture::new();
    let text = "local record = { café = 1, 数量 = 2 }\nprint(record.café)\n\n---@class Box\n---@field width number\n---@field height number\n\n---@class Crate\n\n---@param box Box\nlocal function read(box) return box end\nreturn read\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);
    server.rename(2, &main, text, "café", 1, "数量");
    let refused = server.response(2);
    assert!(refused.contains("already has a member named 数量"), "{refused}");
    assert_rename_error(&refused, "memberConflict");
    // An inferred table conflict identifies the original receiver/member declaration.
    assert_related_location(&refused, &uri(&main), 0, 17, 21, "filesystem", "implementation");

    server.rename(3, &main, text, "width", 0, "height");
    let refused = server.response(3);
    assert!(refused.contains("Box already has a member named height"), "{refused}");
    assert_rename_error(&refused, "memberConflict");
    assert_related_location(&refused, &uri(&main), 5, 10, 16, "filesystem", "implementation");

    server.rename(4, &main, text, "Box", 0, "Crate");
    let refused = server.response(4);
    assert!(refused.contains("a type named Crate is already declared"), "{refused}");
    assert_rename_error(&refused, "typeConflict");
    assert_related_location(&refused, &uri(&main), 7, 10, 15, "filesystem", "implementation");
    server.finish();
}

#[test]
fn prepare_and_rename_library_errors_preserve_original_declaration_provenance() {
    let fixture = Fixture::new();
    let library = fixture.file("api.def.lua", "---@param value number\n---@return number\nfunction external(value) end\n");
    let text = "print(1)\nreturn external(1)\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start_with_options(&fixture.0, &[library.clone()], None);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);

    for id in [2, 3] {
        if id == 2 { server.prepare_rename(id, &main, text, "external", 0); }
        else { server.rename(id, &main, text, "external", 0, "callExternal"); }
        let refused = server.response(id);
        assert!(refused.contains("declared by a library"), "{refused}");
        assert_rename_error(&refused, "libraryOwned");
        assert_related_location(&refused, &uri(&library), 2, 9, 17, "filesystem", "declaration");
    }

    for id in [4, 5] {
        if id == 4 { server.prepare_rename(id, &main, text, "print", 0); }
        else { server.rename(id, &main, text, "print", 0, "output"); }
        let refused = server.response(id);
        assert!(refused.contains("declared by a library"), "{refused}");
        assert_rename_error(&refused, "libraryOwned");
        assert_related_location(&refused, &uri(Path::new("stdlib/Lua54/global.def.lua")), 218, 9, 14, "declarationLibrary", "declaration");
        assert!(refused.contains(&format!(r#""identity":"luanalysis.std.Lua54.global@{}""#, env!("CARGO_PKG_VERSION"))), "{refused}");
    }
    server.finish();
}

#[test]
fn invalid_rename_error_has_no_invented_location_and_allows_a_valid_retry() {
    let fixture = Fixture::new();
    let text = "local total = 1\nreturn total\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);
    server.rename(2, &main, text, "total", 0, "bad-name");
    let refused = server.response(2);
    assert!(refused.contains("is not a Lua name"), "{refused}");
    assert_rename_error(&refused, "invalidName");
    assert!(refused.contains(r#""relatedInformation":[]"#), "{refused}");
    server.rename(3, &main, text, "total", 1, "quantity");
    let renamed = apply_rename(&server.response(3), &main, text, "total", "quantity");
    check_renamed(&server, &main, 2, &renamed);
    server.finish();
}

#[test]
fn checked_unicode_local_rename_preserves_utf16_ranges_in_both_directions() {
    let fixture = Fixture::new();
    let text = "local café = 1\nlocal marker = \"😀\"; print(marker); return café + café\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);

    // More UTF-8 bytes but fewer UTF-16 units, followed by the reverse transformation.
    server.rename(2, &main, text, "café", 1, "总计");
    let renamed = apply_rename(&server.response(2), &main, text, "café", "总计");
    check_renamed(&server, &main, 2, &renamed);
    server.rename(3, &main, &renamed, "总计", 2, "café");
    let restored = apply_rename(&server.response(3), &main, &renamed, "总计", "café");
    assert_eq!(restored, text);
    check_renamed(&server, &main, 3, &restored);
    server.finish();
}

#[test]
fn current_overlay_unicode_rename_uses_the_unsaved_tokens_and_utf16_positions() {
    let fixture = Fixture::new();
    let text = "local first = 1\nreturn first\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);
    let edited = "-- unsaved line\nlocal 数量 = 1\nlocal marker = \"😀\"; print(marker); return 数量 + 数量\n";
    server.change(&main, 2, edited);
    server.rename(2, &main, edited, "数量", 1, "résultat");
    drop(resume);

    let renamed = apply_rename(&server.response(2), &main, edited, "数量", "résultat");
    check_renamed(&server, &main, 3, &renamed);
    server.rename(3, &main, &renamed, "résultat", 0, "数量");
    let restored = apply_rename(&server.response(3), &main, &renamed, "résultat", "数量");
    assert_eq!(restored, edited);
    check_renamed(&server, &main, 4, &restored);
    server.finish();
}

#[test]
fn current_overlay_unicode_global_rename_moves_cross_file_ranges_and_preserves_shadows() {
    let fixture = Fixture::new();
    let definition = "总量 = 1\nreturn 总量\n";
    let util = fixture.file("util.lua", definition);
    let text = "require('util')\nlocal function shadow()\n    local 总量 = 2\n    return 总量\nend\nlocal marker = \"😀\"; print(marker, shadow()); return 总量 + _G.总量\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open(&util, definition);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);
    let edited_util = format!("-- unsaved definition\n{definition}");
    let edited_main = format!("-- unsaved caller\n{text}");
    server.change(&util, 2, &edited_util);
    server.change(&main, 2, &edited_main);
    server.rename(2, &main, &edited_main, "总量", 2, "café");
    drop(resume);

    let response = server.response(2);
    assert_eq!(response.matches(r#""newText":"café""#).count(), 4, "{response}");
    apply_rename(&response, &util, &edited_util, "总量", "café");
    let expected = edited_main.replace("return 总量 + _G.总量", "return café + _G.café");
    let renamed = apply_rename_to(&response, &main, &edited_main, "总量", "café", &expected);
    assert_eq!(renamed.matches("总量").count(), 2, "the local declaration and its read must stay unchanged");
    server.finish();
}

#[test]
fn unicode_global_rename_includes_dot_and_quoted_environment_members() {
    let fixture = Fixture::new();
    let text = "_G[\"总量\"] = 1\nlocal marker = \"😀\"; print(marker, _G.总量, _G[\"总量\"], _ENV.总量)\nreturn 总量 + 1\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);

    server.rename(2, &main, text, "总量", 4, "café");
    let renamed = apply_rename(&server.response(2), &main, text, "总量", "café");
    // A renamed bracket write must still declare the numeric global, so the bare read and
    // every environment spelling remain valid after applying the edits.
    check_renamed(&server, &main, 2, &renamed);
    server.rename(3, &main, &renamed, "café", 1, "合計");
    let renamed = apply_rename(&server.response(3), &main, &renamed, "café", "合計");
    check_renamed(&server, &main, 3, &renamed);
    server.finish();
}

#[test]
fn unicode_table_member_rename_keeps_dot_and_string_index_ranges() {
    let fixture = Fixture::new();
    let text = "local record = { café = 1, 別名 = 2 }\nlocal marker = \"😀\"; print(marker); return record.café + record[\"café\"]\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);

    server.rename(2, &main, text, "café", 1, "別名");
    let refused = server.response(2);
    assert!(refused.contains("-32803") && refused.contains("already has a member named 別名"), "{refused}");
    server.rename(3, &main, text, "café", 1, "数量");
    let renamed = apply_rename(&server.response(3), &main, text, "café", "数量");
    check_renamed(&server, &main, 2, &renamed);
    server.rename(4, &main, &renamed, "数量", 2, "étendue");
    let renamed = apply_rename(&server.response(4), &main, &renamed, "数量", "étendue");
    check_renamed(&server, &main, 3, &renamed);
    server.finish();
}

#[test]
fn unicode_class_and_declared_member_renames_keep_annotation_and_value_uses() {
    let fixture = Fixture::new();
    let text = "---@class Point\n---@field width number\n\n---@param point Point\nlocal function read(point)\n    local marker = \"😀\"; print(marker); return point.width\nend\nreturn read\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);

    server.rename(2, &main, text, "Point", 1, "幾何.位置");
    let renamed = apply_rename(&server.response(2), &main, text, "Point", "幾何.位置");
    check_renamed(&server, &main, 2, &renamed);
    server.rename(3, &main, &renamed, "width", 1, "幅");
    let renamed = apply_rename(&server.response(3), &main, &renamed, "width", "幅");
    check_renamed(&server, &main, 3, &renamed);
    server.rename(4, &main, &renamed, "幾何.位置", 0, "Vector");
    let renamed = apply_rename(&server.response(4), &main, &renamed, "幾何.位置", "Vector");
    check_renamed(&server, &main, 4, &renamed);
    server.rename(5, &main, &renamed, "幅", 0, "largeur");
    let renamed = apply_rename(&server.response(5), &main, &renamed, "幅", "largeur");
    check_renamed(&server, &main, 5, &renamed);
    server.finish();
}

#[test]
fn unicode_parameter_rename_updates_param_and_type_predicate_subject() {
    let fixture = Fixture::new();
    let text = "---@param value unknown\n---@return value is string\nlocal function isString(value)\n    return type(value) == 'string'\nend\nreturn isString\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);

    // Start at the predicate's subject, then rename the Unicode parameter from its body.
    server.rename(2, &main, text, "value", 1, "数据");
    let response = server.response(2);
    assert_eq!(response.matches(r#""newText":"数据""#).count(), 4, "{response}");
    let renamed = apply_rename(&response, &main, text, "value", "数据");
    check_renamed(&server, &main, 2, &renamed);
    server.rename(3, &main, &renamed, "数据", 3, "entrée");
    let renamed = apply_rename(&server.response(3), &main, &renamed, "数据", "entrée");
    check_renamed(&server, &main, 3, &renamed);
    server.finish();
}

#[test]
fn unicode_rename_preserves_distinct_normalization_forms_and_rejects_capture() {
    let fixture = Fixture::new();
    let text = "local café = 1\nlocal cafe\u{0301} = 2\nreturn café + cafe\u{0301}\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);

    server.rename(2, &main, text, "café", 0, "数量");
    let renamed = apply_rename(&server.response(2), &main, text, "café", "数量");
    assert_eq!(renamed.matches("cafe\u{0301}").count(), 2);
    check_renamed(&server, &main, 2, &renamed);
    server.rename(3, &main, &renamed, "数量", 0, "cafe\u{0301}");
    let refused = server.response(3);
    assert!(refused.contains("-32803") && refused.contains("would change what the name at"), "{refused}");
    server.rename(4, &main, &renamed, "cafe\u{0301}", 0, "résultat");
    let renamed = apply_rename(&server.response(4), &main, &renamed, "cafe\u{0301}", "résultat");
    check_renamed(&server, &main, 3, &renamed);
    server.finish();
}

#[test]
fn current_overlay_unicode_rename_rejects_new_capture() {
    let fixture = Fixture::new();
    let text = "local first = 1\nreturn first\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    let resume = server.hold_diagnostics(&main, 1);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);
    let edited = "local first = 1\nlocal 数量 = 2\nreturn first + 数量\n";
    server.change(&main, 2, edited);
    server.rename(2, &main, edited, "first", 0, "数量");
    drop(resume);

    let refused = server.response(2);
    assert!(refused.contains("-32803") && refused.contains("would change what the name at"), "{refused}");
    server.finish();
}

#[test]
fn unicode_rename_rejects_existing_globals_and_invalid_names() {
    let fixture = Fixture::new();
    let text = "源 = 1\n目标 = 2\nreturn 源 + 目标\n";
    let main = fixture.file("main.lua", text);
    let server = Server::start(&fixture.0);
    server.open(&main, text);
    assert_clean_diagnostics(&server, &main, 1);
    server.rename(2, &main, text, "源", 0, "目标");
    let refused = server.response(2);
    assert!(refused.contains("-32803") && refused.contains("a global named 目标 already exists"), "{refused}");

    for (index, name) in ["", "end", "has space", "bad-name", "1st", "café.field", "数量!"].into_iter().enumerate() {
        let id = index as u32 + 3;
        server.rename(id, &main, text, "源", 0, name);
        let refused = server.response(id);
        assert!(refused.contains("-32803") && refused.contains("is not a Lua name"), "{name:?}: {refused}");
    }
    server.finish();
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
    assert_rename_error(&refused, "invalidName");
    assert!(refused.contains(r#""relatedInformation":[]"#), "{refused}");
    server.finish();
}
