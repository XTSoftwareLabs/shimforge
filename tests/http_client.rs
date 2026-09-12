//! A real HTTP client stack driven against an in-process peer.
//!
//! Only the socket connect is mocked. Request encoding, response parsing and
//! connection handling all run their normal code.

use http_body_util::BodyExt;
use hyper::{Request, Response, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use shimforge::{Session, mock};
use std::future::Future;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream as BlockingStream};
use std::pin::Pin;
use std::thread;
use tokio::net::{TcpSocket, TcpStream};

const BODY: &str = r#"{"service":"inventory","healthy":true}"#;

/// A stream connected to a local peer that answers one HTTP request.
fn local_http_peer() -> io::Result<TcpStream> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    thread::spawn(move || {
        let Ok((mut peer, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(&mut peer);
        let mut line = String::new();
        // Read the request line and headers up to the blank separator.
        while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
            if line.trim().is_empty() {
                break;
            }
            line.clear();
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\n\
             content-type: application/json\r\n\
             content-length: {}\r\n\
             connection: close\r\n\
             \r\n\
             {BODY}",
            BODY.len()
        );
        let _ = peer.write_all(response.as_bytes());
        let _ = peer.flush();
        let _ = peer.shutdown(Shutdown::Write);
    });
    let client = BlockingStream::connect(address)?;
    client.set_nonblocking(true)?;
    TcpStream::from_std(client)
}

fn witness() -> impl Future<Output = io::Result<TcpStream>> {
    TcpSocket::new_v4()
        .unwrap()
        .connect("127.0.0.1:80".parse().unwrap())
}

#[tokio::test]
async fn a_request_is_answered_without_opening_an_outside_connection() {
    let mut session = Session::new_global().unwrap();
    let connects = session.mock_async(witness()).unwrap();
    connects
        .expect()
        .once()
        .return_once(local_http_peer())
        .unwrap();

    let client = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    // 198.51.100.0/24 is reserved for documentation and is never routed.
    let request = Request::builder()
        .method("GET")
        .uri("http://198.51.100.4/health")
        .header("user-agent", "shimforge-tests/1.0")
        .body(String::new())
        .unwrap();

    let response = client.request(request).await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(String::from_utf8(body.to_vec()).unwrap(), BODY);
    connects.verify().unwrap();
}

#[tokio::test]
async fn a_connect_failure_surfaces_as_a_client_error() {
    let mut session = Session::new_global().unwrap();
    let connects = session.mock_async(witness()).unwrap();
    connects
        .expect()
        .returning(|| Err(io::ErrorKind::HostUnreachable.into()))
        .unwrap();

    let client: Client<_, String> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let request = Request::builder()
        .method("GET")
        .uri("http://198.51.100.4/health")
        .body(String::new())
        .unwrap();

    let error = client.request(request).await.unwrap_err();
    assert!(error.is_connect(), "{error}");
    session.restore().unwrap();
}

/// An SDK-style client that hands out a boxed future for each request.
type Call<'a> = Pin<Box<dyn Future<Output = io::Result<Response<String>>> + Send + 'a>>;

struct Pipeline {
    endpoint: String,
}

impl Pipeline {
    fn send<'a>(&'a self, path: &'a str) -> Call<'a> {
        Box::pin(async move {
            let request = Request::get(format!("{}{path}", self.endpoint))
                .body(String::new())
                .unwrap();
            let client = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
            let body = client
                .request(request)
                .await
                .map_err(io::Error::other)?
                .into_body()
                .collect()
                .await
                .map_err(io::Error::other)?
                .to_bytes();
            Ok(Response::new(String::from_utf8_lossy(&body).into_owned()))
        })
    }
}

#[tokio::test]
async fn an_sdk_request_method_answers_without_a_transport() {
    let mut session = Session::new_global().unwrap();
    let sends = mock!(
        session,
        Pipeline::send,
        for<'a> fn(&'a Pipeline, &'a str) -> Call<'a>
    )
    .unwrap();
    sends
        .expect()
        .with(|pipeline, path| {
            pipeline.endpoint == "https://inventory.invalid" && **path == *"/v1/items"
        })
        .once()
        .returning(|_, _| {
            Box::pin(async {
                let mut response = Response::new(String::from(BODY));
                *response.status_mut() = StatusCode::OK;
                Ok(response)
            }) as Call<'_>
        })
        .unwrap();

    let pipeline = Pipeline {
        endpoint: String::from("https://inventory.invalid"),
    };
    let response = pipeline.send("/v1/items").await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.body(), BODY);
    sends.verify().unwrap();
}
