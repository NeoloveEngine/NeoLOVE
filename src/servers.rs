#[cfg(not(target_os = "emscripten"))]
mod native {
    use mlua::{Buffer, Compiler, Function, Lua, MultiValue, RegistryKey, Table, TextRequirer, Value};
    use ring::digest;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
    use rustls::{
        ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
    };
    use serde::{Deserialize, Serialize};
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::fs::{self, File};
    use std::io::{BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Component, Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
    use std::sync::{Arc, Condvar, Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    use crate::lua_error::{describe_lua_error, protect_lua_call};

    const MAX_HTTP_HEADER_SIZE: usize = 64 * 1024;
    const MAX_HTTP_BODY_SIZE: usize = 16 * 1024 * 1024;
    const REMOTE_POLL_TIMEOUT_MS: u64 = 5_000;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum HttpScheme {
        Http,
        Https,
    }

    impl HttpScheme {
        fn as_str(self) -> &'static str {
            match self {
                Self::Http => "http",
                Self::Https => "https",
            }
        }
    }

    #[derive(Debug)]
    struct ParsedHttpUrl {
        scheme: HttpScheme,
        host: String,
        host_header: String,
        port: u16,
        path: String,
    }

    #[derive(Debug)]
    struct HttpBinaryResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    #[derive(Debug)]
    struct HttpRequest {
        method: String,
        path: String,
        query: Option<String>,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    #[derive(Debug)]
    struct HttpResponse {
        status: u16,
        reason: &'static str,
        content_type: &'static str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    #[derive(Debug, Clone)]
    enum ClientEvent {
        Payload(Vec<u8>),
        Kicked(Option<String>),
        Closed(Option<String>),
    }

    #[derive(Debug)]
    struct ServerInboundEvent {
        client_key: String,
        payload: Vec<u8>,
    }

    #[derive(Debug)]
    struct RuntimeClientState {
        is_host: bool,
        tags: Vec<String>,
        connected: bool,
        kicked_reason: Option<String>,
        outbound_messages: VecDeque<Vec<u8>>,
        local_sender: Option<Sender<ClientEvent>>,
    }

    #[derive(Debug)]
    struct HostedServerShared {
        clients: Mutex<HashMap<String, RuntimeClientState>>,
        condvar: Condvar,
        stop_flag: AtomicBool,
        host_client_key: String,
        inbound_sender: Sender<ServerInboundEvent>,
    }

    #[derive(Clone)]
    enum ClientTransport {
        Local { shared: Arc<HostedServerShared> },
        Remote {
            base_url: String,
            outbound_sender: Sender<Vec<u8>>,
            stop_flag: Arc<AtomicBool>,
        },
    }

    struct ClientHandleState {
        key: String,
        is_host: bool,
        connected: bool,
        kick_reason: Option<String>,
        callbacks: Vec<RegistryKey>,
        receiver: Receiver<ClientEvent>,
        transport: ClientTransport,
    }

    struct HostedHandleState {
        shared: Arc<HostedServerShared>,
    }

    struct ServersState {
        next_client_id: u64,
        next_hosted_id: u64,
        clients: HashMap<u64, ClientHandleState>,
        hosted: HashMap<u64, HostedHandleState>,
    }

    #[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
    enum PackedValue {
        Nil,
        Boolean(bool),
        Integer(i64),
        Number(f64),
        String(String),
        Buffer(Vec<u8>),
        Array(Vec<PackedValue>),
        Map(Vec<(PackedValue, PackedValue)>),
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct ConnectResponse {
        ok: bool,
        client_key: String,
        is_host: bool,
        tags: Vec<String>,
        error: Option<String>,
    }

    #[derive(Debug, Serialize)]
    struct StatusResponse {
        ok: bool,
        transport: &'static str,
        status: &'static str,
    }

    #[derive(Debug)]
    enum PollResult {
        Message(Vec<u8>),
        Empty,
        Kicked(Option<String>),
        UnknownClient,
    }

    #[derive(Debug)]
    enum RemotePollResult {
        Payload(Vec<u8>),
        Empty,
        Kicked(Option<String>),
    }

    fn normalize_path(path: &Path) -> PathBuf {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::Normal(part) => normalized.push(part),
                Component::RootDir | Component::Prefix(_) => {
                    normalized.push(component.as_os_str());
                }
            }
        }
        normalized
    }

    fn resolve_project_path(root: &Path, input: &str) -> Result<PathBuf, String> {
        let path = PathBuf::from(input);
        let candidate = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        let resolved = normalize_path(&candidate);
        if !resolved.starts_with(root) {
            return Err(format!("path escapes project root: {input}"));
        }
        Ok(resolved)
    }

    fn canonicalize_project_path(root: &Path, input: &str) -> Result<PathBuf, String> {
        let resolved = resolve_project_path(root, input)?;
        let canonical = resolved
            .canonicalize()
            .map_err(|error| format!("failed to resolve '{}': {error}", resolved.display()))?;
        if !canonical.starts_with(root) {
            return Err(format!("path escapes project root: {input}"));
        }
        Ok(canonical)
    }

    fn format_socket_addr(host: &str, port: u16) -> String {
        if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        }
    }

    fn format_url_host(host: &str) -> String {
        if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        }
    }

    fn format_public_url(scheme: HttpScheme, host: &str, port: u16) -> String {
        format!("{}://{}:{port}", scheme.as_str(), format_url_host(host))
    }

    fn normalize_base_url(url: &str) -> String {
        let trimmed = url.trim().trim_end_matches('/').to_string();
        if trimmed.contains("://") {
            trimmed
        } else {
            format!("http://{trimmed}")
        }
    }

    fn join_url(base: &str, suffix: &str) -> String {
        format!("{}{}", base.trim_end_matches('/'), suffix)
    }

    fn get_option_string(options: &Table, names: &[&str]) -> mlua::Result<Option<String>> {
        for name in names {
            match options.get::<Value>(*name)? {
                Value::Nil => {}
                Value::String(value) => {
                    return Ok(Some(value.to_str()?.to_string()));
                }
                _ => {
                    return Err(mlua::Error::external(format!(
                        "option '{name}' must be a string"
                    )));
                }
            }
        }
        Ok(None)
    }

    fn extract_last_value(args: MultiValue, what: &str) -> mlua::Result<Value> {
        args.into_vec()
            .pop()
            .ok_or_else(|| mlua::Error::external(format!("expected {what}")))
    }

    fn extract_last_buffer(args: MultiValue) -> mlua::Result<Buffer> {
        match extract_last_value(args, "buffer payload")? {
            Value::Buffer(buffer) => Ok(buffer),
            _ => Err(mlua::Error::external("expected buffer payload")),
        }
    }

    fn extract_last_function(args: MultiValue) -> mlua::Result<Function> {
        match extract_last_value(args, "callback function")? {
            Value::Function(function) => Ok(function),
            _ => Err(mlua::Error::external("expected callback function")),
        }
    }

    fn bytes_to_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = digest::digest(&digest::SHA256, bytes);
        bytes_to_hex(digest.as_ref())
    }

    fn sha128_hex(bytes: &[u8]) -> String {
        let digest = digest::digest(&digest::SHA256, bytes);
        bytes_to_hex(&digest.as_ref()[..16])
    }

    fn bytes_from_hash_input(value: Value) -> mlua::Result<Vec<u8>> {
        match value {
            Value::String(value) => Ok(value.as_bytes().to_vec()),
            Value::Buffer(buffer) => Ok(buffer.to_vec()),
            _ => Err(mlua::Error::external(
                "hash input must be a string or buffer",
            )),
        }
    }

    fn pack_lua_value(value: Value, visited: &mut HashSet<usize>) -> mlua::Result<PackedValue> {
        match value {
            Value::Nil => Ok(PackedValue::Nil),
            Value::Boolean(value) => Ok(PackedValue::Boolean(value)),
            Value::Integer(value) => Ok(PackedValue::Integer(value)),
            Value::Number(value) => Ok(PackedValue::Number(value)),
            Value::String(value) => Ok(PackedValue::String(value.to_str()?.to_string())),
            Value::Buffer(buffer) => Ok(PackedValue::Buffer(buffer.to_vec())),
            Value::Table(table) => pack_table(table, visited),
            other => Err(mlua::Error::external(format!(
                "cannot serialize Lua value of type '{}'",
                other.type_name()
            ))),
        }
    }

    fn pack_table(table: Table, visited: &mut HashSet<usize>) -> mlua::Result<PackedValue> {
        let pointer = table.to_pointer() as usize;
        if !visited.insert(pointer) {
            return Err(mlua::Error::external(
                "cannot serialize cyclic tables into a buffer",
            ));
        }

        let mut entries = Vec::<(Value, Value)>::new();
        for pair in table.pairs::<Value, Value>() {
            entries.push(pair?);
        }

        let len = table.raw_len();
        let is_sequence = if entries.len() == len {
            let mut seen = vec![false; len];
            let mut values = vec![Value::Nil; len];
            let mut ok = true;
            for (key, value) in &entries {
                match key {
                    Value::Integer(index) if *index >= 1 && (*index as usize) <= len => {
                        let slot = (*index as usize) - 1;
                        seen[slot] = true;
                        values[slot] = value.clone();
                    }
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && seen.into_iter().all(|value| value) {
                Some(values)
            } else {
                None
            }
        } else if entries.is_empty() && len == 0 {
            Some(Vec::new())
        } else {
            None
        };

        let packed = if let Some(values) = is_sequence {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(pack_lua_value(value, visited)?);
            }
            PackedValue::Array(out)
        } else {
            let mut out = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                out.push((
                    pack_lua_value(key, visited)?,
                    pack_lua_value(value, visited)?,
                ));
            }
            PackedValue::Map(out)
        };

        visited.remove(&pointer);
        Ok(packed)
    }

    fn unpack_packed_value(lua: &Lua, value: PackedValue) -> mlua::Result<Value> {
        match value {
            PackedValue::Nil => Ok(Value::Nil),
            PackedValue::Boolean(value) => Ok(Value::Boolean(value)),
            PackedValue::Integer(value) => Ok(Value::Integer(value)),
            PackedValue::Number(value) => Ok(Value::Number(value)),
            PackedValue::String(value) => Ok(Value::String(lua.create_string(&value)?)),
            PackedValue::Buffer(bytes) => Ok(Value::Buffer(lua.create_buffer(bytes)?)),
            PackedValue::Array(values) => {
                let table = lua.create_table()?;
                for value in values {
                    table.push(unpack_packed_value(lua, value)?)?;
                }
                Ok(Value::Table(table))
            }
            PackedValue::Map(entries) => {
                let table = lua.create_table()?;
                for (key, value) in entries {
                    table.set(unpack_packed_value(lua, key)?, unpack_packed_value(lua, value)?)?;
                }
                Ok(Value::Table(table))
            }
        }
    }

    fn serialize_table(lua: &Lua, table: Table) -> mlua::Result<Buffer> {
        let packed = pack_table(table, &mut HashSet::new())?;
        let bytes = rmp_serde::to_vec(&packed).map_err(mlua::Error::external)?;
        lua.create_buffer(bytes)
    }

    fn deserialize_table(lua: &Lua, buffer: Buffer) -> mlua::Result<Table> {
        let packed: PackedValue =
            rmp_serde::from_slice(&buffer.to_vec()).map_err(mlua::Error::external)?;
        match unpack_packed_value(lua, packed)? {
            Value::Table(table) => Ok(table),
            _ => Err(mlua::Error::external(
                "deserialized value was not a table",
            )),
        }
    }

    fn install_common_helpers(lua: &Lua, module: &Table) -> mlua::Result<()> {
        let serialize = lua.create_function(|lua, table: Table| serialize_table(lua, table))?;
        module.set("serializeTable", serialize.clone())?;
        module.set("serialize_table", serialize)?;

        let deserialize =
            lua.create_function(|lua, buffer: Buffer| deserialize_table(lua, buffer))?;
        module.set("deserializeTable", deserialize.clone())?;
        module.set("deserialize_table", deserialize)?;

        let uuid4 = lua.create_function(|_lua, ()| Ok(Uuid::new_v4().to_string()))?;
        module.set("generate_uuid4", uuid4.clone())?;
        module.set("generateUuid4", uuid4)?;

        let uuid7 = lua.create_function(|_lua, ()| Ok(Uuid::now_v7().to_string()))?;
        module.set("generate_uuid7", uuid7.clone())?;
        module.set("generateUuid7", uuid7)?;

        let sha256 = lua.create_function(|_lua, value: Value| {
            let bytes = bytes_from_hash_input(value)?;
            Ok(sha256_hex(&bytes))
        })?;
        module.set("sha256", sha256)?;

        let sha128 = lua.create_function(|_lua, value: Value| {
            let bytes = bytes_from_hash_input(value)?;
            Ok(sha128_hex(&bytes))
        })?;
        module.set("sha128", sha128)?;
        Ok(())
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn parse_http_url(url: &str) -> Result<ParsedHttpUrl, String> {
        let (scheme, rest, default_port) = if let Some(rest) = url.strip_prefix("http://") {
            (HttpScheme::Http, rest, 80)
        } else if let Some(rest) = url.strip_prefix("https://") {
            (HttpScheme::Https, rest, 443)
        } else {
            return Err("only http:// and https:// URLs are supported".to_string());
        };

        let (host_port, path) = if let Some(index) = rest.find(['/', '?', '#']) {
            let suffix = &rest[index..];
            let path_and_query = if suffix.starts_with('/') {
                suffix.to_string()
            } else {
                format!("/{suffix}")
            };
            let path = path_and_query.split('#').next().unwrap_or("/").to_string();
            (&rest[..index], path)
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

    fn parse_http_response(bytes: &[u8]) -> Result<HttpBinaryResponse, String> {
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

        Ok(HttpBinaryResponse {
            status,
            headers,
            body,
        })
    }

    fn configure_stream(stream: &TcpStream) -> Result<(), String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .map_err(|error| format!("failed to set read timeout: {error}"))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(15)))
            .map_err(|error| format!("failed to set write timeout: {error}"))
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

    fn perform_http_request<T: Read + Write>(
        stream: &mut T,
        parsed: &ParsedHttpUrl,
        method: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<HttpBinaryResponse, String> {
        let mut request = format!(
            "{method} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: NeoLOVE/servers\r\nConnection: close\r\nAccept: */*\r\n",
            parsed.path, parsed.host_header
        );
        let has_content_type = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        for (name, value) in headers {
            request.push_str(name);
            request.push_str(": ");
            request.push_str(value);
            request.push_str("\r\n");
        }
        if !has_content_type && !body.is_empty() {
            request.push_str("Content-Type: application/octet-stream\r\n");
        }
        if method != "GET" || !body.is_empty() {
            request.push_str(format!("Content-Length: {}\r\n", body.len()).as_str());
        }
        request.push_str("\r\n");

        stream
            .write_all(request.as_bytes())
            .map_err(|error| format!("failed to send request headers: {error}"))?;
        if !body.is_empty() {
            stream
                .write_all(body)
                .map_err(|error| format!("failed to send request body: {error}"))?;
        }

        let mut raw_response = Vec::new();
        stream
            .read_to_end(&mut raw_response)
            .map_err(|error| format!("failed to read response: {error}"))?;

        parse_http_response(&raw_response)
    }

    fn perform_binary_http_request(
        url: &str,
        method: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<HttpBinaryResponse, String> {
        let parsed = parse_http_url(url)?;
        let tcp_stream = TcpStream::connect((parsed.host.as_str(), parsed.port))
            .map_err(|error| format!("failed to connect: {error}"))?;
        configure_stream(&tcp_stream)?;

        match parsed.scheme {
            HttpScheme::Http => {
                let mut stream = tcp_stream;
                perform_http_request(&mut stream, &parsed, method, headers, body)
            }
            HttpScheme::Https => {
                let server_name = ServerName::try_from(parsed.host.clone())
                    .map_err(|_| format!("invalid TLS server name: {}", parsed.host))?;
                let connection = ClientConnection::new(tls_client_config().clone(), server_name)
                    .map_err(|error| format!("failed to start TLS session: {error}"))?;
                let mut stream = StreamOwned::new(connection, tcp_stream);
                perform_http_request(&mut stream, &parsed, method, headers, body)
            }
        }
    }

    fn find_header_case_insensitive(headers: &[(String, String)], target: &str) -> Option<String> {
        headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(target))
            .map(|(_, value)| value.clone())
    }

    fn read_http_request<R: Read>(stream: &mut R) -> Result<HttpRequest, String> {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let read = stream
                .read(&mut chunk)
                .map_err(|error| format!("failed to read request: {error}"))?;
            if read == 0 {
                return Err("connection closed before request completed".to_string());
            }
            raw.extend_from_slice(&chunk[..read]);
            if raw.len() > MAX_HTTP_HEADER_SIZE {
                return Err("request headers are too large".to_string());
            }
            if let Some(index) = find_bytes(&raw, b"\r\n\r\n") {
                break index;
            }
        };

        let header_bytes = &raw[..header_end];
        let header_text = std::str::from_utf8(header_bytes)
            .map_err(|_| "request headers were not valid utf-8".to_string())?;
        let mut lines = header_text.split("\r\n");
        let request_line = lines
            .next()
            .ok_or_else(|| "missing request line".to_string())?;
        let mut request_line_parts = request_line.split_whitespace();
        let method = request_line_parts
            .next()
            .ok_or_else(|| "missing request method".to_string())?
            .to_string();
        let target = request_line_parts
            .next()
            .ok_or_else(|| "missing request target".to_string())?
            .to_string();

        let mut headers = HashMap::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }

        let content_length = headers
            .get("content-length")
            .map(|value| value.parse::<usize>())
            .transpose()
            .map_err(|_| "invalid content-length header".to_string())?
            .unwrap_or(0);
        if content_length > MAX_HTTP_BODY_SIZE {
            return Err("request body is too large".to_string());
        }

        let body_start = header_end + 4;
        let mut body = raw[body_start..].to_vec();
        while body.len() < content_length {
            let read = stream
                .read(&mut chunk)
                .map_err(|error| format!("failed to read request body: {error}"))?;
            if read == 0 {
                return Err("connection closed before request body completed".to_string());
            }
            body.extend_from_slice(&chunk[..read]);
            if body.len() > MAX_HTTP_BODY_SIZE {
                return Err("request body is too large".to_string());
            }
        }
        body.truncate(content_length);

        let (path, query) = if let Some((path, query)) = target.split_once('?') {
            (path.to_string(), Some(query.to_string()))
        } else {
            (target, None)
        };

        Ok(HttpRequest {
            method,
            path,
            query,
            headers,
            body,
        })
    }

    fn sanitize_header_value(value: &str) -> String {
        value.replace(['\r', '\n'], " ")
    }

    fn write_http_response<W: Write>(stream: &mut W, response: HttpResponse) -> Result<(), String> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, X-NeoLOVE-Client-Key\r\n",
            response.status,
            response.reason,
            response.content_type,
            response.body.len()
        );

        for (name, value) in response.headers {
            head.push_str(&name);
            head.push_str(": ");
            head.push_str(&sanitize_header_value(&value));
            head.push_str("\r\n");
        }
        head.push_str("\r\n");

        stream
            .write_all(head.as_bytes())
            .map_err(|error| format!("failed to write response headers: {error}"))?;
        if !response.body.is_empty() {
            stream
                .write_all(&response.body)
                .map_err(|error| format!("failed to write response body: {error}"))?;
        }
        Ok(())
    }

    fn json_response<T: Serialize>(
        status: u16,
        reason: &'static str,
        value: &T,
    ) -> Result<HttpResponse, String> {
        let body = serde_json::to_vec(value).map_err(|error| format!("failed to encode json: {error}"))?;
        Ok(HttpResponse {
            status,
            reason,
            content_type: "application/json",
            headers: Vec::new(),
            body,
        })
    }

    fn plain_response(status: u16, reason: &'static str, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn empty_response(status: u16, reason: &'static str) -> HttpResponse {
        HttpResponse {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn lock_clients(
        shared: &HostedServerShared,
    ) -> std::sync::MutexGuard<'_, HashMap<String, RuntimeClientState>> {
        shared
            .clients
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn new_runtime_client_state(
        is_host: bool,
        local_sender: Option<Sender<ClientEvent>>,
    ) -> RuntimeClientState {
        let tags = if is_host {
            vec!["host".to_string()]
        } else {
            Vec::new()
        };
        RuntimeClientState {
            is_host,
            tags,
            connected: true,
            kicked_reason: None,
            outbound_messages: VecDeque::new(),
            local_sender,
        }
    }

    fn generate_client_key() -> String {
        Uuid::now_v7().to_string()
    }

    fn register_remote_client(shared: &Arc<HostedServerShared>) -> ConnectResponse {
        let client_key = generate_client_key();
        let mut clients = lock_clients(shared);
        clients.insert(client_key.clone(), new_runtime_client_state(false, None));
        shared.condvar.notify_all();
        ConnectResponse {
            ok: true,
            client_key,
            is_host: false,
            tags: Vec::new(),
            error: None,
        }
    }

    fn client_is_active(shared: &Arc<HostedServerShared>, client_key: &str) -> bool {
        let clients = lock_clients(shared);
        clients
            .get(client_key)
            .map(|client| client.connected)
            .unwrap_or(false)
    }

    fn send_to_client(
        shared: &Arc<HostedServerShared>,
        client_key: &str,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        let mut payload = Some(payload);
        let maybe_sender = {
            let mut clients = lock_clients(shared);
            let client = clients
                .get_mut(client_key)
                .ok_or_else(|| format!("unknown client key: {client_key}"))?;
            if !client.connected {
                return Err(format!("client is disconnected: {client_key}"));
            }
            if let Some(sender) = client.local_sender.clone() {
                Some(sender)
            } else {
                client
                    .outbound_messages
                    .push_back(payload.take().expect("payload already consumed"));
                shared.condvar.notify_all();
                None
            }
        };

        if let Some(sender) = maybe_sender {
            sender
                .send(ClientEvent::Payload(
                    payload.take().expect("payload already consumed"),
                ))
                .map_err(|_| format!("failed to deliver payload to client: {client_key}"))?;
        }
        Ok(())
    }

    fn kick_client(
        shared: &Arc<HostedServerShared>,
        client_key: &str,
        reason: Option<String>,
    ) -> Result<(), String> {
        let mut local_event = None;
        {
            let mut clients = lock_clients(shared);
            let client = clients
                .get_mut(client_key)
                .ok_or_else(|| format!("unknown client key: {client_key}"))?;
            client.connected = false;
            client.kicked_reason = reason.clone();
            if let Some(sender) = client.local_sender.clone() {
                local_event = Some(sender);
            }
        }
        shared.condvar.notify_all();
        if let Some(sender) = local_event {
            let _ = sender.send(ClientEvent::Kicked(reason));
        }
        Ok(())
    }

    fn disconnect_client(
        shared: &Arc<HostedServerShared>,
        client_key: &str,
        reason: Option<String>,
    ) -> bool {
        let changed = {
            let mut clients = lock_clients(shared);
            let Some(client) = clients.get_mut(client_key) else {
                return false;
            };
            let changed = client.connected;
            client.connected = false;
            if client.kicked_reason.is_none() {
                client.kicked_reason = reason;
            }
            changed
        };
        shared.condvar.notify_all();
        changed
    }

    fn client_tags(shared: &Arc<HostedServerShared>, client_key: &str) -> Option<Vec<String>> {
        let clients = lock_clients(shared);
        clients.get(client_key).map(|client| client.tags.clone())
    }

    fn client_is_host(shared: &Arc<HostedServerShared>, client_key: &str) -> bool {
        let clients = lock_clients(shared);
        clients
            .get(client_key)
            .map(|client| client.is_host)
            .unwrap_or(false)
    }

    fn wait_for_client_event(
        shared: &Arc<HostedServerShared>,
        client_key: &str,
        timeout: Duration,
    ) -> PollResult {
        let start = Instant::now();
        let mut clients = lock_clients(shared);
        loop {
            let Some(client) = clients.get_mut(client_key) else {
                return PollResult::UnknownClient;
            };

            if let Some(payload) = client.outbound_messages.pop_front() {
                return PollResult::Message(payload);
            }
            if !client.connected || shared.stop_flag.load(Ordering::SeqCst) {
                return PollResult::Kicked(client.kicked_reason.clone());
            }

            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return PollResult::Empty;
            }
            let remaining = timeout.saturating_sub(elapsed);
            let wait_result = shared.condvar.wait_timeout(clients, remaining);
            let (guard, wait_status) = match wait_result {
                Ok(result) => result,
                Err(poison) => poison.into_inner(),
            };
            clients = guard;
            if wait_status.timed_out() {
                return PollResult::Empty;
            }
        }
    }

    fn stop_hosted_server(shared: &Arc<HostedServerShared>, reason: &str) {
        if shared.stop_flag.swap(true, Ordering::SeqCst) {
            return;
        }

        let mut local_events = Vec::new();
        {
            let mut clients = lock_clients(shared);
            for client in clients.values_mut() {
                if client.kicked_reason.is_none() {
                    client.kicked_reason = Some(reason.to_string());
                }
                client.connected = false;
                if let Some(sender) = client.local_sender.clone() {
                    local_events.push((sender, client.kicked_reason.clone()));
                }
            }
        }
        shared.condvar.notify_all();

        for (sender, reason) in local_events {
            let _ = sender.send(ClientEvent::Kicked(reason));
        }
    }

    fn collect_registry_functions(lua: &Lua, keys: &[RegistryKey]) -> mlua::Result<Vec<Function>> {
        let mut functions = Vec::with_capacity(keys.len());
        for key in keys {
            functions.push(lua.registry_value::<Function>(key)?);
        }
        Ok(functions)
    }

    fn poll_http_callbacks(lua: &Lua) {
        let globals = lua.globals();
        let http = match globals.get::<Table>("http") {
            Ok(table) => table,
            Err(_) => return,
        };
        let poll = match http.get::<Function>("_poll") {
            Ok(function) => function,
            Err(_) => return,
        };
        if let Err(error) = protect_lua_call("polling server-side HTTP callbacks", || {
            poll.call::<()>(())
        }) {
            eprintln!(
                "\x1b[31mLua Error:\x1b[0m Failed to poll server-side HTTP callbacks\n{}",
                describe_lua_error(&error)
            );
        }
    }

    fn add_server_sandbox_module(
        lua: &Lua,
        shared: Arc<HostedServerShared>,
    ) -> mlua::Result<Rc<RefCell<Vec<RegistryKey>>>> {
        let callbacks = Rc::new(RefCell::new(Vec::<RegistryKey>::new()));
        let module = lua.create_table()?;

        let add_callbacks = callbacks.clone();
        let add_callback = lua.create_function(move |lua, args: MultiValue| {
            let callback = extract_last_function(args)?;
            add_callbacks
                .borrow_mut()
                .push(lua.create_registry_value(callback)?);
            Ok(())
        })?;
        module.set("addcallback", add_callback.clone())?;
        module.set("addCallback", add_callback)?;

        let send_shared = shared.clone();
        module.set(
            "send",
            lua.create_function(move |_lua, (client_key, payload): (String, Buffer)| {
                send_to_client(&send_shared, &client_key, payload.to_vec())
                    .map_err(mlua::Error::external)
            })?,
        )?;

        let kick_shared = shared.clone();
        module.set(
            "kick",
            lua.create_function(move |_lua, (client_key, reason): (String, Option<String>)| {
                kick_client(&kick_shared, &client_key, reason).map_err(mlua::Error::external)
            })?,
        )?;

        let host_shared = shared.clone();
        module.set(
            "isHost",
            lua.create_function(move |_lua, client_key: String| {
                Ok(client_is_host(&host_shared, &client_key))
            })?,
        )?;

        let tags_shared = shared.clone();
        module.set(
            "getClientTags",
            lua.create_function(move |lua, client_key: String| {
                let tags = client_tags(&tags_shared, &client_key).unwrap_or_default();
                let table = lua.create_table()?;
                for tag in tags {
                    table.push(tag)?;
                }
                Ok(table)
            })?,
        )?;

        let host_key = shared.host_client_key.clone();
        module.set(
            "getHostClientKey",
            lua.create_function(move |_lua, ()| Ok(host_key.clone()))?,
        )?;

        install_common_helpers(lua, &module)?;
        lua.globals().set("server", module)?;
        Ok(callbacks)
    }

    fn run_server_runtime(
        shared: Arc<HostedServerShared>,
        env_root: PathBuf,
        script_path: PathBuf,
        inbound_receiver: Receiver<ServerInboundEvent>,
        startup_sender: Sender<Result<(), String>>,
    ) {
        let runtime = (|| -> mlua::Result<()> {
            let lua = Lua::new();
            lua.set_compiler(
                Compiler::new()
                    .set_optimization_level(2)
                    .set_debug_level(1)
                    .set_type_info_level(1),
            );

            let require = lua.create_require_function(TextRequirer::new())?;
            lua.globals().set("require", require)?;

            crate::fs_module::add_fs_module(&lua, env_root.clone())?;
            crate::http::add_http_module(&lua)?;
            let http: Table = lua.globals().get("http")?;
            lua.globals().set("https", http)?;

            crate::commands::add_commands_module(&lua, env_root)?;
            let commands: Table = lua.globals().get("commands")?;
            lua.globals().set("cli", commands)?;

            let callbacks = add_server_sandbox_module(&lua, shared.clone())?;
            let source = fs::read_to_string(&script_path).map_err(mlua::Error::external)?;
            lua.load(source.as_str())
                .set_name(format!("@{}", script_path.display()))
                .exec()?;

            let _ = startup_sender.send(Ok(()));

            loop {
                poll_http_callbacks(&lua);

                if shared.stop_flag.load(Ordering::SeqCst) {
                    break;
                }

                match inbound_receiver.recv_timeout(Duration::from_millis(25)) {
                    Ok(event) => {
                        let functions = {
                            let callbacks = callbacks.borrow();
                            collect_registry_functions(&lua, callbacks.as_slice())?
                        };
                        let payload = lua.create_buffer(event.payload)?;
                        for callback in functions {
                            if let Err(error) = protect_lua_call(
                                "running server callback",
                                || callback.call::<()>((event.client_key.clone(), payload.clone())),
                            ) {
                                eprintln!(
                                    "\x1b[31mLua Error in server callback:\x1b[0m\n{}",
                                    describe_lua_error(&error)
                                );
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            Ok(())
        })();

        if let Err(error) = runtime {
            let display = describe_lua_error(&error);
            let _ = startup_sender.send(Err(display.clone()));
            eprintln!("\x1b[31mLua Error in hosted server:\x1b[0m\n{display}");
        }

        stop_hosted_server(&shared, "server stopped");
    }

    fn parse_timeout_ms(query: Option<&str>) -> u64 {
        let Some(query) = query else {
            return REMOTE_POLL_TIMEOUT_MS;
        };
        for segment in query.split('&') {
            let Some((key, value)) = segment.split_once('=') else {
                continue;
            };
            if key == "timeout_ms" {
                if let Ok(timeout) = value.parse::<u64>() {
                    return timeout.clamp(1, 25_000);
                }
            }
        }
        REMOTE_POLL_TIMEOUT_MS
    }

    fn client_key_from_headers(headers: &HashMap<String, String>) -> Option<String> {
        headers.get("x-neolove-client-key").cloned()
    }

    fn handle_http_request(shared: Arc<HostedServerShared>, request: HttpRequest) -> Result<HttpResponse, String> {
        if request.method.eq_ignore_ascii_case("OPTIONS") {
            return Ok(empty_response(204, "No Content"));
        }

        match request.path.as_str() {
            "/" => {
                let response = StatusResponse {
                    ok: true,
                    transport: "neolove-http",
                    status: "running",
                };
                json_response(200, "OK", &response)
            }
            "/connect" if request.method.eq_ignore_ascii_case("POST")
                || request.method.eq_ignore_ascii_case("GET") =>
            {
                if shared.stop_flag.load(Ordering::SeqCst) {
                    return Ok(plain_response(503, "Service Unavailable", "server is stopping"));
                }
                json_response(200, "OK", &register_remote_client(&shared))
            }
            "/send" if request.method.eq_ignore_ascii_case("POST") => {
                let Some(client_key) = client_key_from_headers(&request.headers) else {
                    return Ok(plain_response(400, "Bad Request", "missing client key"));
                };
                if shared.stop_flag.load(Ordering::SeqCst) {
                    return Ok(plain_response(410, "Gone", "server is stopping"));
                }
                if !client_is_active(&shared, &client_key) {
                    return Ok(plain_response(410, "Gone", "client is disconnected"));
                }
                shared
                    .inbound_sender
                    .send(ServerInboundEvent {
                        client_key,
                        payload: request.body,
                    })
                    .map_err(|_| "server runtime is unavailable".to_string())?;
                Ok(empty_response(202, "Accepted"))
            }
            "/poll" if request.method.eq_ignore_ascii_case("GET") => {
                let Some(client_key) = client_key_from_headers(&request.headers) else {
                    return Ok(plain_response(400, "Bad Request", "missing client key"));
                };
                let timeout =
                    Duration::from_millis(parse_timeout_ms(request.query.as_deref()));
                match wait_for_client_event(&shared, &client_key, timeout) {
                    PollResult::Message(payload) => Ok(HttpResponse {
                        status: 200,
                        reason: "OK",
                        content_type: "application/octet-stream",
                        headers: vec![(
                            "X-NeoLOVE-Event".to_string(),
                            "data".to_string(),
                        )],
                        body: payload,
                    }),
                    PollResult::Empty => Ok(empty_response(204, "No Content")),
                    PollResult::Kicked(reason) => {
                        let mut response = plain_response(410, "Gone", "client was kicked");
                        if let Some(reason) = reason {
                            response.headers.push((
                                "X-NeoLOVE-Kick-Reason".to_string(),
                                reason,
                            ));
                        }
                        Ok(response)
                    }
                    PollResult::UnknownClient => {
                        Ok(plain_response(403, "Forbidden", "unknown client key"))
                    }
                }
            }
            "/disconnect" if request.method.eq_ignore_ascii_case("POST") => {
                let Some(client_key) = client_key_from_headers(&request.headers) else {
                    return Ok(plain_response(400, "Bad Request", "missing client key"));
                };
                disconnect_client(&shared, &client_key, Some("client disconnected".to_string()));
                Ok(empty_response(204, "No Content"))
            }
            _ => Ok(plain_response(404, "Not Found", "unknown route")),
        }
    }

    fn handle_connection<T: Read + Write>(
        stream: &mut T,
        shared: Arc<HostedServerShared>,
    ) -> Result<(), String> {
        let request = read_http_request(stream)?;
        let response = handle_http_request(shared, request)?;
        write_http_response(stream, response)
    }

    fn load_cert_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
        let file = File::open(path)
            .map_err(|error| format!("failed to open certificate '{}': {error}", path.display()))?;
        let mut reader = BufReader::new(file);
        let certs = rustls_pemfile::certs(&mut reader)
            .map_err(|error| format!("failed to read certificate '{}': {error}", path.display()))?;
        if certs.is_empty() {
            return Err(format!("no certificates found in '{}'", path.display()));
        }
        Ok(certs.into_iter().map(CertificateDer::from).collect())
    }

    fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, String> {
        let file = File::open(path)
            .map_err(|error| format!("failed to open private key '{}': {error}", path.display()))?;
        let mut reader = BufReader::new(file);
        if let Some(key) = rustls_pemfile::pkcs8_private_keys(&mut reader)
            .map_err(|error| format!("failed to read private key '{}': {error}", path.display()))?
            .into_iter()
            .next()
        {
            return Ok(PrivateKeyDer::Pkcs8(key.into()));
        }

        let file = File::open(path)
            .map_err(|error| format!("failed to reopen private key '{}': {error}", path.display()))?;
        let mut reader = BufReader::new(file);
        if let Some(key) = rustls_pemfile::rsa_private_keys(&mut reader)
            .map_err(|error| format!("failed to read private key '{}': {error}", path.display()))?
            .into_iter()
            .next()
        {
            return Ok(PrivateKeyDer::Pkcs1(key.into()));
        }

        let file = File::open(path)
            .map_err(|error| format!("failed to reopen private key '{}': {error}", path.display()))?;
        let mut reader = BufReader::new(file);
        if let Some(key) = rustls_pemfile::ec_private_keys(&mut reader)
            .map_err(|error| format!("failed to read private key '{}': {error}", path.display()))?
            .into_iter()
            .next()
        {
            return Ok(PrivateKeyDer::Sec1(key.into()));
        }

        Err(format!("no supported private key found in '{}'", path.display()))
    }

    fn build_server_tls_config(
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<Arc<ServerConfig>, String> {
        let certs = load_cert_chain(cert_path)?;
        let key = load_private_key(key_path)?;
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|error| format!("failed to create TLS config: {error}"))?;
        Ok(Arc::new(config))
    }

    fn accept_server_connections(
        listener: TcpListener,
        tls_config: Option<Arc<ServerConfig>>,
        shared: Arc<HostedServerShared>,
    ) {
        while !shared.stop_flag.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    let shared = shared.clone();
                    let tls_config = tls_config.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<(), String> {
                            configure_stream(&stream)?;
                            match tls_config {
                                Some(config) => {
                                    let connection = ServerConnection::new(config)
                                        .map_err(|error| format!("failed to start TLS session: {error}"))?;
                                    let mut stream = StreamOwned::new(connection, stream);
                                    handle_connection(&mut stream, shared)
                                }
                                None => {
                                    let mut stream = stream;
                                    handle_connection(&mut stream, shared)
                                }
                            }
                        })();

                        if let Err(error) = result {
                            eprintln!("server connection error: {error}");
                        }
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    eprintln!("server accept error: {error}");
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    fn connect_remote_client(base_url: &str) -> Result<ConnectResponse, String> {
        let response = perform_binary_http_request(&join_url(base_url, "/connect"), "POST", &[], &[])?;
        if response.status != 200 {
            let body = String::from_utf8_lossy(&response.body);
            return Err(format!(
                "server rejected connect request with status {}: {}",
                response.status, body
            ));
        }
        let payload: ConnectResponse =
            serde_json::from_slice(&response.body).map_err(|error| format!("invalid connect response: {error}"))?;
        if !payload.ok {
            return Err(payload.error.unwrap_or_else(|| "server rejected connect request".to_string()));
        }
        Ok(payload)
    }

    fn send_remote_payload(base_url: &str, client_key: &str, payload: &[u8]) -> Result<(), String> {
        let headers = vec![(
            "X-NeoLOVE-Client-Key".to_string(),
            client_key.to_string(),
        )];
        let response =
            perform_binary_http_request(&join_url(base_url, "/send"), "POST", &headers, payload)?;
        if response.status != 202 {
            let body = String::from_utf8_lossy(&response.body);
            return Err(format!(
                "server rejected payload with status {}: {}",
                response.status, body
            ));
        }
        Ok(())
    }

    fn disconnect_remote_client(base_url: &str, client_key: &str) -> Result<(), String> {
        let headers = vec![(
            "X-NeoLOVE-Client-Key".to_string(),
            client_key.to_string(),
        )];
        let response =
            perform_binary_http_request(&join_url(base_url, "/disconnect"), "POST", &headers, &[])?;
        if response.status != 204 {
            let body = String::from_utf8_lossy(&response.body);
            return Err(format!(
                "server rejected disconnect with status {}: {}",
                response.status, body
            ));
        }
        Ok(())
    }

    fn poll_remote_client(
        base_url: &str,
        client_key: &str,
        timeout_ms: u64,
    ) -> Result<RemotePollResult, String> {
        let headers = vec![(
            "X-NeoLOVE-Client-Key".to_string(),
            client_key.to_string(),
        )];
        let url = format!(
            "{}?timeout_ms={timeout_ms}",
            join_url(base_url, "/poll")
        );
        let response = perform_binary_http_request(&url, "GET", &headers, &[])?;
        match response.status {
            200 => Ok(RemotePollResult::Payload(response.body)),
            204 => Ok(RemotePollResult::Empty),
            410 => Ok(RemotePollResult::Kicked(find_header_case_insensitive(
                &response.headers,
                "X-NeoLOVE-Kick-Reason",
            ))),
            403 => Err("server rejected client key".to_string()),
            status => Err(format!("unexpected poll response status: {status}")),
        }
    }

    fn spawn_remote_client_threads(
        base_url: String,
        client_key: String,
        event_sender: Sender<ClientEvent>,
        outbound_receiver: Receiver<Vec<u8>>,
        stop_flag: Arc<AtomicBool>,
    ) {
        let poll_sender = event_sender.clone();
        let poll_base_url = base_url.clone();
        let poll_client_key = client_key.clone();
        let poll_stop = stop_flag.clone();
        thread::spawn(move || {
            while !poll_stop.load(Ordering::SeqCst) {
                match poll_remote_client(&poll_base_url, &poll_client_key, REMOTE_POLL_TIMEOUT_MS) {
                    Ok(RemotePollResult::Payload(payload)) => {
                        if poll_sender.send(ClientEvent::Payload(payload)).is_err() {
                            break;
                        }
                    }
                    Ok(RemotePollResult::Empty) => {}
                    Ok(RemotePollResult::Kicked(reason)) => {
                        poll_stop.store(true, Ordering::SeqCst);
                        let _ = poll_sender.send(ClientEvent::Kicked(reason));
                        break;
                    }
                    Err(error) => {
                        poll_stop.store(true, Ordering::SeqCst);
                        let _ = poll_sender.send(ClientEvent::Closed(Some(error)));
                        break;
                    }
                }
            }
        });

        thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                match outbound_receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(payload) => {
                        if let Err(error) = send_remote_payload(&base_url, &client_key, &payload) {
                            stop_flag.store(true, Ordering::SeqCst);
                            let _ = event_sender.send(ClientEvent::Closed(Some(error)));
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    }

    fn create_client_handle(
        lua: &Lua,
        state: Rc<RefCell<ServersState>>,
        client_id: u64,
    ) -> mlua::Result<Table> {
        let client = {
            let state = state.borrow();
            state
                .clients
                .get(&client_id)
                .ok_or_else(|| mlua::Error::external("client handle disappeared"))?
                .key
                .clone()
        };
        let is_host = {
            let state = state.borrow();
            state
                .clients
                .get(&client_id)
                .map(|client| client.is_host)
                .unwrap_or(false)
        };

        let table = lua.create_table()?;
        table.set("key", client)?;
        table.set("is_host", is_host)?;

        let send_state = state.clone();
        table.set(
            "send",
            lua.create_function(move |_lua, args: MultiValue| {
                let payload = extract_last_buffer(args)?;
                let bytes = payload.to_vec();

                let (transport, client_key, connected) = {
                    let state = send_state.borrow();
                    let client = state
                        .clients
                        .get(&client_id)
                        .ok_or_else(|| mlua::Error::external("client handle disappeared"))?;
                    (client.transport.clone(), client.key.clone(), client.connected)
                };

                if !connected {
                    return Ok(false);
                }

                let sent = match transport {
                    ClientTransport::Local { shared } => {
                        if !client_is_active(&shared, &client_key) {
                            false
                        } else {
                            shared
                                .inbound_sender
                                .send(ServerInboundEvent {
                                    client_key,
                                    payload: bytes,
                                })
                                .is_ok()
                        }
                    }
                    ClientTransport::Remote {
                        outbound_sender,
                        stop_flag,
                        ..
                    } => {
                        if stop_flag.load(Ordering::SeqCst) {
                            false
                        } else {
                            outbound_sender.send(bytes).is_ok()
                        }
                    }
                };

                Ok(sent)
            })?,
        )?;

        let add_state = state.clone();
        let add_callback = lua.create_function(move |lua, args: MultiValue| {
            let callback = extract_last_function(args)?;
            let key = lua.create_registry_value(callback)?;
            let mut state = add_state.borrow_mut();
            let client = state
                .clients
                .get_mut(&client_id)
                .ok_or_else(|| mlua::Error::external("client handle disappeared"))?;
            client.callbacks.push(key);
            Ok(())
        })?;
        table.set("addcallback", add_callback.clone())?;
        table.set("addCallback", add_callback)?;

        let disconnect_state = state.clone();
        table.set(
            "disconnect",
            lua.create_function(move |_lua, _args: MultiValue| {
                let (transport, client_key, was_connected) = {
                    let mut state = disconnect_state.borrow_mut();
                    let client = state
                        .clients
                        .get_mut(&client_id)
                        .ok_or_else(|| mlua::Error::external("client handle disappeared"))?;
                    let was_connected = client.connected;
                    client.connected = false;
                    (client.transport.clone(), client.key.clone(), was_connected)
                };

                if !was_connected {
                    return Ok(false);
                }

                match transport {
                    ClientTransport::Local { shared } => {
                        disconnect_client(&shared, &client_key, Some("client disconnected".to_string()));
                    }
                    ClientTransport::Remote {
                        base_url,
                        stop_flag,
                        ..
                    } => {
                        stop_flag.store(true, Ordering::SeqCst);
                        thread::spawn(move || {
                            let _ = disconnect_remote_client(&base_url, &client_key);
                        });
                    }
                }

                Ok(true)
            })?,
        )?;

        let connected_state = state.clone();
        table.set(
            "isConnected",
            lua.create_function(move |_lua, _args: MultiValue| {
                Ok(connected_state
                    .borrow()
                    .clients
                    .get(&client_id)
                    .map(|client| client.connected)
                    .unwrap_or(false))
            })?,
        )?;

        let key_state = state.clone();
        table.set(
            "getKey",
            lua.create_function(move |_lua, _args: MultiValue| {
                Ok(key_state
                    .borrow()
                    .clients
                    .get(&client_id)
                    .map(|client| client.key.clone())
                    .unwrap_or_default())
            })?,
        )?;

        let host_state = state.clone();
        table.set(
            "isHost",
            lua.create_function(move |_lua, _args: MultiValue| {
                Ok(host_state
                    .borrow()
                    .clients
                    .get(&client_id)
                    .map(|client| client.is_host)
                    .unwrap_or(false))
            })?,
        )?;

        let kick_state = state;
        table.set(
            "getKickReason",
            lua.create_function(move |_lua, _args: MultiValue| {
                Ok(kick_state
                    .borrow()
                    .clients
                    .get(&client_id)
                    .and_then(|client| client.kick_reason.clone()))
            })?,
        )?;

        Ok(table)
    }

    fn create_hosted_handle(
        lua: &Lua,
        state: Rc<RefCell<ServersState>>,
        hosted_id: u64,
        client: Table,
        port: u16,
        url: String,
    ) -> mlua::Result<Table> {
        let table = lua.create_table()?;
        table.set("client", client)?;
        table.set("port", port)?;
        table.set("url", url.clone())?;

        let stop_state = state.clone();
        table.set(
            "stop",
            lua.create_function(move |_lua, _args: MultiValue| {
                let hosted = stop_state.borrow_mut().hosted.remove(&hosted_id);
                if let Some(hosted) = hosted {
                    stop_hosted_server(&hosted.shared, "server stopped");
                    Ok(true)
                } else {
                    Ok(false)
                }
            })?,
        )?;

        table.set(
            "getPort",
            lua.create_function(move |_lua, _args: MultiValue| Ok(port))?,
        )?;

        table.set(
            "getUrl",
            lua.create_function(move |_lua, _args: MultiValue| Ok(url.clone()))?,
        )?;

        Ok(table)
    }

    pub(crate) fn add_servers_module(lua: &Lua, env_root: PathBuf) -> mlua::Result<()> {
        let state = Rc::new(RefCell::new(ServersState {
            next_client_id: 1,
            next_hosted_id: 1,
            clients: HashMap::new(),
            hosted: HashMap::new(),
        }));

        let module = lua.create_table()?;

        let host_root = env_root.clone();
        let host_state = state.clone();
        module.set(
            "host",
            lua.create_function(move |lua, (script_path, port, options): (String, u16, Option<Table>)| {
                let script_path =
                    canonicalize_project_path(&host_root, &script_path).map_err(mlua::Error::external)?;
                let bind_host = match &options {
                    Some(options) => get_option_string(options, &["host"])?
                        .unwrap_or_else(|| "127.0.0.1".to_string()),
                    None => "127.0.0.1".to_string(),
                };

                let cert_path = match &options {
                    Some(options) => get_option_string(options, &["certPath", "cert_path"])?,
                    None => None,
                };
                let key_path = match &options {
                    Some(options) => get_option_string(options, &["keyPath", "key_path"])?,
                    None => None,
                };

                let tls_config = match (cert_path, key_path) {
                    (Some(cert_path), Some(key_path)) => {
                        let cert_path = canonicalize_project_path(&host_root, &cert_path)
                            .map_err(mlua::Error::external)?;
                        let key_path = canonicalize_project_path(&host_root, &key_path)
                            .map_err(mlua::Error::external)?;
                        Some(
                            build_server_tls_config(&cert_path, &key_path)
                                .map_err(mlua::Error::external)?,
                        )
                    }
                    (None, None) => None,
                    _ => {
                        return Err(mlua::Error::external(
                            "certPath and keyPath must either both be set or both be omitted",
                        ))
                    }
                };

                let listener = TcpListener::bind(format_socket_addr(&bind_host, port))
                    .map_err(mlua::Error::external)?;
                listener
                    .set_nonblocking(true)
                    .map_err(mlua::Error::external)?;
                let actual_port = listener.local_addr().map_err(mlua::Error::external)?.port();
                let url = format_public_url(
                    if tls_config.is_some() {
                        HttpScheme::Https
                    } else {
                        HttpScheme::Http
                    },
                    &bind_host,
                    actual_port,
                );

                let (inbound_sender, inbound_receiver) = mpsc::channel::<ServerInboundEvent>();
                let host_client_key = generate_client_key();
                let (host_event_sender, host_event_receiver) = mpsc::channel::<ClientEvent>();
                let mut clients = HashMap::new();
                clients.insert(
                    host_client_key.clone(),
                    new_runtime_client_state(true, Some(host_event_sender)),
                );

                let shared = Arc::new(HostedServerShared {
                    clients: Mutex::new(clients),
                    condvar: Condvar::new(),
                    stop_flag: AtomicBool::new(false),
                    host_client_key: host_client_key.clone(),
                    inbound_sender: inbound_sender.clone(),
                });

                let (startup_sender, startup_receiver) = mpsc::channel::<Result<(), String>>();
                let runtime_shared = shared.clone();
                let runtime_root = host_root.clone();
                let runtime_script_path = script_path.clone();
                thread::spawn(move || {
                    run_server_runtime(
                        runtime_shared,
                        runtime_root,
                        runtime_script_path,
                        inbound_receiver,
                        startup_sender,
                    );
                });

                match startup_receiver.recv_timeout(Duration::from_secs(5)) {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        stop_hosted_server(&shared, "server failed to start");
                        return Err(mlua::Error::external(error));
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        stop_hosted_server(&shared, "server failed to start");
                        return Err(mlua::Error::external(
                            "timed out waiting for hosted server startup",
                        ));
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        stop_hosted_server(&shared, "server failed to start");
                        return Err(mlua::Error::external(
                            "hosted server shut down before confirming startup",
                        ));
                    }
                }

                let accept_shared = shared.clone();
                thread::spawn(move || accept_server_connections(listener, tls_config, accept_shared));

                let (client_id, hosted_id) = {
                    let mut state = host_state.borrow_mut();
                    let client_id = state.next_client_id;
                    state.next_client_id = state.next_client_id.saturating_add(1);
                    state.clients.insert(
                        client_id,
                        ClientHandleState {
                            key: host_client_key,
                            is_host: true,
                            connected: true,
                            kick_reason: None,
                            callbacks: Vec::new(),
                            receiver: host_event_receiver,
                            transport: ClientTransport::Local {
                                shared: shared.clone(),
                            },
                        },
                    );

                    let hosted_id = state.next_hosted_id;
                    state.next_hosted_id = state.next_hosted_id.saturating_add(1);
                    state.hosted.insert(
                        hosted_id,
                        HostedHandleState {
                            shared,
                        },
                    );
                    (client_id, hosted_id)
                };

                let client = create_client_handle(lua, host_state.clone(), client_id)?;
                create_hosted_handle(lua, host_state.clone(), hosted_id, client, actual_port, url)
            })?,
        )?;

        let connect_state = state.clone();
        module.set(
            "connect",
            lua.create_function(move |lua, url: String| {
                let base_url = normalize_base_url(&url);
                let response =
                    connect_remote_client(&base_url).map_err(mlua::Error::external)?;

                let (event_sender, event_receiver) = mpsc::channel::<ClientEvent>();
                let (outbound_sender, outbound_receiver) = mpsc::channel::<Vec<u8>>();
                let stop_flag = Arc::new(AtomicBool::new(false));
                spawn_remote_client_threads(
                    base_url.clone(),
                    response.client_key.clone(),
                    event_sender,
                    outbound_receiver,
                    stop_flag.clone(),
                );

                let client_id = {
                    let mut state = connect_state.borrow_mut();
                    let client_id = state.next_client_id;
                    state.next_client_id = state.next_client_id.saturating_add(1);
                    state.clients.insert(
                        client_id,
                        ClientHandleState {
                            key: response.client_key,
                            is_host: response.is_host,
                            connected: true,
                            kick_reason: None,
                            callbacks: Vec::new(),
                            receiver: event_receiver,
                            transport: ClientTransport::Remote {
                                base_url,
                                outbound_sender,
                                stop_flag,
                            },
                        },
                    );
                    client_id
                };

                create_client_handle(lua, connect_state.clone(), client_id)
            })?,
        )?;

        let poll_state = state;
        module.set(
            "_poll",
            lua.create_function(move |lua, ()| {
                let client_ids: Vec<u64> = poll_state.borrow().clients.keys().copied().collect();

                for client_id in client_ids {
                    loop {
                        let next_event = {
                            let mut state = poll_state.borrow_mut();
                            let Some(client) = state.clients.get_mut(&client_id) else {
                                break;
                            };

                            match client.receiver.try_recv() {
                                Ok(event) => {
                                    match &event {
                                        ClientEvent::Kicked(reason) | ClientEvent::Closed(reason) => {
                                            client.connected = false;
                                            if client.kick_reason.is_none() {
                                                client.kick_reason = reason.clone();
                                            }
                                        }
                                        ClientEvent::Payload(_) => {}
                                    }

                                    let callbacks = collect_registry_functions(lua, client.callbacks.as_slice())?;
                                    Some((event, callbacks))
                                }
                                Err(TryRecvError::Empty) => None,
                                Err(TryRecvError::Disconnected) => {
                                    client.connected = false;
                                    None
                                }
                            }
                        };

                        let Some((event, callbacks)) = next_event else {
                            break;
                        };

                        if let ClientEvent::Payload(payload) = event {
                            let payload = lua.create_buffer(payload)?;
                            for callback in callbacks {
                                if let Err(error) = protect_lua_call(
                                    "running server client callback",
                                    || callback.call::<()>(payload.clone()),
                                ) {
                                    eprintln!(
                                        "\x1b[31mLua Error in server client callback:\x1b[0m\n{}",
                                        describe_lua_error(&error)
                                    );
                                }
                            }
                        }
                    }
                }
                Ok(())
            })?,
        )?;

        install_common_helpers(lua, &module)?;
        lua.globals().set("servers", module)?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn serialize_round_trip_preserves_nested_tables_and_buffers() -> mlua::Result<()> {
            let lua = Lua::new();
            let root = lua.create_table()?;
            root.set("message", "hello")?;
            root.set("count", 3)?;
            root.set("bytes", lua.create_buffer(vec![1, 2, 3, 4])?)?;

            let nested = lua.create_table()?;
            nested.set("ok", true)?;
            nested.push("a")?;
            nested.push("b")?;
            root.set("nested", nested)?;

            let buffer = serialize_table(&lua, root)?;
            let decoded = deserialize_table(&lua, buffer)?;
            assert_eq!(decoded.get::<String>("message")?, "hello");
            assert_eq!(decoded.get::<i64>("count")?, 3);
            let bytes: Buffer = decoded.get("bytes")?;
            assert_eq!(bytes.to_vec(), vec![1, 2, 3, 4]);

            let nested: Table = decoded.get("nested")?;
            assert_eq!(nested.get::<bool>("ok")?, true);
            assert_eq!(nested.get::<String>(1)?, "a");
            assert_eq!(nested.get::<String>(2)?, "b");
            Ok(())
        }

        #[test]
        fn sha128_is_truncated_sha256() {
            let bytes = b"neo-love";
            let full = sha256_hex(bytes);
            let short = sha128_hex(bytes);
            assert_eq!(short.len(), 32);
            assert_eq!(short, full[..32]);
        }

        #[test]
        fn generated_uuids_have_expected_versions() {
            let v4 = Uuid::parse_str(&Uuid::new_v4().to_string()).unwrap();
            let v7 = Uuid::parse_str(&Uuid::now_v7().to_string()).unwrap();
            assert_eq!(v4.get_version_num(), 4);
            assert_eq!(v7.get_version_num(), 7);
        }
    }
}

#[cfg(target_os = "emscripten")]
mod native {
    use mlua::Lua;
    use std::path::PathBuf;

    pub(crate) fn add_servers_module(lua: &Lua, _env_root: PathBuf) -> mlua::Result<()> {
        let module = lua.create_table()?;
        let error = "servers are not available in web builds";
        let unsupported = lua.create_function(move |_lua, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::external(error))
        })?;

        module.set("host", unsupported.clone())?;
        module.set("connect", unsupported.clone())?;
        module.set("serializeTable", unsupported.clone())?;
        module.set("serialize_table", unsupported.clone())?;
        module.set("deserializeTable", unsupported.clone())?;
        module.set("deserialize_table", unsupported.clone())?;
        module.set("generateUuid4", unsupported.clone())?;
        module.set("generate_uuid4", unsupported.clone())?;
        module.set("generateUuid7", unsupported.clone())?;
        module.set("generate_uuid7", unsupported.clone())?;
        module.set("sha256", unsupported.clone())?;
        module.set("sha128", unsupported)?;
        module.set("_poll", lua.create_function(move |_lua, ()| Ok(()))?)?;

        lua.globals().set("servers", module)?;
        Ok(())
    }
}

pub(crate) use native::add_servers_module;
