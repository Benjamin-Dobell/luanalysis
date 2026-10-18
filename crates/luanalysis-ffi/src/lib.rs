//! The C interface of Luanalysis (`include/luanalysis.h`): checking, and the language server in
//! the calling process. No function unwinds into its caller: a panic is caught and reported as
//! the function's failure value.
#![allow(non_camel_case_types)]

use checker::cli::{Parsed, check_json, parse_arguments};
use checker::lsp::LanguageServer;
use std::ffi::{CStr, CString, OsString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A language server running in this process (`luanalysis_server_start`).
pub struct luanalysis_server {
    server: LanguageServer,
}

/// Receives each message for the client: `message` is `length` bytes of UTF-8 JSON, followed by
/// a NUL, valid only during the call.
pub type luanalysis_message_callback = Option<unsafe extern "C" fn(user_data: *mut c_void, message: *const c_char, length: usize)>;

/// The caller's callback and its data, called from the server's thread.
struct Callback {
    function: unsafe extern "C" fn(*mut c_void, *const c_char, usize),
    user_data: *mut c_void,
}

// The caller promises (`luanalysis.h`) that its callback may run on another thread.
unsafe impl Send for Callback {}
unsafe impl Sync for Callback {}

impl Callback {
    fn call(&self, message: &str) {
        let Ok(message) = CString::new(message) else { return };
        // SAFETY: the caller supplied a callback that accepts this NUL-terminated message.
        unsafe { (self.function)(self.user_data, message.as_ptr(), message.as_bytes().len()) }
    }
}

/// The library's version, as `luanalysis --version` prints it.
#[unsafe(no_mangle)]
pub extern "C" fn luanalysis_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// `count` NUL-terminated UTF-8 strings, or `None` when one is not UTF-8.
///
/// # Safety
/// `arguments` must point to `count` valid NUL-terminated strings, or `count` must be 0.
unsafe fn arguments(arguments: *const *const c_char, count: usize) -> Option<Vec<OsString>> {
    if count == 0 { return Some(Vec::new()); }
    if arguments.is_null() { return None; }
    // SAFETY: the caller supplies `count` string pointers.
    let pointers = unsafe { std::slice::from_raw_parts(arguments, count) };
    pointers.iter().map(|&pointer| {
        if pointer.is_null() { return None; }
        // SAFETY: each pointer is a NUL-terminated string.
        let text = unsafe { CStr::from_ptr(pointer) }.to_str().ok()?;
        Some(OsString::from(text))
    }).collect()
}

/// Checks once, as `luanalysis check --format json <arguments>` does, and returns the JSON
/// report, or NULL when an argument is not UTF-8 or checking failed internally. Free the report
/// with `luanalysis_string_free`.
///
/// # Safety
/// `arguments` must point to `count` NUL-terminated strings, or `count` must be 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luanalysis_check(arguments: *const *const c_char, count: usize) -> *mut c_char {
    // SAFETY: forwarded from the caller.
    let Some(arguments) = (unsafe { self::arguments(arguments, count) }) else { return std::ptr::null_mut() };
    catch_unwind(AssertUnwindSafe(|| check_json(arguments)))
        .ok()
        .and_then(|report| CString::new(report).ok())
        .map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Frees a string this library returned. NULL is ignored.
///
/// # Safety
/// `string` must be NULL or a string from this library not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luanalysis_string_free(string: *mut c_char) {
    if !string.is_null() {
        // SAFETY: the string came from `CString::into_raw` in this library.
        drop(unsafe { CString::from_raw(string) });
    }
}

/// Starts a language server, as `luanalysis lsp <arguments>` does, without stdio: the client's
/// messages go to `luanalysis_server_send`, and each message for the client goes to `callback`,
/// in order, on a thread of the server's. Returns NULL for invalid arguments.
///
/// # Safety
/// `arguments` must point to `count` NUL-terminated strings, or `count` must be 0. `callback`
/// must be safe to call from another thread with `user_data` until `luanalysis_server_stop`
/// returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luanalysis_server_start(
    arguments: *const *const c_char,
    count: usize,
    callback: luanalysis_message_callback,
    user_data: *mut c_void,
) -> *mut luanalysis_server {
    let Some(function) = callback else { return std::ptr::null_mut() };
    // SAFETY: forwarded from the caller.
    let Some(arguments) = (unsafe { self::arguments(arguments, count) }) else { return std::ptr::null_mut() };
    let callback = Callback { function, user_data };
    catch_unwind(AssertUnwindSafe(move || {
        let Ok(Parsed::Run(arguments)) = parse_arguments(arguments) else { return std::ptr::null_mut() };
        let server = LanguageServer::start(arguments.inputs, arguments.options, move |message| callback.call(message));
        Box::into_raw(Box::new(luanalysis_server { server }))
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Delivers one message from the client: `length` bytes of UTF-8 JSON (a JSON-RPC message
/// without its Content-Length header). Returns 0, or -1 when the server has stopped or the
/// message is not UTF-8. May be called from any thread.
///
/// # Safety
/// `server` must come from `luanalysis_server_start` and not be stopped; `message` must point to
/// `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luanalysis_server_send(server: *const luanalysis_server, message: *const c_char, length: usize) -> c_int {
    if server.is_null() || (message.is_null() && length != 0) { return -1; }
    // SAFETY: the caller supplies `length` bytes.
    let bytes = if length == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(message.cast::<u8>(), length) } };
    let Ok(message) = std::str::from_utf8(bytes) else { return -1 };
    // SAFETY: a live server from `luanalysis_server_start`.
    let server = unsafe { &*server };
    match catch_unwind(AssertUnwindSafe(|| server.server.send(message))) {
        Ok(true) => 0,
        _ => -1,
    }
}

/// Ends the client's stream, waits for the server, and frees it. Returns the server's exit
/// code: 0 after the client's `shutdown` request and `exit` notification, 1 otherwise. NULL is
/// ignored and returns 1.
///
/// # Safety
/// `server` must be NULL or come from `luanalysis_server_start` and not be stopped already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luanalysis_server_stop(server: *mut luanalysis_server) -> c_int {
    if server.is_null() { return 1; }
    // SAFETY: the server came from `Box::into_raw` in `luanalysis_server_start`.
    let server = unsafe { Box::from_raw(server) };
    catch_unwind(AssertUnwindSafe(move || server.server.stop())).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;

    fn fixture(name: &str, source: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("luanalysis ffi {} {name}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("main.lua"), source).unwrap();
        std::fs::canonicalize(directory).unwrap()
    }

    #[test]
    fn check_returns_the_json_report_and_frees_it() {
        let directory = fixture("check", "---@type number\nlocal value = 'text'\n");
        let arguments: Vec<CString> = ["--lua-version", "5.4", directory.join("main.lua").to_str().unwrap()]
            .iter().map(|argument| CString::new(*argument).unwrap()).collect();
        let pointers: Vec<*const c_char> = arguments.iter().map(|argument| argument.as_ptr()).collect();
        let report = unsafe { luanalysis_check(pointers.as_ptr(), pointers.len()) };
        assert!(!report.is_null());
        let text = unsafe { CStr::from_ptr(report) }.to_str().unwrap().to_owned();
        unsafe { luanalysis_string_free(report) };
        assert!(text.contains(r#""exitCode":1"#) && text.contains(r#""code":"TS2322""#), "{text}");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_host_names_the_standard_library_it_provides() {
        let directory = fixture("allow list", "local now = os.time()\nlocal handle = io.open('x')\n");
        let arguments: Vec<CString> = ["--lua-version", "5.2", "--stdlib", "basic,os.time", directory.join("main.lua").to_str().unwrap()]
            .iter().map(|argument| CString::new(*argument).unwrap()).collect();
        let pointers: Vec<*const c_char> = arguments.iter().map(|argument| argument.as_ptr()).collect();
        let report = unsafe { luanalysis_check(pointers.as_ptr(), pointers.len()) };
        let text = unsafe { CStr::from_ptr(report) }.to_str().unwrap().to_owned();
        unsafe { luanalysis_string_free(report) };
        assert!(text.contains(r#""message":"Cannot find name 'io'.""#), "{text}");
        assert_eq!(text.matches(r#""code":"#).count(), 1, "{text}");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_arguments_are_reported_in_the_json_report() {
        let argument = CString::new("--jobs").unwrap();
        let report = unsafe { luanalysis_check(&argument.as_ptr(), 1) };
        let text = unsafe { CStr::from_ptr(report) }.to_str().unwrap().to_owned();
        unsafe { luanalysis_string_free(report) };
        assert!(text.contains(r#""code":"LUA0000""#), "{text}");
    }

    unsafe extern "C" fn collect(user_data: *mut c_void, message: *const c_char, length: usize) {
        let messages = unsafe { &*(user_data as *const Mutex<mpsc::Sender<String>>) };
        let message = unsafe { std::slice::from_raw_parts(message.cast::<u8>(), length) };
        let _ = messages.lock().unwrap().send(String::from_utf8(message.to_vec()).unwrap());
    }

    #[test]
    fn a_server_answers_through_the_callback_and_exits_cleanly() {
        let directory = fixture("server", "local value = 1\nreturn value\n");
        let (sender, received) = mpsc::channel::<String>();
        let messages = Box::new(Mutex::new(sender));
        let arguments: Vec<CString> = ["--lua-version", "5.4"].iter().map(|argument| CString::new(*argument).unwrap()).collect();
        let pointers: Vec<*const c_char> = arguments.iter().map(|argument| argument.as_ptr()).collect();
        let user_data = &*messages as *const Mutex<mpsc::Sender<String>> as *mut c_void;
        let server = unsafe { luanalysis_server_start(pointers.as_ptr(), pointers.len(), Some(collect), user_data) };
        assert!(!server.is_null());
        let send = |message: &str| unsafe { luanalysis_server_send(server, message.as_ptr().cast(), message.len()) };
        let path = directory.display().to_string().replace('\\', "/");
        let uri = if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") };
        assert_eq!(send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{uri}","capabilities":{{}}}}}}"#)), 0);
        let initialized = received.recv_timeout(std::time::Duration::from_secs(60)).unwrap();
        assert!(initialized.contains(r#""id":1"#) && initialized.contains("completionProvider"), "{initialized}");
        assert!(initialized.contains(r#""renameProvider":{"prepareProvider":true}"#), "{initialized}");
        assert_eq!(send("not json"), 0);
        let error = received.recv_timeout(std::time::Duration::from_secs(60)).unwrap();
        assert!(error.contains("-32700"), "{error}");
        assert_eq!(send(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#), 0);
        let shutdown = loop {
            let message = received.recv_timeout(std::time::Duration::from_secs(60)).unwrap();
            if message.contains(r#""id":2"#) { break message; }
        };
        assert!(shutdown.contains(r#""result":null"#), "{shutdown}");
        assert_eq!(send(r#"{"jsonrpc":"2.0","method":"exit"}"#), 0);
        assert_eq!(unsafe { luanalysis_server_stop(server) }, 0);
        drop(messages);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
