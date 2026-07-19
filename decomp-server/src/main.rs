use base64::decode;
use hyper::body::HttpBody as _;
use hyper::header::{HeaderValue, CONTENT_LENGTH, CONTENT_TYPE};
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use log::{error, info, warn};
use luau_lifter::decompile_bytecode;
use std::any::Any;
use std::convert::Infallible;
use std::env;
use std::net::SocketAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

const MIB: usize = 1024 * 1024;
const DEFAULT_MAX_BODY_MIB: usize = 64;
const DEFAULT_WARN_MIB: usize = 8;
const DEFAULT_PORT: u16 = 9002;
const RECEIVE_PROGRESS_BYTES: usize = 8 * MIB;
const ROBLOX_ENCODE_KEY: u8 = 203;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_DECOMPILATIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct ServerConfig {
    max_body_bytes: usize,
    warn_bytes: usize,
}

fn read_mib_setting(name: &str, default_mib: usize) -> usize {
    match env::var(name) {
        Ok(value) => match value.parse::<usize>() {
            Ok(mib) => mib.saturating_mul(MIB),
            Err(_) => {
                warn!("ignoring invalid {name}={value:?}; using the default of {default_mib} MiB");
                default_mib * MIB
            }
        },
        Err(_) => default_mib * MIB,
    }
}

fn read_port() -> u16 {
    match env::var("DECOMP_PORT") {
        Ok(value) => match value.parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => {
                warn!("ignoring invalid DECOMP_PORT={value:?}; using {DEFAULT_PORT}");
                DEFAULT_PORT
            }
        },
        Err(_) => DEFAULT_PORT,
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= 1024 {
        format!("{:.2} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_limit(bytes: usize) -> String {
    if bytes == 0 {
        "unlimited".to_string()
    } else {
        format_bytes(bytes)
    }
}

fn text_response(status: StatusCode, body: String) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "unknown panic payload".to_string()
    }
}

async fn handle_request(
    request: Request<Body>,
    remote_addr: SocketAddr,
    config: ServerConfig,
) -> Result<Response<Body>, Infallible> {
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request_started = Instant::now();
    let (parts, mut body) = request.into_parts();
    let announced_length = parts
        .headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());

    info!(
        "[request {request_id}] accepted {remote_addr} {} {} (Content-Length: {})",
        parts.method,
        parts.uri,
        announced_length
            .map(format_bytes)
            .unwrap_or_else(|| "unknown".to_string())
    );

    if config.max_body_bytes != 0
        && announced_length.is_some_and(|length| length > config.max_body_bytes)
    {
        let message = format!(
            "request {request_id} rejected: encoded body exceeds the {} limit",
            format_bytes(config.max_body_bytes)
        );
        warn!("[request {request_id}] {message}");
        return Ok(text_response(StatusCode::PAYLOAD_TOO_LARGE, message));
    }

    let receive_started = Instant::now();
    let initial_capacity = announced_length.unwrap_or(0).min(RECEIVE_PROGRESS_BYTES);
    let mut encoded_body = Vec::with_capacity(initial_capacity);
    let mut next_progress = RECEIVE_PROGRESS_BYTES;

    while let Some(chunk_result) = body.data().await {
        let chunk = match chunk_result {
            Ok(chunk) => chunk,
            Err(error) => {
                let message = format!("request {request_id}: failed while reading body: {error}");
                error!("[request {request_id}] {message}");
                return Ok(text_response(StatusCode::BAD_REQUEST, message));
            }
        };

        if config.max_body_bytes != 0
            && chunk.len() > config.max_body_bytes.saturating_sub(encoded_body.len())
        {
            let message = format!(
                "request {request_id} rejected while receiving data: encoded body exceeded the {} limit",
                format_bytes(config.max_body_bytes)
            );
            warn!("[request {request_id}] {message}");
            return Ok(text_response(StatusCode::PAYLOAD_TOO_LARGE, message));
        }

        encoded_body.extend_from_slice(&chunk);

        if encoded_body.len() >= next_progress {
            if let Some(total) = announced_length.filter(|total| *total > 0) {
                info!(
                    "[request {request_id}] receiving body: {} / {} ({:.1}%)",
                    format_bytes(encoded_body.len()),
                    format_bytes(total),
                    encoded_body.len() as f64 * 100.0 / total as f64
                );
            } else {
                info!(
                    "[request {request_id}] receiving body: {} received",
                    format_bytes(encoded_body.len())
                );
            }

            next_progress = encoded_body
                .len()
                .saturating_div(RECEIVE_PROGRESS_BYTES)
                .saturating_add(1)
                .saturating_mul(RECEIVE_PROGRESS_BYTES);
        }
    }

    info!(
        "[request {request_id}] body received: {} of Base64 in {:.3}s",
        format_bytes(encoded_body.len()),
        receive_started.elapsed().as_secs_f64()
    );

    let decode_started = Instant::now();
    info!("[request {request_id}] decoding Base64");
    let bytecode = match tokio::task::spawn_blocking(move || decode(encoded_body)).await {
        Ok(Ok(bytecode)) => bytecode,
        Ok(Err(error)) => {
            let message = format!("request {request_id}: invalid Base64 bytecode: {error}");
            warn!("[request {request_id}] {message}");
            return Ok(text_response(StatusCode::BAD_REQUEST, message));
        }
        Err(error) => {
            let message = format!("request {request_id}: Base64 worker failed: {error}");
            error!("[request {request_id}] {message}");
            return Ok(text_response(StatusCode::INTERNAL_SERVER_ERROR, message));
        }
    };

    let version = bytecode.first().copied();
    let types_version = version
        .filter(|version| *version >= 4)
        .and_then(|_| bytecode.get(1).copied());
    info!(
        "[request {request_id}] decoded {} of bytecode in {:.3}s (bytecode version: {}, type version: {}, opcode key: {ROBLOX_ENCODE_KEY})",
        format_bytes(bytecode.len()),
        decode_started.elapsed().as_secs_f64(),
        version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "missing".to_string()),
        types_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "n/a".to_string())
    );

    if config.warn_bytes != 0 && bytecode.len() >= config.warn_bytes {
        warn!(
            "[request {request_id}] large bytecode input: {} (warning threshold: {})",
            format_bytes(bytecode.len()),
            format_bytes(config.warn_bytes)
        );
    }

    let decompile_started = Instant::now();
    let active = ACTIVE_DECOMPILATIONS.fetch_add(1, Ordering::SeqCst) + 1;
    info!("[request {request_id}] decompilation started (active decompilations: {active})");
    let decompile_result = tokio::task::spawn_blocking(move || {
        catch_unwind(AssertUnwindSafe(|| {
            decompile_bytecode(&bytecode, ROBLOX_ENCODE_KEY)
        }))
    })
    .await;
    let active = ACTIVE_DECOMPILATIONS.fetch_sub(1, Ordering::SeqCst) - 1;

    let output = match decompile_result {
        Ok(Ok(output)) => output,
        Ok(Err(payload)) => {
            let panic = panic_message(payload);
            let message = format!("request {request_id}: decompiler panicked: {panic}");
            error!(
                "[request {request_id}] {message} after {:.3}s (active decompilations: {active})",
                decompile_started.elapsed().as_secs_f64()
            );
            return Ok(text_response(StatusCode::INTERNAL_SERVER_ERROR, message));
        }
        Err(error) => {
            let message = format!("request {request_id}: decompiler worker failed: {error}");
            error!(
                "[request {request_id}] {message} after {:.3}s (active decompilations: {active})",
                decompile_started.elapsed().as_secs_f64()
            );
            return Ok(text_response(StatusCode::INTERNAL_SERVER_ERROR, message));
        }
    };

    if output.starts_with("failed to deserialize bytecode:") {
        warn!("[request {request_id}] decompiler returned: {output}");
    }
    if config.warn_bytes != 0 && output.len() >= config.warn_bytes {
        warn!(
            "[request {request_id}] large decompiled response: {}. The client must allocate and process this entire string",
            format_bytes(output.len())
        );
    }

    info!(
        "[request {request_id}] decompilation completed in {:.3}s; returning {} (total request time: {:.3}s, active decompilations: {active})",
        decompile_started.elapsed().as_secs_f64(),
        format_bytes(output.len()),
        request_started.elapsed().as_secs_f64()
    );

    Ok(text_response(StatusCode::OK, output))
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("DECOMP_LOG", "info"))
        .format_timestamp_millis()
        .init();

    let config = ServerConfig {
        max_body_bytes: read_mib_setting("DECOMP_MAX_BODY_MB", DEFAULT_MAX_BODY_MIB),
        warn_bytes: read_mib_setting("DECOMP_WARN_MB", DEFAULT_WARN_MIB),
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], read_port()));

    info!("decomp-server v{} starting", env!("CARGO_PKG_VERSION"));
    info!("diagnostic logging enabled (DECOMP_LOG default: info; example: DECOMP_LOG=debug)");
    info!(
        "request body limit: {} (set DECOMP_MAX_BODY_MB=0 for unlimited)",
        format_limit(config.max_body_bytes)
    );
    info!(
        "large input/output warning threshold: {} (configure with DECOMP_WARN_MB)",
        format_limit(config.warn_bytes)
    );

    let make_service = make_service_fn(move |connection: &AddrStream| {
        let remote_addr = connection.remote_addr();
        async move {
            Ok::<_, Infallible>(service_fn(move |request| {
                handle_request(request, remote_addr, config)
            }))
        }
    });

    let server = match Server::try_bind(&addr) {
        Ok(builder) => builder.serve(make_service),
        Err(error) => {
            error!("failed to listen on http://{addr}: {error}");
            error!(
                "another decomp-server may already be using port {}; set DECOMP_PORT to choose another",
                addr.port()
            );
            return;
        }
    };

    info!("listening on http://{addr}");
    if let Err(error) = server.await {
        error!("server error: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_diagnostic_sizes() {
        assert_eq!(format_bytes(12), "12 B");
        assert_eq!(format_bytes(1536), "1.50 KiB");
        assert_eq!(format_bytes(2 * MIB), "2.00 MiB");
        assert_eq!(format_limit(0), "unlimited");
    }

    #[test]
    fn extracts_string_panic_messages() {
        assert_eq!(panic_message(Box::new("boom")), "boom");
        assert_eq!(panic_message(Box::new("boom".to_string())), "boom");
    }
}
