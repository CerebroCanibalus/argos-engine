//! End-to-end protocol test: spawns the real binary and speaks NDJSON over
//! stdio, exactly as an MCP host does.
//!
//! The unit tests in `main.rs` exercise the tools through `FlojoTester`, which
//! never touches the wire. This file covers what only the wire can show:
//! the `initialize` handshake, the `tools/list` payload including annotations,
//! and the argument validation path. Every tool call here is validated before
//! any provider is contacted, so the test needs no network.

use std::process::Stdio;
use std::time::Duration;

use flojo_mcp::serde_json::{self, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

const BOOT_TIMEOUT: Duration = Duration::from_secs(20);

/// Thin wrapper: writes one JSON-RPC message and reads until `id` answers.
struct Host {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl Host {
    async fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_argos-engine"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the binary must be buildable for integration tests");
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    async fn send(&mut self, message: &str) {
        self.stdin
            .write_all(message.as_bytes())
            .await
            .expect("stdin must stay open for the session");
        self.stdin
            .write_all(b"\n")
            .await
            .expect("messages are newline-delimited");
    }

    /// Read until the response with `id`, ignoring notifications.
    async fn receive(&mut self, id: u64) -> Value {
        let mut line = String::new();
        loop {
            line.clear();
            let read = tokio::time::timeout(BOOT_TIMEOUT, self.stdout.read_line(&mut line))
                .await
                .expect("the server must answer within the boot timeout")
                .expect("stdout must not close");
            assert!(
                read > 0,
                "the server closed stdout before answering id {id}"
            );
            let parsed: Value = serde_json::from_str(line.trim()).expect("NDJSON line");
            if parsed["id"].as_u64() == Some(id) {
                return parsed;
            }
            // Notifications carry no `id`; keep reading.
        }
    }

    async fn initialize(&mut self, id: u64) -> Value {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":"2025-03-26","capabilities":{{}},"clientInfo":{{"name":"e2e","version":"1"}}}}}}"#
        ))
        .await;
        self.receive(id).await
    }

    async fn shutdown(mut self) {
        // Closing stdin is the stdio transport's shutdown signal.
        let _ = self.stdin.shutdown().await;
        let _ = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
    }
}

/// Pull the text block out of a `tools/call` response, if it succeeded.
fn text_payload(response: &Value) -> Option<&str> {
    let content = response["result"]["content"].as_array()?;
    content
        .iter()
        .find(|block| block["type"] == "text")
        .and_then(|block| block["text"].as_str())
}

#[tokio::test]
async fn handshake_advertises_the_crate_version() {
    let mut host = Host::spawn().await;
    let response = host.initialize(1).await;
    let advertised = response["result"]["serverInfo"]["version"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        advertised,
        env!("CARGO_PKG_VERSION"),
        "the initialize handshake must not drift from Cargo.toml"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn listed_tools_carry_all_four_annotations() {
    let mut host = Host::spawn().await;
    host.initialize(1).await;
    host.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#)
        .await;
    host.send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
        .await;
    let response = host.receive(2).await;

    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools/list must return an array");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or_default())
        .collect();
    assert!(
        names.contains(&"search"),
        "search must be listed: {names:?}"
    );
    assert!(
        names.contains(&"status"),
        "status must be listed: {names:?}"
    );

    // This is what a directory validates: every hint present and boolean.
    for wanted in ["search", "status"] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == wanted)
            .expect("tool listed");
        let annotations = &tool["annotations"];
        assert!(annotations.is_object(), "{wanted} must declare annotations");
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(
                annotations[hint].is_boolean(),
                "{wanted}.{hint} must be an explicit boolean, got {annotations:?}"
            );
        }
        assert_eq!(annotations["readOnlyHint"], true, "{wanted} only reads");
        assert_eq!(
            annotations["destructiveHint"], false,
            "{wanted} must not claim destructiveness"
        );
    }
    host.shutdown().await;
}

#[tokio::test]
async fn status_reports_shape_over_the_wire() {
    let mut host = Host::spawn().await;
    host.initialize(1).await;
    host.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#)
        .await;
    host.send(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"status","arguments":{}}}"#,
    )
    .await;
    let response = host.receive(2).await;

    let payload = text_payload(&response)
        .expect("status must answer with a text block")
        .to_string();
    let status: Value = serde_json::from_str(&payload).expect("status payload must be JSON");
    assert_eq!(status["name"], "argos-engine");
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["profile"], "general");
    assert!(
        status["providers"]
            .as_array()
            .is_some_and(|p| !p.is_empty()),
        "the general profile must probe at least one provider"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn search_rejects_bad_arguments_before_contacting_any_provider() {
    let mut host = Host::spawn().await;
    host.initialize(1).await;
    host.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#)
        .await;

    let cases: Vec<(&str, u64, &str)> = vec![
        (
            "empty query",
            2,
            r#"{"name":"search","arguments":{"query":"  "}}"#,
        ),
        (
            "unknown profile",
            3,
            r#"{"name":"search","arguments":{"query":"rust","profile":"inventado"}}"#,
        ),
        (
            "malformed domain",
            4,
            r#"{"name":"search","arguments":{"query":"rust","domains":["example.com) OR (site:other.test"]}}"#,
        ),
    ];

    for (label, id, arguments) in cases {
        host.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{arguments}}}"#
        ))
        .await;
        let response = host.receive(id).await;
        assert!(
            response["error"].is_object(),
            "{label} must be rejected, got {response:?}"
        );
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "{label} must carry a typed message, got {response:?}"
        );
    }
    host.shutdown().await;
}
