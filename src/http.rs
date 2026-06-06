use mlua::{Function, Lua, RegistryKey, Table, Value, Variadic};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

#[derive(Clone, Debug)]
struct ScriptHttpRequest {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
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

fn validate_http_method(method: &str) -> Result<String, mlua::Error> {
    let method = method.trim().to_ascii_uppercase();
    if method.is_empty()
        || !method
            .bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'0'..=b'9' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'/' | b':' | b';' | b'=' | b'?' | b'@' | b'_' | b'~'))
    {
        return Err(mlua::Error::external("invalid HTTP method"));
    }
    Ok(method)
}

fn validate_header(name: String, value: String) -> Result<(String, String), mlua::Error> {
    if name.is_empty()
        || name.contains(['\r', '\n'])
        || value.contains(['\r', '\n'])
        || !name.bytes().all(|b| {
            matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'/' | b':' | b';' | b'=' | b'?' | b'@' | b'_' | b'~')
        })
    {
        return Err(mlua::Error::external("invalid HTTP header"));
    }
    Ok((name, value))
}

fn parse_headers_table(table: Table) -> mlua::Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    for pair in table.pairs::<String, String>() {
        let (name, value) = pair?;
        headers.push(validate_header(name, value)?);
    }
    Ok(headers)
}

fn parse_request_options(table: Table) -> mlua::Result<ScriptHttpRequest> {
    let url: String = table.get("url")?;
    let method = validate_http_method(table.get::<Option<String>>("method")?.as_deref().unwrap_or("GET"))?;
    let headers = match table.get::<Option<Table>>("headers")? {
        Some(headers) => parse_headers_table(headers)?,
        None => Vec::new(),
    };
    let body = match table.get::<Option<Value>>("body")? {
        Some(Value::String(body)) => body.as_bytes().to_vec(),
        Some(Value::Nil) | None => Vec::new(),
        Some(value) => {
            return Err(mlua::Error::external(format!(
                "HTTP request body must be a string, got {}",
                value.type_name()
            )));
        }
    };
    Ok(ScriptHttpRequest {
        url,
        method,
        headers,
        body,
    })
}

fn parse_request_args(args: Variadic<Value>) -> mlua::Result<(ScriptHttpRequest, Function)> {
    match args.as_slice() {
        [Value::String(url), Value::Function(callback)] => Ok((
            ScriptHttpRequest {
                url: url.to_str()?.to_string(),
                method: "GET".to_string(),
                headers: Vec::new(),
                body: Vec::new(),
            },
            callback.clone(),
        )),
        [Value::Table(options), Value::Function(callback)] => {
            Ok((parse_request_options(options.clone())?, callback.clone()))
        }
        [Value::String(url), Value::Table(options), Value::Function(callback)] => {
            let mut request = parse_request_options(options.clone())?;
            request.url = url.to_str()?.to_string();
            Ok((request, callback.clone()))
        }
        _ => Err(mlua::Error::external(
            "expected http.request(url, callback) or http.request(options, callback)",
        )),
    }
}

fn install_http_module(
    lua: &Lua,
    start_request: impl Fn(u64, ScriptHttpRequest, Sender<HttpResponseEvent>) + 'static,
    poll_extra: impl Fn(&Sender<HttpResponseEvent>) + 'static,
) -> mlua::Result<()> {
    let (sender, receiver) = mpsc::channel::<HttpResponseEvent>();
    let state = Rc::new(RefCell::new(HttpState {
        next_request_id: 1,
        callbacks: HashMap::new(),
        sender,
        receiver,
    }));

    let module = lua.create_table()?;

    let request_state = state.clone();
    let request = lua.create_function(move |lua, args: Variadic<Value>| {
        let (request, callback) = parse_request_args(args)?;
        let (request_id, sender) = {
            let mut state = request_state.borrow_mut();
            let request_id = state.next_request_id;
            state.next_request_id = state.next_request_id.saturating_add(1);
            let callback_key = lua.create_registry_value(callback)?;
            state.callbacks.insert(request_id, callback_key);
            (request_id, state.sender.clone())
        };
        start_request(request_id, request, sender);
        Ok(request_id)
    })?;

    let get = request.clone();
    module.set("request", request)?;
    module.set("get", get)?;

    let poll_state = state;
    let poll_sender = poll_state.borrow().sender.clone();
    module.set(
        "_poll",
        lua.create_function(move |lua, ()| {
            poll_extra(&poll_sender);
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

                let call_result = crate::lua_error::protect_lua_call("running http callback", || {
                    callback.call::<()>(payload)
                });
                lua.remove_registry_value(callback_key)?;
                if let Err(error) = call_result {
                    eprintln!(
                        "\x1b[31mLua Error in http callback:\x1b[0m\n{}",
                        crate::lua_error::describe_lua_error(&error)
                    );
                }
            }
            Ok(())
        })?,
    )?;

    lua.globals().set("http", module)?;
    Ok(())
}

#[cfg(not(target_os = "emscripten"))]
mod native {
    use super::*;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

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
        request: &ScriptHttpRequest,
    ) -> Result<(u16, Vec<(String, String)>, String), String> {
        let mut wire = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: NeoLOVE\r\nConnection: close\r\nAccept: */*\r\n",
            request.method, parsed.path, parsed.host_header
        );
        let mut has_content_type = false;
        let mut has_content_length = false;
        for (name, value) in &request.headers {
            if name.eq_ignore_ascii_case("content-type") {
                has_content_type = true;
            }
            if name.eq_ignore_ascii_case("content-length") {
                has_content_length = true;
                continue;
            }
            wire.push_str(name);
            wire.push_str(": ");
            wire.push_str(value);
            wire.push_str("\r\n");
        }
        if !has_content_type && !request.body.is_empty() {
            wire.push_str("Content-Type: application/octet-stream\r\n");
        }
        if !has_content_length && (request.method != "GET" || !request.body.is_empty()) {
            wire.push_str(format!("Content-Length: {}\r\n", request.body.len()).as_str());
        }
        wire.push_str("\r\n");

        stream
            .write_all(wire.as_bytes())
            .map_err(|err| format!("failed to send request headers: {err}"))?;
        if !request.body.is_empty() {
            stream
                .write_all(&request.body)
                .map_err(|err| format!("failed to send request body: {err}"))?;
        }

        let mut raw_response = Vec::new();
        stream
            .read_to_end(&mut raw_response)
            .map_err(|err| format!("failed to read response: {err}"))?;

        let (status, headers, body_bytes) = parse_http_response(&raw_response)?;
        let body = String::from_utf8_lossy(&body_bytes).to_string();
        Ok((status, headers, body))
    }

    fn perform_http_request_for_script(
        request: &ScriptHttpRequest,
    ) -> Result<(u16, Vec<(String, String)>, String), String> {
        let parsed = parse_http_url(&request.url)?;
        let tcp_stream = TcpStream::connect((parsed.host.as_str(), parsed.port))
            .map_err(|err| format!("failed to connect: {err}"))?;
        configure_stream(&tcp_stream)?;

        match parsed.scheme {
            HttpScheme::Http => {
                let mut stream = tcp_stream;
                perform_http_request(&mut stream, &parsed, request)
            }
            HttpScheme::Https => {
                let server_name = ServerName::try_from(parsed.host.clone())
                    .map_err(|_| format!("invalid TLS server name: {}", parsed.host))?;
                let connection = ClientConnection::new(tls_client_config().clone(), server_name)
                    .map_err(|err| format!("failed to start TLS session: {err}"))?;
                let mut stream = StreamOwned::new(connection, tcp_stream);
                perform_http_request(&mut stream, &parsed, request)
            }
        }
    }

    pub(crate) fn add_http_module(lua: &Lua) -> mlua::Result<()> {
        install_http_module(lua, |request_id, request, sender| {
            std::thread::spawn(move || {
                let event = match perform_http_request_for_script(&request) {
                    Ok((status, headers, body)) => HttpResponseEvent {
                        request_id,
                        url: request.url,
                        status: Some(status),
                        headers,
                        body,
                        error: None,
                    },
                    Err(error) => HttpResponseEvent {
                        request_id,
                        url: request.url,
                        status: None,
                        headers: Vec::new(),
                        body: String::new(),
                        error: Some(error),
                    },
                };
                let _ = sender.send(event);
            });
        }, |_sender| {})
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_http_url_supports_https_default_port() {
            let parsed = parse_http_url("https://example.com/path?q=1")
                .expect("https URL with default port should parse");
            assert_eq!(parsed.scheme, HttpScheme::Https);
            assert_eq!(parsed.host, "example.com");
            assert_eq!(parsed.host_header, "example.com");
            assert_eq!(parsed.port, 443);
            assert_eq!(parsed.path, "/path?q=1");
        }

        #[test]
        fn parse_http_url_supports_custom_port_and_ipv6_host_header() {
            let parsed = parse_http_url("https://[::1]:8443/hello")
                .expect("https URL with IPv6 host and custom port should parse");
            assert_eq!(parsed.host, "::1");
            assert_eq!(parsed.host_header, "[::1]:8443");
            assert_eq!(parsed.port, 8443);
            assert_eq!(parsed.path, "/hello");
        }
    }
}

#[cfg(target_os = "emscripten")]
mod native {
    use super::*;
    use std::ffi::{c_char, c_void, CStr, CString};

    unsafe extern "C" {
        fn neolove_web_http_start(
            request_id: i32,
            url: *const c_char,
            method: *const c_char,
            headers_json: *const c_char,
            body: *const u8,
            body_len: i32,
        ) -> i32;
        fn neolove_web_http_poll(request_id: *mut i32, status: *mut i32, ok: *mut i32) -> i32;
        fn neolove_web_http_copy_field(field: i32, buffer: *mut c_char, capacity: i32) -> i32;
    }

    fn cstring(value: &str) -> Result<CString, String> {
        CString::new(value).map_err(|_| "HTTP strings cannot contain NUL bytes".to_string())
    }

    fn copy_web_field(field: i32) -> Result<String, String> {
        let mut capacity = 1024i32;
        loop {
            let mut buffer = vec![0u8; capacity as usize];
            let written = unsafe {
                neolove_web_http_copy_field(field, buffer.as_mut_ptr() as *mut c_char, capacity)
            };
            if written >= 0 {
                let cstr = unsafe { CStr::from_ptr(buffer.as_ptr() as *const c_char) };
                return Ok(cstr.to_string_lossy().into_owned());
            }
            let required = -written;
            if required <= capacity {
                return Err("failed to copy web HTTP field".to_string());
            }
            capacity = required;
        }
    }

    pub(crate) fn add_http_module(lua: &Lua) -> mlua::Result<()> {
        install_http_module(
            lua,
            |request_id, request, sender| {
                let result = (|| -> Result<(), String> {
                    let url = cstring(&request.url)?;
                    let method = cstring(&request.method)?;
                    let headers_json = serde_json::to_string(
                        &request
                            .headers
                            .iter()
                            .cloned()
                            .collect::<HashMap<String, String>>(),
                    )
                    .map_err(|error| format!("failed to encode HTTP headers: {error}"))?;
                    let headers_json = cstring(&headers_json)?;
                    let body_ptr = if request.body.is_empty() {
                        std::ptr::null()
                    } else {
                        request.body.as_ptr()
                    };
                    let ok = unsafe {
                        neolove_web_http_start(
                            request_id as i32,
                            url.as_ptr(),
                            method.as_ptr(),
                            headers_json.as_ptr(),
                            body_ptr,
                            request.body.len() as i32,
                        )
                    };
                    if ok == 0 {
                        return Err("failed to start browser fetch".to_string());
                    }
                    Ok(())
                })();

                if let Err(error) = result {
                    let _ = sender.send(HttpResponseEvent {
                        request_id,
                        url: request.url,
                        status: None,
                        headers: Vec::new(),
                        body: String::new(),
                        error: Some(error),
                    });
                }
            },
            |sender| loop {
                let mut request_id = 0i32;
                let mut status = 0i32;
                let mut ok = 0i32;
                let has_event = unsafe {
                    neolove_web_http_poll(&mut request_id, &mut status, &mut ok)
                };
                if has_event == 0 {
                    break;
                }
                let url = copy_web_field(0).unwrap_or_default();
                let body = copy_web_field(1).unwrap_or_default();
                let error = copy_web_field(2).unwrap_or_default();
                let headers_json = copy_web_field(3).unwrap_or_else(|_| "{}".to_string());
                let headers_map: HashMap<String, String> =
                    serde_json::from_str(&headers_json).unwrap_or_default();
                let _ = sender.send(HttpResponseEvent {
                    request_id: request_id as u64,
                    url,
                    status: if status >= 0 { Some(status as u16) } else { None },
                    headers: headers_map.into_iter().collect(),
                    body,
                    error: if ok != 0 && error.is_empty() { None } else { Some(error) },
                });
            },
        )
    }

    #[allow(dead_code)]
    fn _unused(_: *mut c_void) {}
}

pub(crate) use native::add_http_module;
