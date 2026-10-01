//! Bounded transport shared by Azure DevOps PR policy reads and mutations.

use std::future::Future;
use std::time::Duration;

use anyhow::{Context, ensure};
use serde::de::DeserializeOwned;

use super::authenticate_ado_request;
use super::pr_mutations::UpdatePrContext;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn client() -> anyhow::Result<reqwest::Client> {
    client_with_timeout(REQUEST_TIMEOUT)
}

fn client_with_timeout(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("Failed to create bounded PR client")
}

async fn read_body(mut response: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_RESPONSE_BYTES as u64),
        "PR response exceeds the 8 MiB bound"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("PR response stream failed")?
    {
        ensure!(
            chunk.len() <= MAX_RESPONSE_BYTES.saturating_sub(bytes.len()),
            "PR response exceeds the 8 MiB bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(crate) trait BoundedPrResponse {
    fn bounded_json<T: DeserializeOwned + Send>(
        self,
    ) -> impl Future<Output = anyhow::Result<T>> + Send;
    fn bounded_text(self) -> impl Future<Output = anyhow::Result<String>> + Send;
}

impl BoundedPrResponse for reqwest::Response {
    async fn bounded_json<T: DeserializeOwned + Send>(self) -> anyhow::Result<T> {
        if let Some(token) = self.headers().get("x-ms-continuationtoken") {
            ensure!(
                token.to_str()?.trim().is_empty(),
                "Incomplete PR metadata cannot authorize mutation"
            );
        }
        serde_json::from_slice(&read_body(self).await?).context("Malformed PR metadata")
    }

    async fn bounded_text(self) -> anyhow::Result<String> {
        String::from_utf8(read_body(self).await?).context("PR response text is not valid UTF-8")
    }
}

pub(crate) async fn get_json<T: DeserializeOwned + Send>(
    ctx: &UpdatePrContext<'_>,
    url: &str,
) -> anyhow::Result<T> {
    let response = authenticate_ado_request(ctx.client.get(url), ctx.token, ctx.connection_type)
        .send()
        .await
        .context("PR metadata read failed")?;
    ensure!(
        response.status().is_success(),
        "PR metadata read failed (HTTP {})",
        response.status()
    );
    response.bounded_json().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    #[tokio::test]
    async fn exact_response_bound_and_oversized_bodies() {
        for size in [MAX_RESPONSE_BYTES, MAX_RESPONSE_BYTES + 1] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; size]))
                .mount(&server)
                .await;
            let response = client().unwrap().get(server.uri()).send().await.unwrap();
            let result = response.bounded_text().await;
            if size == MAX_RESPONSE_BYTES {
                assert_eq!(result.unwrap().len(), size);
            } else {
                assert!(result.unwrap_err().to_string().contains("8 MiB"));
            }
        }
    }

    #[tokio::test]
    async fn continuation_and_malformed_json_fail_closed() {
        for response in [
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"value":[]}))
                .insert_header("x-ms-continuationtoken", "more"),
            ResponseTemplate::new(200).set_body_string("not json"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(response)
                .mount(&server)
                .await;
            let response = client().unwrap().get(server.uri()).send().await.unwrap();
            assert!(response.bounded_json::<serde_json::Value>().await.is_err());
        }
    }

    #[tokio::test]
    async fn streamed_responses_are_bounded_without_content_length() {
        for chunked in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let sender = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await?;
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await?;
                    assert!(count > 0 && request.len() < 8192);
                    request.extend_from_slice(&buffer[..count]);
                }
                let headers = if chunked {
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n"
                };
                stream.write_all(headers.as_bytes()).await?;
                let content = vec![b'x'; MAX_RESPONSE_BYTES + 1];
                if chunked {
                    for chunk in content.chunks(65536) {
                        stream
                            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                            .await?;
                        stream.write_all(chunk).await?;
                        stream.write_all(b"\r\n").await?;
                    }
                    stream.write_all(b"0\r\n\r\n").await?;
                } else {
                    stream.write_all(&content).await?;
                }
                stream.shutdown().await
            });
            let response = client()
                .unwrap()
                .get(format!("http://{address}"))
                .send()
                .await
                .unwrap();
            assert!(response.content_length().is_none());
            assert!(
                response
                    .bounded_text()
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("8 MiB")
            );
            if let Err(error) = sender.await.unwrap() {
                assert!(matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                ));
            }
        }
    }

    #[tokio::test]
    async fn mutation_timeout_is_reported_as_uncertain_without_replay() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let client = client_with_timeout(Duration::from_millis(100)).unwrap();
        client.get(server.uri()).send().await.unwrap();
        let ctx = crate::safe_outputs::ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_organization: Some("org".into()),
            ado_project: Some("P".into()),
            repository_name: Some("repo".into()),
            ..Default::default()
        };
        let operation = UpdatePrContext {
            client: &client,
            target: crate::safe_outputs::resolve_repository_write_target(None, &ctx).unwrap(),
            pr_id: 42,
            token: "test-token",
            connection_type: None,
        };
        let result = super::super::pr_mutations::execute_add_labels(&operation, &["test".into()])
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.contains("delivery uncertain"));
        assert_eq!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|request| request.method.as_str() == "POST")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn request_deadline_applies_without_retries() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let error = client_with_timeout(Duration::from_millis(50))
            .unwrap()
            .get(server.uri())
            .send()
            .await
            .unwrap_err();
        assert!(error.is_timeout());
        assert!(server.received_requests().await.unwrap().len() <= 1);
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }
}
