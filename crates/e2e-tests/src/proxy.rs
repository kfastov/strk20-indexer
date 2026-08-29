//! Recording reverse proxy (spec §10.3 topology): sits between the client
//! and the indexer, byte-captures every request (method, URI, headers, body)
//! for the mechanical no-key assertion, and forwards transparently.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Captured {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Captured {
    /// Every byte of the request, concatenated — the scanner input.
    pub fn all_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(self.method.as_bytes());
        out.push(b' ');
        out.extend_from_slice(self.uri.as_bytes());
        out.push(b'\n');
        for (k, v) in &self.headers {
            out.extend_from_slice(k.as_bytes());
            out.push(b':');
            out.extend_from_slice(v.as_bytes());
            out.push(b'\n');
        }
        out.extend_from_slice(&self.body);
        out
    }
}

#[derive(Clone)]
pub struct RecordingProxy {
    pub target: String,
    pub captured: Arc<Mutex<Vec<Captured>>>,
    http: reqwest::Client,
}

impl RecordingProxy {
    pub fn new(target: &str) -> Self {
        Self {
            target: target.trim_end_matches('/').to_owned(),
            captured: Arc::new(Mutex::new(Vec::new())),
            http: reqwest::Client::new(),
        }
    }

    pub fn take_captured(&self) -> Vec<Captured> {
        std::mem::take(&mut self.captured.lock().unwrap())
    }

    pub async fn serve(&self) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().fallback(forward).with_state(self.clone());
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        addr
    }
}

async fn forward(
    State(proxy): State<RecordingProxy>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_owned();
    proxy.captured.lock().unwrap().push(Captured {
        method: method.to_string(),
        uri: path_and_query.clone(),
        headers: headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    v.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect(),
        body: body.to_vec(),
    });

    let url = format!("{}{}", proxy.target, path_and_query);
    let mut req = proxy
        .http
        .request(method.clone(), &url)
        .body(body.to_vec());
    for (k, v) in headers.iter() {
        if k == axum::http::header::HOST {
            continue;
        }
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut out_headers = HeaderMap::new();
            for (k, v) in resp.headers() {
                if let (Ok(name), Ok(value)) = (
                    axum::http::HeaderName::from_bytes(k.as_str().as_bytes()),
                    axum::http::HeaderValue::from_bytes(v.as_bytes()),
                ) {
                    out_headers.insert(name, value);
                }
            }
            out_headers.remove(axum::http::header::TRANSFER_ENCODING);
            out_headers.remove(axum::http::header::CONTENT_LENGTH);
            let bytes = resp.bytes().await.unwrap_or_default();
            (status, out_headers, bytes).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("proxy error: {e}")).into_response(),
    }
}
