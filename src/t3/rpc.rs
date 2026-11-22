//! T3 Code's WebSocket RPC, as far as aitop needs it.
//!
//! A T3 server answers on `GET /ws?orchestrationProtocol=2` with the session
//! token in an `Authorization: Bearer` header. Every message is one JSON
//! text frame. A call is
//! `{"_tag":"Request","id":"1","tag":"server.getUsageSummary","payload":{…},"headers":[]}`
//! and its answer is
//! `{"_tag":"Exit","requestId":"1","exit":{"_tag":"Success","value":{…}}}`,
//! or a `Failure` whose `cause` lists the errors.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::AUTHORIZATION;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// The protocol generation this client speaks; servers refuse others.
const ORCHESTRATION_PROTOCOL: u32 = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The server did not accept the token.
    Unauthorized,
    /// The call was answered with a failure: its error tag and message.
    Failed { tag: String, message: String },
    /// The server could not be reached, or stopped answering.
    Transport(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unauthorized => write!(f, "the token was not accepted"),
            Error::Failed { message, .. } => write!(f, "{message}"),
            Error::Transport(e) => write!(f, "{e}"),
        }
    }
}

fn transport(e: impl std::fmt::Display) -> Error {
    Error::Transport(e.to_string())
}

pub struct Rpc {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl Rpc {
    /// Open a session on the server at `base` (`http://…` or `https://…`).
    pub fn connect(base: &str, token: &str) -> Result<Rpc, Error> {
        let url = ws_url(base).ok_or_else(|| transport(format!("not an http address: {base}")))?;
        let mut request = url.as_str().into_client_request().map_err(transport)?;
        let bearer = format!("Bearer {token}").parse().map_err(|_| transport("the token is not usable in a header"))?;
        request.headers_mut().insert(AUTHORIZATION, bearer);
        let uri = request.uri();
        let host = uri.host().ok_or_else(|| transport(format!("no host in {base}")))?.to_string();
        let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });

        let addrs = (host.as_str(), port).to_socket_addrs().map_err(|e| transport(format!("{host}: {e}")))?;
        let mut last = transport(format!("{host} has no address"));
        let mut stream = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = transport(format!("{host}:{port}: {e}")),
            }
        }
        let stream = stream.ok_or(last)?;
        // Bounds the handshake and each read; calls keep their own deadline.
        let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
        let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
        let _ = stream.set_nodelay(true);
        match tungstenite::client_tls(request, stream) {
            Ok((socket, _)) => Ok(Rpc { socket, next_id: 1 }),
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))) => {
                Err(match response.status().as_u16() {
                    401 | 403 => Error::Unauthorized,
                    426 => transport("this T3 Code speaks a different protocol version than aitop does"),
                    code => transport(format!("{host} answered HTTP {code}")),
                })
            }
            Err(e) => Err(transport(format!("{host}:{port}: {e}"))),
        }
    }

    /// Make one call and wait up to `deadline` for its answer.
    pub fn call(&mut self, tag: &str, payload: Value, deadline: Duration) -> Result<Value, Error> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let request = json!({ "_tag": "Request", "id": id, "tag": tag, "payload": payload, "headers": [] });
        self.socket.send(Message::text(request.to_string())).map_err(transport)?;
        let until = Instant::now() + deadline;
        loop {
            if Instant::now() >= until {
                return Err(transport(format!("no answer to {tag}")));
            }
            let text = match self.socket.read() {
                Ok(Message::Text(t)) => t,
                Ok(Message::Close(_)) => return Err(transport("the server closed the connection")),
                Ok(_) => continue,
                // A read that timed out: keep waiting until the deadline.
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    continue
                }
                Err(e) => return Err(transport(e)),
            };
            let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else { continue };
            // A frame is one message or a batch of them.
            let messages = match frame {
                Value::Array(items) => items,
                one => vec![one],
            };
            for mut m in messages {
                if m["requestId"].as_str() != Some(&id) {
                    continue;
                }
                match m["_tag"].as_str() {
                    Some("Exit") => return exit(m["exit"].take()),
                    // A streamed answer: acknowledge each part so the server sends the next.
                    Some("Chunk") => {
                        let ack = json!({ "_tag": "Ack", "requestId": id });
                        self.socket.send(Message::text(ack.to_string())).map_err(transport)?;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// `http://host:3773` → `ws://host:3773/ws?orchestrationProtocol=2`.
fn ws_url(base: &str) -> Option<String> {
    let base = base.trim().trim_end_matches('/');
    let rest = base.strip_prefix("http").filter(|r| r.starts_with("://") || r.starts_with("s://"))?;
    Some(format!("ws{rest}/ws?orchestrationProtocol={ORCHESTRATION_PROTOCOL}"))
}

/// The value of a successful `exit`, or its first error.
fn exit(mut exit: Value) -> Result<Value, Error> {
    if exit["_tag"].as_str() == Some("Success") {
        return Ok(exit["value"].take());
    }
    let cause = &exit["cause"][0];
    let error = if cause["error"].is_object() { &cause["error"] } else { cause };
    let text = |v: &Value| v.as_str().map(String::from);
    Err(Error::Failed {
        tag: text(&error["_tag"]).unwrap_or_else(|| "unknown".into()),
        message: text(&error["message"])
            .or_else(|| text(&error["detail"]))
            .or_else(|| text(&cause["defect"]))
            .unwrap_or_else(|| "the call failed".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(ws_url("http://127.0.0.1:3773").unwrap(), "ws://127.0.0.1:3773/ws?orchestrationProtocol=2");
        assert_eq!(
            ws_url("https://box.example.ts.net/").unwrap(),
            "wss://box.example.ts.net/ws?orchestrationProtocol=2"
        );
        assert_eq!(ws_url("ftp://x"), None);
        assert_eq!(ws_url("httpx://x"), None);
    }

    #[test]
    fn exits() {
        let ok = json!({"_tag": "Success", "value": {"n": 1}});
        assert_eq!(exit(ok).unwrap()["n"], 1);
        let denied = json!({"_tag": "Failure", "cause": [{"_tag": "Fail", "error": {
            "_tag": "EnvironmentAuthorizationError",
            "message": "The authenticated token is missing required scope: access:write."}}]});
        assert_eq!(
            exit(denied).unwrap_err(),
            Error::Failed {
                tag: "EnvironmentAuthorizationError".into(),
                message: "The authenticated token is missing required scope: access:write.".into()
            }
        );
        let window = json!({"_tag": "Failure", "cause": [{"_tag": "Fail", "error": {
            "_tag": "UsageReadError", "reason": "invalidWindow", "detail": "bad window"}}]});
        assert_eq!(exit(window).unwrap_err().to_string(), "bad window");
        assert!(matches!(exit(json!({"_tag": "Failure", "cause": []})).unwrap_err(), Error::Failed { .. }));
    }

    /// A stand-in server: checks the token, then answers each request with `answer`.
    // The handshake callback's signature is tungstenite's.
    #[allow(clippy::result_large_err)]
    pub(crate) fn serve(
        token: &'static str,
        answer: impl Fn(&str, &Value) -> Value + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        use tungstenite::handshake::server::{ErrorResponse, Request, Response};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            // One connection per test server is enough.
            let (stream, _) = listener.accept().unwrap();
            let check = |req: &Request, res: Response| -> Result<Response, ErrorResponse> {
                let ok = req.headers().get("authorization").is_some_and(|h| h == &format!("Bearer {token}"))
                    && req.uri().query() == Some("orchestrationProtocol=2");
                if ok {
                    Ok(res)
                } else {
                    Err(tungstenite::http::Response::builder().status(401).body(None).unwrap())
                }
            };
            let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else { return };
            while let Ok(msg) = ws.read() {
                let Message::Text(t) = msg else { continue };
                let req: Value = serde_json::from_str(t.as_str()).unwrap();
                assert_eq!(req["_tag"], "Request");
                let exit = answer(req["tag"].as_str().unwrap(), &req["payload"]);
                // An unrelated push first, then the answer in a batch.
                ws.send(Message::text(json!({"_tag": "Pong"}).to_string())).unwrap();
                let reply = json!([{ "_tag": "Exit", "requestId": req["id"], "exit": exit }]);
                ws.send(Message::text(reply.to_string())).unwrap();
            }
        });
        (base, handle)
    }

    #[test]
    fn calls_a_server() {
        let (base, server) = serve("secret", |tag, payload| match tag {
            "echo" => json!({"_tag": "Success", "value": payload}),
            _ => {
                json!({"_tag": "Failure", "cause": [{"_tag": "Fail", "error": {"_tag": "Nope", "message": "no such call"}}]})
            }
        });
        let mut rpc = Rpc::connect(&base, "secret").unwrap();
        let wait = Duration::from_secs(5);
        assert_eq!(rpc.call("echo", json!({"a": 1}), wait).unwrap()["a"], 1);
        assert_eq!(rpc.call("other", json!({}), wait).unwrap_err().to_string(), "no such call");
        assert_eq!(rpc.call("echo", json!({"b": 2}), wait).unwrap()["b"], 2, "ids keep answers apart");
        drop(rpc);
        server.join().unwrap();
    }

    #[test]
    fn a_wrong_token_is_unauthorized() {
        let (base, server) = serve("secret", |_, _| json!({}));
        assert_eq!(Rpc::connect(&base, "wrong").err(), Some(Error::Unauthorized));
        server.join().unwrap();
        assert!(matches!(Rpc::connect("http://127.0.0.1:1", "x"), Err(Error::Transport(_))));
    }
}
