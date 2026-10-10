//! Client for the desktop app's JSON-lines control protocol. One connection, reconnect on failure.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::AutomationError;

/// How often a blocking export's app job is polled.
const JOB_POLL: Duration = Duration::from_millis(250);

type Conn = (BufReader<tokio::net::tcp::OwnedReadHalf>, tokio::net::tcp::OwnedWriteHalf);

pub struct BridgeClient {
    addr: String,
    /// Where the app's control token file is looked up (`None`: the default data directory).
    data_dir: Option<std::path::PathBuf>,
    conn: Mutex<Option<Conn>>,
    next_id: AtomicU64,
}

impl BridgeClient {
    /// `addr` such as `127.0.0.1:9876` (loopback only).
    pub fn new(addr: impl Into<String>) -> Result<Self, AutomationError> {
        Self::with_data_dir(addr, None)
    }

    /// Like [`Self::new`], with the app's data directory (`--data-dir`) for its control token.
    pub fn with_data_dir(addr: impl Into<String>, data_dir: Option<std::path::PathBuf>) -> Result<Self, AutomationError> {
        let addr = addr.into();
        let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(&addr);
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
            return Err(AutomationError::BadRequest(format!("bridge address must be loopback, got `{addr}`")));
        }
        Ok(Self { addr, data_dir, conn: Mutex::new(None), next_id: AtomicU64::new(1) })
    }

    /// Run engine command `id` in the app. A long call ([`crate::long_job::kind`]: a command that
    /// takes `wait` called with `wait: true`, a cloud AI task or a face analysis) is started as an
    /// app job and polled here until it finishes, so the app keeps repainting (its status bar shows
    /// the progress) and answering other requests, and no request waits on the control server's
    /// 60 s reply limit (#91, #92). The reply is a blocking call's: `{job, …, result}`, the applied
    /// result of an AI task, or the job's error.
    pub async fn execute(&self, id: &str, params: Value) -> Result<Value, AutomationError> {
        let Some(kind) = crate::long_job::kind(id, &params) else {
            return self.call("engine.execute", json!({"command": id, "params": params})).await;
        };
        let mut start = self.call("engine.execute", json!({"command": id, "params": crate::long_job::start_params(kind, &params)})).await?;
        let Some(job) = start.get("job").and_then(Value::as_u64) else { return Ok(start) };
        let result = loop {
            tokio::time::sleep(JOB_POLL).await;
            let jobs = self.call("engine.execute", json!({"command": "jobs.list", "params": {}})).await?;
            let Some(j) = jobs.as_array().and_then(|a| a.iter().find(|j| j["id"].as_u64() == Some(job))).cloned() else {
                break Value::Null;
            };
            if j["finished"].as_bool() == Some(true) {
                break j["result"].clone();
            }
        };
        if let Some(e) = result.get("error").and_then(Value::as_str) {
            return Err(AutomationError::App(format!("{id} failed: {e}")));
        }
        if kind == crate::long_job::Kind::Background {
            // the answer is cached: the same call applies it
            return self.call("engine.execute", json!({"command": id, "params": params})).await;
        }
        if let Some(o) = start.as_object_mut() {
            o.insert("result".into(), result);
        }
        Ok(start)
    }

    /// Present the app's control token on a new connection (#163). Read on every connect, so a
    /// restarted app's new token is picked up. Without one (an app started with
    /// `FILMCRAFT_CONTROL_NO_AUTH=1`, or an older app) nothing is sent.
    async fn authenticate(&self, conn: &mut Conn) -> Result<(), AutomationError> {
        let port = self.addr.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).unwrap_or(0);
        let Some(token) = filmcraft_engine::control_token::for_client(port, self.data_dir.as_deref()) else { return Ok(()) };
        let line = format!("{}\n", json!({"id": 0, "method": "auth", "params": {"token": token}}));
        conn.1.write_all(line.as_bytes()).await.map_err(|e| AutomationError::Bridge(e.to_string()))?;
        let mut buf = String::new();
        tokio::time::timeout(Duration::from_secs(10), conn.0.read_line(&mut buf))
            .await
            .map_err(|_| AutomationError::Bridge("timeout authenticating".into()))?
            .map_err(|e| AutomationError::Bridge(e.to_string()))?;
        let v: Value = serde_json::from_str(&buf).map_err(|e| AutomationError::Bridge(format!("bad reply to auth: {e}")))?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        let why = v.get("error").and_then(Value::as_str).unwrap_or("refused");
        Err(AutomationError::Bridge(format!("{} refused the control token ({why}); is it the app started with --control {port}?", self.addr)))
    }

    /// Call a control method; returns `result` or the app's error.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, AutomationError> {
        let mut guard = self.conn.lock().await;
        for attempt in 0..2 {
            if guard.is_none() {
                let s = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&self.addr))
                    .await
                    .map_err(|_| AutomationError::Bridge(format!("timed out connecting to {}", self.addr)))?
                    .map_err(|e| AutomationError::Bridge(format!("cannot connect to {} ({e}); start the app with `filmcraft --control <port>`", self.addr)))?;
                let (r, w) = s.into_split();
                let mut conn = (BufReader::new(r), w);
                self.authenticate(&mut conn).await?;
                *guard = Some(conn);
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let Some(conn) = guard.as_mut() else {
                return Err(AutomationError::Bridge(format!("not connected to {}", self.addr)));
            };
            let line = format!("{}\n", json!({"id": id, "method": method, "params": params}));
            let res: Result<Value, AutomationError> = async {
                conn.1.write_all(line.as_bytes()).await.map_err(|e| AutomationError::Bridge(e.to_string()))?;
                let mut buf = String::new();
                tokio::time::timeout(Duration::from_secs(90), conn.0.read_line(&mut buf))
                    .await
                    .map_err(|_| AutomationError::Bridge("timeout".into()))?
                    .map_err(|e| AutomationError::Bridge(e.to_string()))?;
                serde_json::from_str(&buf).map_err(|e| AutomationError::Bridge(format!("bad reply: {e}")))
            }
            .await;
            match res {
                Ok(v) => {
                    return if v.get("ok").and_then(Value::as_bool) == Some(true) {
                        Ok(v.get("result").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(AutomationError::App(v.get("error").and_then(Value::as_str).unwrap_or("error").to_string()))
                    };
                }
                Err(e) if attempt == 0 => {
                    *guard = None;
                    let _ = e;
                }
                Err(e) => return Err(e),
            }
        }
        Err(AutomationError::Bridge("unreachable".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// A stand-in for the app's control server: the export starts job 7, which `jobs.list` reports
    /// running twice and then finished with `result`. Every request is recorded.
    async fn fake_app(result: Value) -> (String, Arc<Mutex<Vec<Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            let (r, mut w) = s.into_split();
            let mut lines = BufReader::new(r).lines();
            let mut polls = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                let req: Value = serde_json::from_str(&line).unwrap();
                log.lock().await.push(req.clone());
                let reply = match req["params"]["command"].as_str() {
                    Some("file.exportMedia") => json!({"job": 7, "path": "/tmp/out.mov"}),
                    Some("jobs.list") => {
                        polls += 1;
                        let finished = polls >= 3;
                        json!([{"id": 7, "finished": finished, "result": if finished { result.clone() } else { Value::Null }}])
                    }
                    _ => json!(null),
                };
                let line = format!("{}\n", json!({"id": req["id"], "ok": true, "result": reply}));
                w.write_all(line.as_bytes()).await.unwrap();
            }
        });
        (addr, seen)
    }

    /// A blocking bridge export runs as an app job (#91, #92): the app never sees `wait: true` (it
    /// would encode on its UI thread and the reply would time out after 60 s), and the caller still
    /// gets the finished result.
    #[tokio::test]
    async fn a_blocking_export_is_polled_as_a_job() {
        let (addr, seen) = fake_app(json!({"frames": 642})).await;
        let b = BridgeClient::new(addr).unwrap();
        let r = b.execute("file.exportMedia", json!({"path": "/tmp/out.mov", "wait": true})).await.unwrap();
        assert_eq!(r, json!({"job": 7, "path": "/tmp/out.mov", "result": {"frames": 642}}));
        let seen = seen.lock().await;
        assert_eq!(seen[0]["params"]["params"]["wait"], json!(false));
        assert!(seen.iter().all(|q| q["params"]["params"]["wait"] != json!(true)));
        assert_eq!(seen.iter().filter(|q| q["params"]["command"] == "file.exportMedia").count(), 1, "started once");
        assert_eq!(seen.iter().filter(|q| q["params"]["command"] == "jobs.list").count(), 3);
    }

    /// #163: the client reads the app's token file for the port and authenticates first, again
    /// after a reconnect; the app answers nothing before that.
    #[tokio::test]
    async fn the_client_presents_the_apps_token_on_every_connection() {
        if filmcraft_engine::control_token::token_from_env().is_some() {
            return; // the environment's token would take precedence over the file
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = std::env::temp_dir().join(format!("fc-bridge-token-{}", std::process::id()));
        let path = filmcraft_engine::control_token::token_path(&dir, addr.port());
        filmcraft_engine::control_token::write(&path, "t0ken").unwrap();
        let auths = Arc::new(Mutex::new(0usize));
        let count = auths.clone();
        tokio::spawn(async move {
            // two connections: the second after the first is dropped by the app
            for conn in 0..2 {
                let (s, _) = listener.accept().await.unwrap();
                let (r, mut w) = s.into_split();
                let mut lines = BufReader::new(r).lines();
                let first: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(first["method"], "auth");
                assert_eq!(first["params"]["token"], "t0ken");
                *count.lock().await += 1;
                w.write_all(format!("{}\n", json!({"id": first["id"], "ok": true, "result": null})).as_bytes()).await.unwrap();
                let req: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                if conn == 0 {
                    continue; // drop the connection without answering: the client reconnects
                }
                w.write_all(format!("{}\n", json!({"id": req["id"], "ok": true, "result": "pong"})).as_bytes()).await.unwrap();
            }
        });
        let b = BridgeClient::with_data_dir(addr.to_string(), Some(dir.clone())).unwrap();
        assert_eq!(b.call("ping", json!({})).await.unwrap(), json!("pong"));
        assert_eq!(*auths.lock().await, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_failed_blocking_export_is_an_error() {
        let (addr, _) = fake_app(json!({"error": "encode: disk full"})).await;
        let b = BridgeClient::new(addr).unwrap();
        let e = b.execute("file.exportMedia", json!({"path": "/tmp/out.mov", "wait": true})).await.unwrap_err();
        assert!(e.to_string().contains("file.exportMedia failed: encode: disk full"), "{e}");
    }

    /// Everything else is forwarded unchanged.
    #[tokio::test]
    async fn other_commands_are_forwarded() {
        let (addr, seen) = fake_app(Value::Null).await;
        let b = BridgeClient::new(addr).unwrap();
        b.execute("file.exportMedia", json!({"path": "/tmp/out.mov"})).await.unwrap();
        b.execute("sequence.inspect", json!({})).await.unwrap();
        let seen = seen.lock().await;
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1]["params"]["command"], "sequence.inspect");
    }
}
