//! 示教 MCP 服务（docs/integration/agent-teaching.md）：本机 HTTP 上的无状态 JSON-RPC，只开放候选示教工具。

pub mod canvas;

use base64::Engine;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{json, Value};
use std::convert::Infallible;
use tauri::AppHandle;

const DEFAULT_PORT: u16 = 47821;
const MIN_TOKEN: usize = 16;
const MAX_BODY: usize = 4 << 20;
/// 支持的协议版本，新的在前；客户端要的不在其中时回最新的
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const INSTRUCTIONS: &str = "GlueSight 段配方的走廊示教工具，只改候选。先 teach_status 看现状与下一步；画走廊前用 teach_view 看帧并按网格读坐标；改完用 teach_trial 试测。发布、检测项、公差与图纸标称值由人在界面确认。";

/// 工具返回的一段内容。
pub enum Content {
    Text(String),
    Png(Vec<u8>),
}

/// 按环境变量启动服务；没设令牌时不启动。
pub fn start(app: &AppHandle) {
    let Ok(token) = std::env::var("GLUESIGHT_MCP_TOKEN") else {
        return;
    };
    if token.chars().count() < MIN_TOKEN {
        crate::applog::write(
            "warn",
            "示教 MCP",
            &format!("GLUESIGHT_MCP_TOKEN 少于 {MIN_TOKEN} 个字符，服务未启动"),
        );
        return;
    }
    let port = match std::env::var("GLUESIGHT_MCP_PORT") {
        Ok(text) => match text.parse::<u16>() {
            Ok(port) => port,
            Err(_) => {
                crate::applog::write(
                    "warn",
                    "示教 MCP",
                    &format!("GLUESIGHT_MCP_PORT={text} 不是端口号，服务未启动"),
                );
                return;
            }
        },
        Err(_) => DEFAULT_PORT,
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => listener,
            Err(e) => {
                crate::applog::write(
                    "error",
                    "示教 MCP",
                    &format!("监听 127.0.0.1:{port} 失败：{e}"),
                );
                return;
            }
        };
        crate::applog::write(
            "info",
            "示教 MCP",
            &format!("已在 http://127.0.0.1:{port}/mcp 提供示教工具"),
        );
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let (app, token) = (app.clone(), token.clone());
            tauri::async_runtime::spawn(async move {
                let service = hyper::service::service_fn(move |request| {
                    handle(app.clone(), token.clone(), request)
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
}

fn reply(status: StatusCode, body: Option<Value>) -> Response<Full<Bytes>> {
    let mut response = Response::builder().status(status);
    if body.is_some() {
        response = response.header("Content-Type", "application/json");
    }
    if status == StatusCode::METHOD_NOT_ALLOWED {
        response = response.header("Allow", "POST");
    }
    let bytes = body.map_or_else(Bytes::new, |b| Bytes::from(b.to_string()));
    response.body(Full::new(bytes)).unwrap()
}

async fn handle(
    app: AppHandle,
    token: String,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    Ok(respond(&token, request, |method, params| async move {
        dispatch(&app, &method, &params).await
    })
    .await)
}

/// 一次 HTTP 请求：先准入，再解析 JSON-RPC，把方法交给 `dispatch`
async fn respond<B, F, Fut>(token: &str, request: Request<B>, dispatch: F) -> Response<Full<Bytes>>
where
    B: hyper::body::Body,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    F: FnOnce(String, Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, (i64, String)>>,
{
    if let Err(status) = admit(
        request.uri().path(),
        request.method(),
        request.headers(),
        token,
    ) {
        return reply(status, None);
    }
    let body = match Limited::new(request.into_body(), MAX_BODY).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return reply(StatusCode::PAYLOAD_TOO_LARGE, None),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(e) => {
            return reply(
                StatusCode::BAD_REQUEST,
                Some(rpc_error(
                    Value::Null,
                    -32700,
                    &format!("JSON 解析失败：{e}"),
                )),
            )
        }
    };
    let Some(id) = message.get("id").cloned() else {
        // 通知与客户端回的响应都不需要回复
        return reply(StatusCode::ACCEPTED, None);
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let body = match dispatch(method, params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => rpc_error(id, code, &message),
    };
    reply(StatusCode::OK, Some(body))
}

async fn dispatch(app: &AppHandle, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": crate::workspace::agent::tools() })),
        "tools/call" => call(app, params).await,
        _ => Err((-32601, format!("不支持的方法：{method}"))),
    }
}

/// 准入：路径、浏览器来源、令牌、方法依次检查
fn admit(
    path: &str,
    method: &Method,
    headers: &hyper::HeaderMap,
    token: &str,
) -> Result<(), StatusCode> {
    if path != "/mcp" {
        return Err(StatusCode::NOT_FOUND);
    }
    // 浏览器发出的请求都带 Origin：拒绝，免得网页借本机地址调用
    if headers.contains_key(hyper::header::ORIGIN) {
        return Err(StatusCode::FORBIDDEN);
    }
    let authorized = headers
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|v| v == token);
    if !authorized {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if method != Method::POST {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    Ok(())
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|v| PROTOCOLS.contains(v))
        .unwrap_or(PROTOCOLS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "gluesight", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

async fn call(app: &AppHandle, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "缺少工具名".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let outcome = crate::workspace::agent::call(app, name, arguments).await;
    let (content, failed) = match outcome {
        Ok(content) => (content, false),
        Err(crate::workspace::agent::ToolError::Unknown) => {
            return Err((-32602, format!("没有工具 {name}")))
        }
        Err(crate::workspace::agent::ToolError::Failed(message)) => {
            (vec![Content::Text(message)], true)
        }
    };
    let content: Vec<Value> = content
        .into_iter()
        .map(|c| match c {
            Content::Text(text) => json!({ "type": "text", "text": text }),
            Content::Png(bytes) => json!({
                "type": "image",
                "mimeType": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        })
        .collect();
    Ok(json!({ "content": content, "isError": failed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn requests_without_the_token_or_from_a_browser_never_reach_the_tools() {
        let token = "0123456789abcdef";
        let bearer = format!("Bearer {token}");
        let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let cases: [(Method, Vec<(&str, &str)>, StatusCode); 5] = [
            (
                Method::POST,
                vec![("authorization", &bearer)],
                StatusCode::OK,
            ),
            (Method::POST, vec![], StatusCode::UNAUTHORIZED),
            (
                Method::POST,
                vec![("authorization", "Bearer 0123456789abcdeX")],
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::POST,
                vec![
                    ("authorization", &bearer),
                    ("origin", "http://evil.example"),
                ],
                StatusCode::FORBIDDEN,
            ),
            (
                Method::GET,
                vec![("authorization", &bearer)],
                StatusCode::METHOD_NOT_ALLOWED,
            ),
        ];
        for (method, headers, expected) in cases {
            let mut request = Request::builder().method(method.clone()).uri("/mcp");
            for (name, value) in &headers {
                request = request.header(*name, *value);
            }
            let request = request.body(Full::new(Bytes::from(ping))).unwrap();
            let reached = AtomicUsize::new(0);
            let response = tauri::async_runtime::block_on(respond(token, request, |_, _| {
                reached.fetch_add(1, Ordering::SeqCst);
                async { Ok(json!({})) }
            }));
            assert_eq!(response.status(), expected, "{method} {headers:?}");
            assert_eq!(
                reached.load(Ordering::SeqCst),
                usize::from(expected == StatusCode::OK),
                "{method} {headers:?}"
            );
        }
    }
}
