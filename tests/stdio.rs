//! Drives the built binary over stdio the way an editor does.
//!
//! The unit tests cover the conversions; this covers the wiring they cannot see \u2014 that the process
//! speaks framed JSON-RPC on stdout, answers `initialize` with the capabilities we think we
//! declared, and pushes diagnostics unprompted after `didOpen`. A server can pass every unit test
//! and still be unusable because its stdout is polluted or its handshake is wrong.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hexaly-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr is left inherited so a panic inside the server shows up in test output
            // instead of vanishing into a pipe nobody reads.
            .spawn()
            .expect("the binary was built by cargo test");

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));

        Self { child, stdin, stdout }
    }

    fn send(&mut self, body: &str) {
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).expect("server accepts input");
        self.stdin.flush().expect("flush");
    }

    /// Reads one framed message. Deliberately parses the `Content-Length` header rather than
    /// scanning for JSON: if the server ever writes an unframed byte to stdout, this is what
    /// catches it.
    fn receive(&mut self) -> String {
        let mut length = None;

        loop {
            let mut header = String::new();
            let read = self.stdout.read_line(&mut header).expect("server produces headers");
            assert_ne!(read, 0, "server closed stdout before sending a message");

            if header == "\r\n" {
                break;
            }

            if let Some(value) = header.strip_prefix("Content-Length: ") {
                length = Some(value.trim().parse::<usize>().expect("numeric Content-Length"));
            }
        }

        let mut body = vec![0; length.expect("a Content-Length header preceded the body")];
        self.stdout.read_exact(&mut body).expect("server produces a full body");
        String::from_utf8(body).expect("the body is UTF-8")
    }

    /// Reads until a message containing `needle` arrives, so an interleaved `window/logMessage`
    /// does not fail the test it happens to precede.
    fn receive_containing(&mut self, needle: &str) -> String {
        for _ in 0..10 {
            let message = self.receive();
            if message.contains(needle) {
                return message;
            }
        }
        panic!("no message containing {needle:?} within 10 messages");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn serves_diagnostics_over_stdio() {
    let mut server = Server::start();

    server.send(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}"#,
    );

    let initialize = server.receive_containing("\"id\":1");
    assert!(
        initialize.contains("\"positionEncoding\":\"utf-16\""),
        "got {initialize}"
    );
    // Incremental sync is `2` in the wire enum. Asserting the number rather than the Rust constant
    // checks what the client actually receives.
    assert!(initialize.contains("\"textDocumentSync\":2"), "got {initialize}");
    assert!(initialize.contains("hexaly-lsp"), "got {initialize}");

    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);

    // A file whose second line cannot parse.
    server.send(
        r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/broken.hxm","languageId":"hexaly","version":1,"text":"function model() {\n    a <- bool(;\n}\n"}}}"#,
    );

    let published = server.receive_containing("publishDiagnostics");
    assert!(published.contains("file:///tmp/broken.hxm"), "got {published}");
    assert!(published.contains("hexaly-syntax"), "got {published}");
    assert!(published.contains("\"line\":1"), "the error is on line 1: {published}");

    // Fixing the file must retract the diagnostic, not merely stop adding to it.
    server.send(
        r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///tmp/broken.hxm","version":2},"contentChanges":[{"range":{"start":{"line":1,"character":13},"end":{"line":1,"character":15}},"text":"();"}]}}"#,
    );

    let cleared = server.receive_containing("publishDiagnostics");
    assert!(
        cleared.contains("\"diagnostics\":[]"),
        "expected empty set, got {cleared}"
    );
    assert!(
        cleared.contains("\"version\":2"),
        "version accompanies the retraction: {cleared}"
    );

    server.send(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#);
    let shutdown = server.receive_containing("\"id\":2");
    assert!(shutdown.contains("\"result\":null"), "got {shutdown}");
}

#[test]
fn reports_the_hexaly_installation_it_found() {
    let mut server = Server::start();

    server.send(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null}}"#,
    );
    server.receive_containing("\"id\":1");
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);

    // Either outcome is correct depending on the machine; what matters is that the server says
    // which, because "why do I have no compiler diagnostics" is otherwise unanswerable from the
    // client side.
    let logged = server.receive_containing("Hexaly");
    assert!(
        logged.contains("Hexaly found") || logged.contains("Hexaly not found") || logged.contains("Hexaly configured"),
        "got {logged}"
    );
}

#[test]
fn honours_a_configured_hexaly_path() {
    let mut server = Server::start();

    // A path that does not exist, to prove the setting is read rather than silently discarded in
    // favour of whatever discovery would have found.
    server.send(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":null,"rootUri":null,"initializationOptions":{"hexalyPath":"/configured/by/the/user/hexaly"}}}"#,
    );
    server.receive_containing("\"id\":1");
    server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);

    let logged = server.receive_containing("Hexaly");
    assert!(logged.contains("/configured/by/the/user/hexaly"), "got {logged}");
}
