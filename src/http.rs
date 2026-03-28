use mlua::{Function, Lua, RegistryKey};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::lua_error::{describe_lua_error, protect_lua_call};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HttpScheme {
    Http,
    Https,
}

struct ParsedHttpUrl {
    scheme: HttpScheme,
    host: String,
    host_header: String,
    port: u16,
    path: String,
}

struct HttpResponseEvent {
    request_id: u64,
    url: String,
    status: Option<u16>,
    headers: Vec<(String, String)>,
    body: String,
    error: Option<String>,
}

struct HttpState {
    next_request_id: u64,
    callbacks: HashMap<u64, RegistryKey>,
    sender: Sender<HttpResponseEvent>,
    receiver: Receiver<HttpResponseEvent>,
}

fn parse_http_url(url: &str) -> Result<ParsedHttpUrl, String> {
    let (scheme, rest, default_port) = if let Some(rest) = url.strip_prefix("http://") {
        (HttpScheme::Http, rest, 80)
    } else if let Some(rest) = url.strip_prefix("https://") {
        (HttpScheme::Https, rest, 443)
    } else {
        return Err("only http:// and https:// URLs are supported".to_string());
    };

    let (host_port, path) = if let Some(idx) = rest.find(['/', '?', '#']) {
        let suffix = &rest[idx..];
        let path_and_query = if suffix.starts_with('/') {
            suffix.to_string()
        } else {
            format!("/{suffix}")
        };
        let path = path_and_query.split('#').next().unwrap_or("/").to_string();
        (&rest[..idx], path)
    } else {
        (rest, "/".to_string())
    };

    if host_port.is_empty() {
        return Err("URL is missing host".to_string());
    }

    let (host, port) = if let Some(stripped) = host_port.strip_prefix('[') {
        let bracket_end = stripped
            .find(']')
            .ok_or_else(|| "invalid URL host: missing closing ']'".to_string())?;
        let host = stripped[..bracket_end].to_string();
        let remainder = &stripped[bracket_end + 1..];
        let port = if remainder.is_empty() {
            default_port
        } else if let Some(port_str) = remainder.strip_prefix(':') {
            port_str
                .parse::<u16>()
                .map_err(|_| format!("invalid port in URL: {port_str}"))?
        } else {
            return Err("invalid URL host/port separator".to_string());
        };
        (host, port)
    } else if let Some((host, port_str)) = host_port.rsplit_once(':') {
        if host.contains(':') {
            return Err("IPv6 URLs must wrap the host in []".to_string());
        }
        let port = port_str
            .parse::<u16>()
            .map_err(|_| format!("invalid port in URL: {port_str}"))?;
        (host.to_string(), port)
    } else {
        (host_port.to_string(), default_port)
    };

    if host.is_empty() {
        return Err("URL host is empty".to_string());
    }

    let host_header = if host.contains(':') {
        if port == default_port {
            format!("[{host}]")
        } else {
            format!("[{host}]:{port}")
        }
    } else if port == default_port {
        host.clone()
    } else {
        format!("{host}:{port}")
    };

    Ok(ParsedHttpUrl {
        scheme,
        host,
        host_header,
        port,
        path,
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn decode_chunked_body(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut index = 0usize;

    loop {
        let line_end_rel = find_bytes(&input[index..], b"\r\n")
            .ok_or_else(|| "invalid chunked body: missing chunk size delimiter".to_string())?;
        let line_end = index + line_end_rel;
        let size_line = std::str::from_utf8(&input[index..line_end])
            .map_err(|_| "invalid chunked body: non-utf8 chunk size".to_string())?;
        let size_hex = size_line
            .split(';')
            .next()
            .map(str::trim)
            .unwrap_or_default();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| format!("invalid chunk size: {size_hex}"))?;
        index = line_end + 2;

        if size == 0 {
            break;
        }
        if index + size > input.len() {
            return Err("invalid chunked body: truncated chunk data".to_string());
        }
        out.extend_from_slice(&input[index..index + size]);
        index += size;

        if input.get(index..index + 2) != Some(b"\r\n") {
            return Err("invalid chunked body: missing chunk terminator".to_string());
        }
        index += 2;
    }

    Ok(out)
}

fn parse_http_response(bytes: &[u8]) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
    let header_end = find_bytes(bytes, b"\r\n\r\n")
        .ok_or_else(|| "invalid HTTP response: header/body separator not found".to_string())?;
    let headers_raw = &bytes[..header_end];
    let body_raw = &bytes[header_end + 4..];

    let headers_text = String::from_utf8_lossy(headers_raw);
    let mut lines = headers_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| "invalid HTTP response: missing status line".to_string())?;
    let mut status_parts = status_line.split_whitespace();
    let _http_version = status_parts
        .next()
        .ok_or_else(|| "invalid HTTP response: missing protocol".to_string())?;
    let status = status_parts
        .next()
        .ok_or_else(|| "invalid HTTP response: missing status code".to_string())?
        .parse::<u16>()
        .map_err(|_| "invalid HTTP response: bad status code".to_string())?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let is_chunked = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("transfer-encoding")
            && value
                .to_ascii_lowercase()
                .split(',')
                .any(|part| part.trim() == "chunked")
    });

    let body = if is_chunked {
        decode_chunked_body(body_raw)?
    } else {
        body_raw.to_vec()
    };

    Ok((status, headers, body))
}

fn tls_client_config() -> &'static Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    })
}

fn configure_stream(stream: &TcpStream) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|err| format!("failed to set read timeout: {err}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|err| format!("failed to set write timeout: {err}"))
}

fn perform_http_request<T: Read + Write>(
    stream: &mut T,
    parsed: &ParsedHttpUrl,
) -> Result<(u16, Vec<(String, String)>, String), String> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: NeoLOVE\r\nConnection: close\r\nAccept: */*\r\n\r\n",
        parsed.path, parsed.host_header
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("failed to send request: {err}"))?;

    let mut raw_response = Vec::new();
    stream
        .read_to_end(&mut raw_response)
        .map_err(|err| format!("failed to read response: {err}"))?;

    let (status, headers, body_bytes) = parse_http_response(&raw_response)?;
    let body = String::from_utf8_lossy(&body_bytes).to_string();
    Ok((status, headers, body))
}

fn perform_http_get(url: &str) -> Result<(u16, Vec<(String, String)>, String), String> {
    let parsed = parse_http_url(url)?;
    let tcp_stream = TcpStream::connect((parsed.host.as_str(), parsed.port))
        .map_err(|err| format!("failed to connect: {err}"))?;
    configure_stream(&tcp_stream)?;

    match parsed.scheme {
        HttpScheme::Http => {
            let mut stream = tcp_stream;
            perform_http_request(&mut stream, &parsed)
        }
        HttpScheme::Https => {
            let server_name = ServerName::try_from(parsed.host.clone())
                .map_err(|_| format!("invalid TLS server name: {}", parsed.host))?;
            let connection = ClientConnection::new(tls_client_config().clone(), server_name)
                .map_err(|err| format!("failed to start TLS session: {err}"))?;
            let mut stream = StreamOwned::new(connection, tcp_stream);
            perform_http_request(&mut stream, &parsed)
        }
    }
}

pub(crate) fn add_http_module(lua: &Lua) -> mlua::Result<()> {
    let (sender, receiver) = mpsc::channel::<HttpResponseEvent>();
    let state = Rc::new(RefCell::new(HttpState {
        next_request_id: 1,
        callbacks: HashMap::new(),
        sender,
        receiver,
    }));

    let module = lua.create_table()?;

    let request_state = state.clone();
    let request = lua.create_function(move |lua, (url, callback): (String, Function)| {
        let (request_id, sender) = {
            let mut state = request_state.borrow_mut();
            let request_id = state.next_request_id;
            state.next_request_id = state.next_request_id.saturating_add(1);
            let callback_key = lua.create_registry_value(callback)?;
            state.callbacks.insert(request_id, callback_key);
            (request_id, state.sender.clone())
        };

        std::thread::spawn(move || {
            let event = match perform_http_get(&url) {
                Ok((status, headers, body)) => HttpResponseEvent {
                    request_id,
                    url,
                    status: Some(status),
                    headers,
                    body,
                    error: None,
                },
                Err(error) => HttpResponseEvent {
                    request_id,
                    url,
                    status: None,
                    headers: Vec::new(),
                    body: String::new(),
                    error: Some(error),
                },
            };
            let _ = sender.send(event);
        });

        Ok(request_id)
    })?;

    module.set("request", request.clone())?;
    module.set("get", request)?;

    let poll_state = state;
    module.set(
        "_poll",
        lua.create_function(move |lua, ()| {
            loop {
                let next_event = {
                    let state = poll_state.borrow();
                    state.receiver.try_recv()
                };

                let event = match next_event {
                    Ok(event) => event,
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                };

                let callback_key = {
                    let mut state = poll_state.borrow_mut();
                    state.callbacks.remove(&event.request_id)
                };

                let Some(callback_key) = callback_key else {
                    continue;
                };

                let callback: Function = lua.registry_value(&callback_key)?;
                let payload = lua.create_table()?;
                payload.set("ok", event.error.is_none())?;
                payload.set("url", event.url)?;
                payload.set("status", event.status)?;
                payload.set("body", event.body)?;
                payload.set("error", event.error)?;

                let headers = lua.create_table()?;
                for (name, value) in event.headers {
                    headers.set(name, value)?;
                }
                payload.set("headers", headers)?;

                let call_result =
                    protect_lua_call("running http callback", || callback.call::<()>(payload));
                lua.remove_registry_value(callback_key)?;
                if let Err(error) = call_result {
                    eprintln!(
                        "\x1b[31mLua Error in http callback:\x1b[0m\n{}",
                        describe_lua_error(&error)
                    );
                }
            }
            Ok(())
        })?,
    )?;

    lua.globals().set("http", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_url_supports_https_default_port() {
        let parsed = parse_http_url("https://example.com/path?q=1").unwrap();
        assert_eq!(parsed.scheme, HttpScheme::Https);
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.host_header, "example.com");
        assert_eq!(parsed.port, 443);
        assert_eq!(parsed.path, "/path?q=1");
    }

    #[test]
    fn parse_http_url_supports_custom_port_and_ipv6_host_header() {
        let parsed = parse_http_url("https://[::1]:8443/hello").unwrap();
        assert_eq!(parsed.host, "::1");
        assert_eq!(parsed.host_header, "[::1]:8443");
        assert_eq!(parsed.port, 8443);
        assert_eq!(parsed.path, "/hello");
    }
}
