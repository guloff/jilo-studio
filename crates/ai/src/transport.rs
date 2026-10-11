//! How requests reach a provider: the [`Transport`] trait, the scripted [`FakeTransport`] for tests,
//! and [`HttpTransport`] (feature `net`).

use std::collections::VecDeque;
use std::sync::Mutex;

/// Header names whose values are secrets (shown as `***` by `Debug`).
const SECRET_HEADERS: [&str; 2] = ["authorization", "x-goog-api-key"];

/// An HTTP request.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET` or `POST`.
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self { method: "GET", url: url.into(), headers: Vec::new(), body: Vec::new() }
    }

    pub fn post(url: impl Into<String>, content_type: &str, body: Vec<u8>) -> Self {
        Self { method: "POST", url: url.into(), headers: vec![("Content-Type".into(), content_type.into())], body }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The value of header `name` (case-insensitive).
    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The body as JSON (tests and the fake transport's matchers).
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> =
            self.headers.iter().map(|(k, v)| (k.as_str(), if SECRET_HEADERS.iter().any(|s| k.eq_ignore_ascii_case(s)) { "***" } else { v.as_str() })).collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// An HTTP answer (any status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self { status, body: body.into() }
    }

    pub fn json(status: u16, v: &serde_json::Value) -> Self {
        Self { status, body: v.to_string().into_bytes() }
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Sends a request and returns the answer whatever its status; `Err` only when no answer came
/// (no connection, timeout, TLS failure).
pub trait Transport: Send + Sync {
    fn send(&self, req: &Request) -> Result<Response, String>;

    /// GET `req` into `out` as it arrives, reporting `(bytes so far, total when known)` to
    /// `progress`, which stops the download by returning false. Returns the status: a failed
    /// answer's body is not written. The default reads the whole answer with [`send`](Self::send)
    /// (the fake transport); the HTTP transport streams it.
    fn download(&self, req: &Request, out: &mut dyn std::io::Write, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<u16, String> {
        let r = self.send(req)?;
        if !r.ok() {
            return Ok(r.status);
        }
        let total = r.body.len() as u64;
        let mut done = 0u64;
        for chunk in r.body.chunks(64 << 10) {
            out.write_all(chunk).map_err(|e| e.to_string())?;
            done += chunk.len() as u64;
            if !progress(done, Some(total)) {
                return Err(CANCELLED.into());
            }
        }
        Ok(r.status)
    }
}

/// The error of a download its `progress` stopped.
pub const CANCELLED: &str = "cancelled";

/// A transport for builds without network access: every request fails.
pub struct NoNetwork;

impl Transport for NoNetwork {
    fn send(&self, _: &Request) -> Result<Response, String> {
        Err("this build has no network access".into())
    }
}

type Matcher = Box<dyn Fn(&Request) -> bool + Send + Sync>;

/// Answers from a script and records every request (tests: no network).
///
/// [`FakeTransport::push`] queues answers given in order; [`FakeTransport::route`] answers every
/// request that a matcher accepts (checked first, so concurrent requests can be told apart).
#[derive(Default)]
pub struct FakeTransport {
    queue: Mutex<VecDeque<Result<Response, String>>>,
    routes: Mutex<Vec<(Matcher, Box<dyn Fn(&Request) -> Result<Response, String> + Send + Sync>)>>,
    sent: Mutex<Vec<Request>>,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue the next answer.
    pub fn push(&self, r: Response) -> &Self {
        lock(&self.queue).push_back(Ok(r));
        self
    }

    /// Queue a network failure.
    pub fn push_error(&self, e: &str) -> &Self {
        lock(&self.queue).push_back(Err(e.into()));
        self
    }

    /// Answer every request `when` accepts with `answer`.
    pub fn route(
        &self,
        when: impl Fn(&Request) -> bool + Send + Sync + 'static,
        answer: impl Fn(&Request) -> Result<Response, String> + Send + Sync + 'static,
    ) -> &Self {
        lock(&self.routes).push((Box::new(when), Box::new(answer)));
        self
    }

    /// The requests sent so far.
    pub fn sent(&self) -> Vec<Request> {
        lock(&self.sent).clone()
    }

    /// Answers still queued.
    pub fn pending(&self) -> usize {
        lock(&self.queue).len()
    }
}

impl Transport for FakeTransport {
    fn send(&self, req: &Request) -> Result<Response, String> {
        lock(&self.sent).push(req.clone());
        if let Some((_, answer)) = lock(&self.routes).iter().find(|(when, _)| when(req)) {
            return answer(req);
        }
        lock(&self.queue).pop_front().unwrap_or_else(|| Err(format!("fake transport: no answer scripted for {} {}", req.method, req.url)))
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// HTTPS with pure-Rust TLS (rustls + RustCrypto) and the operating system's certificate verifier.
#[cfg(feature = "net")]
pub struct HttpTransport {
    agent: ureq::Agent,
}

/// Largest answer read (a long transcript is well under this).
#[cfg(feature = "net")]
const MAX_ANSWER: u64 = 64 << 20;

#[cfg(feature = "net")]
impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "net")]
impl HttpTransport {
    pub fn new() -> Self {
        let provider = std::sync::Arc::new(rustls_rustcrypto::provider());
        let agent = ureq::Agent::config_builder()
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::Rustls)
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .unversioned_rustls_crypto_provider(provider)
                    .build(),
            )
            // the provider's answer is read whatever its status: errors carry their reason
            .http_status_as_error(false)
            .timeout_connect(Some(std::time::Duration::from_secs(30)))
            // recognising two minutes of audio can take a while; nothing legitimate takes 10 min
            .timeout_global(Some(std::time::Duration::from_secs(600)))
            .build()
            .into();
        Self { agent }
    }
}

#[cfg(feature = "net")]
impl Transport for HttpTransport {
    fn send(&self, req: &Request) -> Result<Response, String> {
        let resp = if req.method == "GET" {
            let mut r = self.agent.get(&req.url);
            for (k, v) in &req.headers {
                r = r.header(k, v);
            }
            r.call()
        } else {
            let mut r = self.agent.post(&req.url);
            for (k, v) in &req.headers {
                r = r.header(k, v);
            }
            r.send(&req.body[..])
        };
        // ureq's error text names the host and the failure, never a header value
        let resp = resp.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.into_body().with_config().limit(MAX_ANSWER).read_to_vec().map_err(|e| e.to_string())?;
        Ok(Response { status, body })
    }

    fn download(&self, req: &Request, out: &mut dyn std::io::Write, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<u16, String> {
        use std::io::Read;
        let mut r = self.agent.get(&req.url);
        for (k, v) in &req.headers {
            r = r.header(k, v);
        }
        // a large file on a slow line gets half an hour; an answer that never starts, a minute
        let resp = r
            .config()
            .timeout_global(Some(std::time::Duration::from_secs(1800)))
            .timeout_recv_response(Some(std::time::Duration::from_secs(60)))
            .build()
            .call();
        let resp = resp.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Ok(status);
        }
        let mut body = resp.into_body();
        let total = body.content_length();
        let mut reader = body.with_config().limit(MAX_DOWNLOAD).reader();
        let mut buf = vec![0u8; 256 << 10];
        let mut done = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if !progress(done, total) {
                return Err(CANCELLED.into());
            }
        }
        Ok(status)
    }
}

/// Largest file downloaded (an app update is a few tens of megabytes).
#[cfg(feature = "net")]
const MAX_DOWNLOAD: u64 = 2 << 30;
