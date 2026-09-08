use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SHORT: Duration = Duration::from_millis(100);
fn client() -> Arc<QobuzClient> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    Arc::new(QobuzClient::new().unwrap())
}
fn body(id: u64) -> String {
    serde_json::json!({"user_auth_token": "fixture-token", "user": {"id": id, "display_name": "Fixture"}}).to_string()
}

enum Reply {
    HeadersStall,
    BodyStall,
    Drip,
    Response(u16, String),
    Late(String),
    Disconnect,
}
struct Server {
    url: String,
    task: Option<tokio::task::JoinHandle<()>>,
    accepted: Option<tokio::sync::oneshot::Receiver<()>>,
}
impl Server {
    async fn new(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/user/login", listener.local_addr().unwrap());
        let (send, accepted) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let len = socket.read(&mut buffer).await.unwrap();
                assert!(len > 0);
                request.extend_from_slice(&buffer[..len]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let body_len = if headers.starts_with("POST ") {
                        "extra=partner".len()
                    } else {
                        0
                    };
                    if request.len() >= end + 4 + body_len {
                        break;
                    }
                }
            }
            let _ = send.send(());
            match reply {
                Reply::HeadersStall => {
                    let _ = socket.read(&mut buffer).await;
                }
                Reply::BodyStall => {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{\"user\":").await.unwrap();
                    let _ = socket.read(&mut buffer).await;
                }
                Reply::Drip => {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n").await.unwrap();
                    for _ in 0..1000 {
                        if socket.write_all(b" ").await.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(15)).await;
                    }
                }
                Reply::Response(status, body) => {
                    socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
                Reply::Late(body) => {
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                }
                Reply::Disconnect => {}
            }
        });
        Self {
            url,
            task: Some(task),
            accepted: Some(accepted),
        }
    }
    async fn finish(mut self) {
        let task = self.task.take().unwrap();
        // The fixture must be reaped, including when the client's body closes.
        let mut task = task;
        let result = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
        if result.is_err() {
            task.abort();
            let _ = task.await;
            panic!("HTTP fixture did not stop");
        }
        result.unwrap().unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn restore(client: &QobuzClient, server: &Server) -> Result<UserSession> {
    client
        .complete_token_restore(
            client
                .token_restore_request(&server.url, "fixture-app", "fixture-token")
                .unwrap()
                .timeout(SHORT),
        )
        .await
}

async fn assert_timeout(reply: Reply) {
    let client = client();
    let server = Server::new(reply).await;
    let start = std::time::Instant::now();
    let error = restore(&client, &server).await.unwrap_err();
    assert!(
        matches!(error, ApiError::NetworkError(ref e) if e.is_timeout()),
        "{error}"
    );
    assert!(start.elapsed() < Duration::from_millis(750));
    assert!(!client.is_logged_in().await);
    server.finish().await;
}
#[tokio::test]
async fn headers_stall_releases_restore() {
    assert_timeout(Reply::HeadersStall).await;
}
#[tokio::test]
async fn partial_body_stall_releases_restore() {
    assert_timeout(Reply::BodyStall).await;
}
#[tokio::test]
async fn body_trickle_cannot_extend_total_deadline() {
    assert_timeout(Reply::Drip).await;
}

#[tokio::test]
async fn success_auth_rejection_and_transport_keep_their_classification() {
    let client = client();
    let rejected = Server::new(Reply::Response(401, String::new())).await;
    assert!(matches!(
        restore(&client, &rejected).await,
        Err(ApiError::AuthenticationError(_))
    ));
    rejected.finish().await;
    let disconnected = Server::new(Reply::Disconnect).await;
    assert!(matches!(
        restore(&client, &disconnected).await,
        Err(ApiError::NetworkError(_))
    ));
    disconnected.finish().await;
    let valid = Server::new(Reply::Response(200, body(12))).await;
    assert_eq!(restore(&client, &valid).await.unwrap().user_id, 12);
    assert_eq!(client.session.read().await.as_ref().unwrap().user_id, 12);
    valid.finish().await;
}

#[tokio::test]
async fn timed_out_response_cannot_replace_a_retried_account() {
    let client = client();
    let late = Server::new(Reply::Late(body(1))).await;
    assert!(restore(&client, &late).await.is_err());
    let retry = Server::new(Reply::Response(200, body(2))).await;
    assert_eq!(restore(&client, &retry).await.unwrap().user_id, 2);
    retry.finish().await;
    late.finish().await;
    assert_eq!(client.session.read().await.as_ref().unwrap().user_id, 2);
}

#[tokio::test]
async fn canceled_request_cannot_publish_a_late_session() {
    let client = client();
    let mut late = Server::new(Reply::Late(body(1))).await;
    let request = client
        .token_restore_request(&late.url, "app", "token")
        .unwrap();
    let owned = client.clone();
    let task = tokio::spawn(async move { owned.complete_token_restore(request).await });
    late.accepted.take().unwrap().await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let retry = Server::new(Reply::Response(200, body(2))).await;
    assert_eq!(restore(&client, &retry).await.unwrap().user_id, 2);
    retry.finish().await;
    late.finish().await;
    assert_eq!(client.session.read().await.as_ref().unwrap().user_id, 2);
}

#[tokio::test]
async fn streaming_builder_and_body_do_not_inherit_restore_timeout() {
    let client = client();
    let server = Server::new(Reply::Late("stream fixture".into())).await;
    let restore = client
        .token_restore_request(&server.url, "app", "token")
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(restore.timeout(), Some(&TOKEN_RESTORE_TIMEOUT));
    let stream = client.http.get(&server.url).build().unwrap();
    assert_eq!(stream.timeout(), None);
    let start = std::time::Instant::now();
    let bytes = client
        .http
        .execute(stream)
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(start.elapsed() > SHORT);
    assert_eq!(&bytes[..], b"stream fixture");
    server.finish().await;
}
