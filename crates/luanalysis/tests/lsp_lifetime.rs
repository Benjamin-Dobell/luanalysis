//! A host that stops a language server gets back control only when every thread the server
//! started has ended: a check in progress is cancelled rather than left to finish.
use luanalysis::{cli::CheckInputs, lsp::LanguageServer, project_check::ProjectCheckOptions};
use std::{fs, sync::{atomic::Ordering, mpsc}, time::{Duration, Instant}};

#[test]
fn stopping_a_server_ends_a_check_in_progress_and_every_worker() {
    let root = std::env::temp_dir().join(format!("luanalysis lsp lifetime {}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    // Enough source that its check is still running when the server is stopped.
    let mut body = String::new();
    for index in 0..200 {
        body.push_str(&format!(
            "---@param values number[]\n---@return number\nlocal function total{index}(values)\n    local sum = 0\n    for _, value in ipairs(values) do\n        if value > {index} then sum = sum + value * 2 else sum = sum - value end\n    end\n    return sum\nend\n",
        ));
    }
    for file in 0..40 {
        fs::write(root.join(format!("module{file}.lua")), &body).unwrap();
    }
    let main = root.join("module0.lua");
    let (sender, messages) = mpsc::channel::<String>();
    let inputs = CheckInputs { files: vec![root.clone()], libraries: Vec::new(), module_roots: Vec::new(), package_path: None, module_map: None };
    let server = LanguageServer::start(inputs, ProjectCheckOptions { jobs: 2, ..ProjectCheckOptions::default() }, move |message| {
        let _ = sender.send(message.to_owned());
    });
    let running = server.running_workers();
    let uri = format!("file://{}", main.display()).replace(' ', "%20");
    server.send(&format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":null,"capabilities":{{"window":{{"workDoneProgress":true}}}}}}}}"#));
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"lua","version":1,"text":"local x = 1\n"}}}}}}"#,
    ));
    // The server shows progress as soon as it hands the check to its checking thread.
    loop {
        let message = messages.recv_timeout(Duration::from_secs(60)).expect("the check to begin");
        if message.contains("window/workDoneProgress/create") { break; }
    }
    assert!(running.load(Ordering::SeqCst) > 0);
    assert_eq!(server.stop(), 1);
    assert_eq!(running.load(Ordering::SeqCst), 0, "a checking or query thread outlived stop");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stopping_a_server_ends_work_within_one_large_file_promptly() {
    let root = std::env::temp_dir().join(format!("luanalysis lsp lifetime large {}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    // One file whose recipes alone take seconds to build: work within a file stops too.
    let body: String = (0..18000).map(|index| format!("local function f{index}(value) local record = {{name = value, count = {index}}}; return record end\n")).collect();
    let main = root.join("main.lua");
    fs::write(&main, &body).unwrap();
    let (sender, messages) = mpsc::channel::<String>();
    let inputs = CheckInputs { files: vec![root.clone()], libraries: Vec::new(), module_roots: Vec::new(), package_path: None, module_map: None };
    let server = LanguageServer::start(inputs, ProjectCheckOptions { jobs: 2, ..ProjectCheckOptions::default() }, move |message| {
        let _ = sender.send(message.to_owned());
    });
    let running = server.running_workers();
    let uri = format!("file://{}", main.display()).replace(' ', "%20");
    server.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":null,"capabilities":{"window":{"workDoneProgress":true}}}}"#);
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    let text = body.replace('\n', "\\n");
    server.send(&format!(r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"lua","version":1,"text":"{text}"}}}}}}"#));
    loop {
        let message = messages.recv_timeout(Duration::from_secs(60)).expect("the check to begin");
        if message.contains("window/workDoneProgress/create") { break; }
    }
    std::thread::sleep(Duration::from_millis(200));
    let stopping = Instant::now();
    assert_eq!(server.stop(), 1);
    assert!(stopping.elapsed() < Duration::from_secs(3), "stopping took {:?}", stopping.elapsed());
    assert_eq!(running.load(Ordering::SeqCst), 0, "a checking or query thread outlived stop");
    fs::remove_dir_all(root).unwrap();
}
