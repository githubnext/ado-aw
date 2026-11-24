//! Stdio transport shared by ado-aw's rmcp servers.
//!
//! MCP clients on protocol revision 2026-07-28 or later (for example the Go
//! SDK v1.8 inside MCPG v0.5) open a session with a stateless
//! `server/discover` request (SEP-2575). When discovery fails they fall back to
//! the legacy `initialize` handshake. rmcp 1.x treats any first message other
//! than `initialize` as fatal and exits, so the client's fallback reaches a
//! closed pipe. This transport answers those leading discovery requests with
//! "method not found" and then hands the untouched stream to rmcp.

use log::info;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

const DISCOVER_METHOD: &str = "server/discover";
const METHOD_NOT_FOUND: i64 = -32601;
/// Upper bound on the discovery requests answered before the stream is handed
/// to rmcp. The Go SDK sends at most two.
const MAX_DISCOVERY_REPLIES: usize = 8;

/// Stdin/stdout pair for `ServiceExt::serve` that tolerates SEP-2575 discovery.
pub async fn stdio() -> std::io::Result<(impl AsyncRead + Send + Unpin + 'static, tokio::io::Stdout)>
{
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut writer = tokio::io::stdout();
    let first = answer_discovery(&mut reader, &mut writer).await?;
    Ok((std::io::Cursor::new(first).chain(reader), writer))
}

/// Answers leading `server/discover` requests and returns the first other line
/// (empty at end of input) so it can be replayed to rmcp.
async fn answer_discovery<R, W>(reader: &mut R, writer: &mut W) -> std::io::Result<Vec<u8>>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = Vec::new();
    for _ in 0..MAX_DISCOVERY_REPLIES {
        if reader.read_until(b'\n', &mut line).await? == 0 {
            break;
        }
        let Some(id) = discovery_request_id(&line) else {
            break;
        };
        let mut reply = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": METHOD_NOT_FOUND,
                "message": format!("Method not found: {DISCOVER_METHOD}"),
            },
        }))?;
        reply.push(b'\n');
        writer.write_all(&reply).await?;
        writer.flush().await?;
        info!("Declined MCP {DISCOVER_METHOD}; awaiting the initialize handshake");
        line.clear();
    }
    Ok(line)
}

fn discovery_request_id(line: &[u8]) -> Option<Value> {
    let message: Value = serde_json::from_slice(line).ok()?;
    if message.get("method")?.as_str()? != DISCOVER_METHOD {
        return None;
    }
    message
        .get("id")
        .filter(|id| id.is_string() || id.is_number())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::{ServerHandler, ServiceExt};

    const DISCOVER: &str = r#"{"jsonrpc":"2.0","id":0,"method":"server/discover","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"go-sdk","version":"1.8.0"}}}"#;
    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"go-sdk","version":"1.8.0"}}}"#;
    const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

    #[derive(Clone)]
    struct EmptyServer;
    impl ServerHandler for EmptyServer {}

    #[tokio::test]
    async fn declines_discovery_and_replays_the_next_message() {
        let input = format!("{DISCOVER}\n{INITIALIZE}\n");
        let mut reader = BufReader::new(input.as_bytes());
        let mut written = Vec::new();

        let first = answer_discovery(&mut reader, &mut written).await.unwrap();

        assert_eq!(first, format!("{INITIALIZE}\n").into_bytes());
        let reply: Value = serde_json::from_slice(&written).unwrap();
        assert_eq!(reply["id"], 0);
        assert_eq!(reply["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn passes_a_legacy_handshake_through_untouched() {
        let input = format!("{INITIALIZE}\n");
        let mut reader = BufReader::new(input.as_bytes());
        let mut written = Vec::new();

        let first = answer_discovery(&mut reader, &mut written).await.unwrap();

        assert_eq!(first, input.into_bytes());
        assert!(written.is_empty());
    }

    #[tokio::test]
    async fn rmcp_completes_the_handshake_after_declined_discovery() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (server_read, mut server_write) = tokio::io::split(server);
        let (client_read, mut client_write) = tokio::io::split(client);
        client_write
            .write_all(format!("{DISCOVER}\n{INITIALIZE}\n{INITIALIZED}\n").as_bytes())
            .await
            .unwrap();

        let mut reader = BufReader::new(server_read);
        let first = answer_discovery(&mut reader, &mut server_write)
            .await
            .unwrap();
        let service = EmptyServer
            .serve((std::io::Cursor::new(first).chain(reader), server_write))
            .await
            .expect("rmcp should accept the replayed initialize");

        let mut lines = BufReader::new(client_read).lines();
        let declined: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(declined["error"]["code"], METHOD_NOT_FOUND);
        let initialized: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(initialized["id"], 1);
        assert!(initialized["result"]["protocolVersion"].is_string());

        service.cancel().await.unwrap();
    }
}
