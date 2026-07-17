use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn file_uri(path: &Path) -> String {
    format!(
        "file://{}",
        path.canonicalize().expect("canonical test path").display()
    )
}

fn frame(message: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).expect("serialize LSP message");
    let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    framed.extend(body);
    framed
}

fn parse_frames(output: &[u8]) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut offset = 0;
    while offset < output.len() {
        let header_end = output[offset..]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| offset + position)
            .unwrap_or_else(|| panic!("incomplete LSP header at byte {offset}"));
        let headers = std::str::from_utf8(&output[offset..header_end]).expect("UTF-8 LSP headers");
        let content_length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length:"))
            .expect("Content-Length header")
            .trim()
            .parse()
            .expect("numeric Content-Length");
        let body_start = header_end + 4;
        let body_end = body_start + content_length;
        assert!(
            body_end <= output.len(),
            "LSP body extends beyond captured stdout"
        );
        messages.push(
            serde_json::from_slice(&output[body_start..body_end]).expect("valid LSP JSON body"),
        );
        offset = body_end;
    }
    messages
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

fn position_params(uri: &str, line: u64, character: u64) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character }
    })
}

fn response<'a>(responses: &'a HashMap<u64, Value>, id: u64) -> &'a Value {
    responses
        .get(&id)
        .unwrap_or_else(|| panic!("missing LSP response id {id}"))
}

#[test]
fn native_assembly_features_work_through_freight_lsp() {
    let project = tempfile::tempdir().expect("temp project");
    std::fs::write(
        project.path().join("freight.toml"),
        "[package]\nname = \"asm-lsp-e2e\"\nversion = \"0.1.0\"\n\n[target]\narch = \"x86_64\"\n",
    )
    .expect("write manifest");

    let include_path = project.path().join("defs.inc");
    let main_path = project.path().join("main.s");
    let preprocessed_path = project.path().join("preprocessed.S");
    let asm_path = project.path().join("intel.asm");
    let nasm_path = project.path().join("standalone.nasm");

    let include_source = ".equ HEIGHT, 25\nincluded_label:\n    ret\n";
    let main_source = concat!(
        ".include \"defs.inc\"\n",
        ".text\n",
        ".globl main\n",
        ".equ WIDTH, 80\n",
        ".macro save reg\n",
        "    push \\reg\n",
        ".endm\n",
        "main:\n",
        "    mov $WIDTH, %eax\n",
        "    add $WIDTH, %eax\n",
        "    mov $HEIGHT, %ebx\n",
        "    call helper\n",
        "    jmp 1f\n",
        "1:\n",
        "    save %rbp\n",
        "    ret\n",
        "helper:\n",
        "    ret\n",
    );
    let preprocessed_source = "uppercase_s:\n    ret\n";
    let asm_source = concat!(
        "%define COUNT 4\n",
        "%macro saveall 0\n",
        "    push rax\n",
        "%endmacro\n",
        "intel_entry:\n",
        "    mov rax, COUNT\n",
    );
    let nasm_source = "nasm_entry:\n    ret\n";

    std::fs::write(&include_path, include_source).expect("write include");
    std::fs::write(&main_path, main_source).expect("write GAS source");
    std::fs::write(&preprocessed_path, preprocessed_source).expect("write .S source");
    std::fs::write(&asm_path, asm_source).expect("write .asm source");
    std::fs::write(&nasm_path, nasm_source).expect("write .nasm source");

    let root_uri = file_uri(project.path());
    let include_uri = file_uri(&include_path);
    let main_uri = file_uri(&main_path);
    let preprocessed_uri = file_uri(&preprocessed_path);
    let asm_uri = file_uri(&asm_path);
    let nasm_uri = file_uri(&nasm_path);

    let mut input = vec![
        request(
            1,
            "initialize",
            json!({
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {}
            }),
        ),
        notification("initialized", json!({})),
    ];
    for (uri, language_id, text) in [
        (&main_uri, "asm", main_source),
        (&preprocessed_uri, "asm", preprocessed_source),
        (&asm_uri, "nasm", asm_source),
        (&nasm_uri, "nasm", nasm_source),
    ] {
        input.push(notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text
                }
            }),
        ));
    }

    input.extend([
        request(2, "textDocument/hover", position_params(&main_uri, 8, 5)),
        request(3, "textDocument/hover", position_params(&main_uri, 8, 18)),
        request(4, "textDocument/hover", position_params(&main_uri, 10, 10)),
        request(
            5,
            "textDocument/definition",
            position_params(&main_uri, 10, 10),
        ),
        request(
            6,
            "textDocument/definition",
            position_params(&main_uri, 0, 12),
        ),
        request(
            7,
            "textDocument/definition",
            position_params(&main_uri, 12, 9),
        ),
        request(
            8,
            "textDocument/completion",
            position_params(&main_uri, 18, 0),
        ),
        request(
            9,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": main_uri } }),
        ),
        request(10, "workspace/symbol", json!({ "query": "height" })),
        request(
            11,
            "textDocument/foldingRange",
            json!({ "textDocument": { "uri": main_uri } }),
        ),
        request(
            12,
            "textDocument/references",
            json!({
                "textDocument": { "uri": main_uri },
                "position": { "line": 8, "character": 10 },
                "context": { "includeDeclaration": true }
            }),
        ),
        request(
            13,
            "textDocument/documentHighlight",
            position_params(&main_uri, 8, 10),
        ),
        request(
            14,
            "textDocument/selectionRange",
            json!({
                "textDocument": { "uri": main_uri },
                "positions": [{ "line": 8, "character": 10 }]
            }),
        ),
        request(
            15,
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": main_uri } }),
        ),
        request(
            16,
            "textDocument/rename",
            json!({
                "textDocument": { "uri": main_uri },
                "position": { "line": 8, "character": 10 },
                "newName": "LINE_WIDTH"
            }),
        ),
        request(17, "textDocument/hover", position_params(&main_uri, 2, 2)),
        request(
            18,
            "textDocument/references",
            json!({
                "textDocument": { "uri": main_uri },
                "position": { "line": 8, "character": 10 },
                "context": { "includeDeclaration": false }
            }),
        ),
        request(
            19,
            "textDocument/rename",
            json!({
                "textDocument": { "uri": main_uri },
                "position": { "line": 10, "character": 10 },
                "newName": "ROW_HEIGHT"
            }),
        ),
        request(
            20,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": preprocessed_uri } }),
        ),
        request(
            21,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": asm_uri } }),
        ),
        request(
            22,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": nasm_uri } }),
        ),
        request(
            23,
            "textDocument/references",
            json!({
                "textDocument": { "uri": main_uri },
                "position": { "line": 10, "character": 10 },
                "context": { "includeDeclaration": true }
            }),
        ),
    ]);

    let changed_source = ".equ LIVE, 1\ndup:\ndup:\n    mov $LIVE, %eax\n";
    input.push(notification(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": main_uri, "version": 2 },
            "contentChanges": [{ "text": changed_source }]
        }),
    ));
    input.push(request(
        30,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": main_uri } }),
    ));
    input.push(request(
        31,
        "textDocument/hover",
        position_params(&main_uri, 3, 10),
    ));
    input.push(request(99, "shutdown", json!(null)));
    input.push(notification("exit", json!(null)));

    let mut child = Command::new(env!("CARGO_BIN_EXE_freight"))
        .args(["lsp", "--no-clangd", "--no-asm-lsp"])
        .current_dir(project.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start freight lsp");
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        for message in &input {
            stdin.write_all(&frame(message)).expect("write LSP frame");
        }
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait for freight lsp");
    assert!(
        output.status.success(),
        "freight lsp failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let messages = parse_frames(&output.stdout);
    let responses: HashMap<u64, Value> = messages
        .iter()
        .filter_map(|message| Some((message.get("id")?.as_u64()?, message.get("result")?.clone())))
        .collect();

    let capabilities = &response(&responses, 1)["capabilities"];
    for provider in [
        "hoverProvider",
        "definitionProvider",
        "completionProvider",
        "documentSymbolProvider",
        "workspaceSymbolProvider",
        "foldingRangeProvider",
        "referencesProvider",
        "documentHighlightProvider",
        "selectionRangeProvider",
        "semanticTokensProvider",
        "renameProvider",
    ] {
        assert!(
            capabilities.get(provider).is_some(),
            "initialize omitted {provider}: {capabilities}"
        );
    }

    assert!(response(&responses, 2)["contents"]["value"]
        .as_str()
        .is_some_and(|text| text.contains("instruction") && text.contains("mov")));
    assert!(response(&responses, 3)["contents"]["value"]
        .as_str()
        .is_some_and(|text| text.contains("register") && text.contains("eax")));
    assert!(response(&responses, 4)["contents"]["value"]
        .as_str()
        .is_some_and(|text| text.contains("constant") && text.contains("defs.inc")));

    assert_eq!(response(&responses, 5)["uri"], json!(include_uri));
    assert_eq!(response(&responses, 5)["range"]["start"]["line"], 0);
    assert_eq!(response(&responses, 6)["uri"], json!(include_uri));
    assert_eq!(response(&responses, 7)["uri"], json!(main_uri));
    assert_eq!(response(&responses, 7)["range"]["start"]["line"], 13);

    let completions = response(&responses, 8)["items"]
        .as_array()
        .expect("completion items");
    assert!(completions.iter().any(|item| item["label"] == "HEIGHT"));
    assert!(completions.iter().any(|item| item["label"] == ".globl"));

    let main_symbols = response(&responses, 9)
        .as_array()
        .expect("document symbols");
    for expected in ["WIDTH", "save", "main", "helper"] {
        assert!(
            main_symbols.iter().any(|symbol| symbol["name"] == expected),
            "missing {expected} from document symbols: {main_symbols:?}"
        );
    }
    assert!(response(&responses, 10)
        .as_array()
        .is_some_and(|symbols| symbols.iter().any(|symbol| symbol["name"] == "HEIGHT")));
    assert!(response(&responses, 11)
        .as_array()
        .is_some_and(|ranges| ranges.len() >= 2));
    assert_eq!(response(&responses, 12).as_array().map(Vec::len), Some(3));
    let highlights = response(&responses, 13)
        .as_array()
        .expect("document highlights");
    assert_eq!(highlights.len(), 3);
    assert_eq!(
        highlights
            .iter()
            .filter(|highlight| highlight["kind"] == 3)
            .count(),
        1
    );
    assert!(response(&responses, 14)[0]["parent"]["range"].is_object());
    let semantic_data = response(&responses, 15)["data"]
        .as_array()
        .expect("semantic token data");
    assert!(!semantic_data.is_empty());
    assert_eq!(semantic_data.len() % 5, 0);
    assert_eq!(
        response(&responses, 16)["changes"][&main_uri]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
    assert!(response(&responses, 17)["contents"]["value"]
        .as_str()
        .is_some_and(|text| text.contains("directive") && text.contains(".globl")));
    assert_eq!(response(&responses, 18).as_array().map(Vec::len), Some(2));
    assert_eq!(
        response(&responses, 19)["changes"][&main_uri]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        response(&responses, 19)["changes"][&include_uri]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(response(&responses, 23).as_array().map(Vec::len), Some(2));

    for (id, expected) in [(20, "uppercase_s"), (21, "intel_entry"), (22, "nasm_entry")] {
        assert!(
            response(&responses, id)
                .as_array()
                .is_some_and(|symbols| symbols.iter().any(|symbol| symbol["name"] == expected)),
            "extension routing failed for response {id}"
        );
    }
    let nasm_symbols = response(&responses, 21).as_array().expect("NASM symbols");
    assert!(nasm_symbols.iter().any(|symbol| symbol["name"] == "COUNT"));
    assert!(nasm_symbols
        .iter()
        .any(|symbol| symbol["name"] == "saveall"));

    let changed_symbols = response(&responses, 30)
        .as_array()
        .expect("changed symbols");
    assert!(changed_symbols
        .iter()
        .any(|symbol| symbol["name"] == "LIVE"));
    assert!(!changed_symbols
        .iter()
        .any(|symbol| symbol["name"] == "WIDTH"));
    assert!(response(&responses, 31)["contents"]["value"]
        .as_str()
        .is_some_and(|text| text.contains("constant") && text.contains("LIVE")));

    let duplicate_diagnostics = messages.iter().filter(|message| {
        message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && message["params"]["uri"] == main_uri
            && message["params"]["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| {
                    diagnostics
                        .iter()
                        .filter(|diagnostic| diagnostic["code"] == "duplicate-symbol")
                        .count()
                        == 2
                })
    });
    assert_eq!(duplicate_diagnostics.count(), 1);
    assert!(response(&responses, 99).is_null());
}
