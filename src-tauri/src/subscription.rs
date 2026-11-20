//! Subscription formats other than "a JSON array of full Xray configs".
//!
//! Providers commonly answer with a base64-wrapped list of `vless://`, `vmess://`,
//! `trojan://` or `ss://` links. Those are converted into one Xray config with a
//! balancer; the app still rewrites `routing.rules` for its whitelist afterwards.

use serde_json::{json, Value};

/// Parses the subscription body into one or more full Xray configs.
pub fn parse_response(text: &str) -> Result<Vec<Value>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Подписка вернула пустой ответ.".into());
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if value.get("proxies").is_some() {
            return Err("Это подписка Clash. Splitline работает с Xray-конфигами \
                 или списком ссылок vless/vmess/trojan/ss."
                .into());
        }
        let list = match value {
            Value::Array(items) => items,
            other => vec![other],
        };
        let usable: Vec<Value> = list
            .into_iter()
            .filter(|item| item.get("outbounds").is_some())
            .collect();
        if usable.is_empty() {
            return Err("В подписке нет ни одного конфига Xray.".into());
        }
        return Ok(usable);
    }

    // A plain list may be wrapped in base64; try that before giving up.
    let decoded = decode_base64(trimmed).unwrap_or_else(|| trimmed.to_string());
    let body = if decoded.contains("://") {
        decoded
    } else {
        trimmed.to_string()
    };
    if !body.contains("://") {
        return Err(format!(
            "Неизвестный формат подписки. Ответ сервера: {}",
            head_of(trimmed)
        ));
    }

    let mut outbounds = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (index, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        match parse_uri(line) {
            Ok(mut outbound) => {
                outbound["tag"] = Value::from(format!("proxy-{}", index + 1));
                outbounds.push(outbound);
            }
            Err(error) => errors.push(error),
        }
    }
    if outbounds.is_empty() {
        let hint = errors
            .first()
            .cloned()
            .unwrap_or_else(|| "ни одной ссылки".into());
        return Err(format!("Не удалось разобрать подписку: {hint}"));
    }

    outbounds.push(json!({ "tag": "direct", "protocol": "freedom" }));
    outbounds.push(json!({ "tag": "block", "protocol": "blackhole" }));
    Ok(vec![json!({
        "outbounds": outbounds,
        "routing": {
            "balancers": [{
                "tag": "auto",
                "selector": ["proxy-"],
                "strategy": { "type": "leastPing" }
            }],
            "rules": []
        }
    })])
}

fn head_of(text: &str) -> String {
    let printable: String = text.chars().filter(|c| !c.is_control()).collect();
    if printable.chars().count() > 200 {
        let head: String = printable.chars().take(200).collect();
        return format!("{head}…");
    }
    printable
}

fn parse_uri(line: &str) -> Result<Value, String> {
    let (scheme, rest) = line
        .split_once("://")
        .ok_or_else(|| format!("не ссылка: {}", head_of(line)))?;
    match scheme.to_ascii_lowercase().as_str() {
        "vless" => parse_vless(rest),
        "vmess" => parse_vmess(rest),
        "trojan" => parse_trojan(rest),
        "ss" => parse_ss(rest),
        other => Err(format!("протокол {other} не поддерживается Xray")),
    }
}

struct Parts {
    user: String,
    host: String,
    port: u16,
    query: Vec<(String, String)>,
}

fn split_parts(rest: &str) -> Result<Parts, String> {
    let before_name = rest.split('#').next().unwrap_or(rest);
    let (authority, query_text) = match before_name.split_once('?') {
        Some((left, right)) => (left, right),
        None => (before_name, ""),
    };
    let query = query_text
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (key.to_ascii_lowercase(), percent_decode(value)),
            None => (pair.to_ascii_lowercase(), String::new()),
        })
        .collect();
    let (user, host_port) = match authority.rsplit_once('@') {
        Some((left, right)) => (left.to_string(), right),
        None => (String::new(), authority),
    };
    let (host, port) = split_host_port(host_port)?;
    Ok(Parts {
        user,
        host,
        port,
        query,
    })
}

fn split_host_port(value: &str) -> Result<(String, u16), String> {
    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| format!("в ссылке нет порта: {value}"))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("неверный порт: {port}"))?;
    if host.is_empty() {
        return Err("в ссылке нет адреса сервера".into());
    }
    Ok((host.to_string(), port))
}

fn param<'a>(parts: &'a Parts, key: &str) -> Option<&'a str> {
    parts
        .query
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

fn stream_settings(parts: &Parts) -> Value {
    let network = param(parts, "type").unwrap_or("tcp");
    let security = param(parts, "security").unwrap_or("none");
    let mut stream = json!({ "network": network });
    let server_name = param(parts, "sni").or_else(|| param(parts, "host"));
    if security == "tls" {
        let mut tls = json!({ "allowInsecure": false });
        if let Some(name) = server_name {
            tls["serverName"] = Value::from(name);
        }
        if let Some(alpn) = param(parts, "alpn") {
            tls["alpn"] = json!(alpn.split(',').collect::<Vec<_>>());
        }
        if let Some(fingerprint) = param(parts, "fp") {
            tls["fingerprint"] = Value::from(fingerprint);
        }
        stream["security"] = Value::from("tls");
        stream["tlsSettings"] = tls;
    } else if security == "reality" {
        let mut reality = json!({
            "show": false,
            "fingerprint": param(parts, "fp").unwrap_or("chrome"),
            "publicKey": param(parts, "pbk").unwrap_or_default(),
            "shortId": param(parts, "sid").unwrap_or(""),
            "serverName": server_name.unwrap_or_default(),
        });
        if let Some(spider) = param(parts, "spx") {
            reality["spiderX"] = Value::from(spider);
        }
        stream["security"] = Value::from("reality");
        stream["realitySettings"] = reality;
    }
    match network {
        "ws" => {
            let mut ws = json!({ "path": param(parts, "path").unwrap_or("/") });
            if let Some(host) = param(parts, "host") {
                ws["headers"] = json!({ "Host": host });
            }
            stream["wsSettings"] = ws;
        }
        "grpc" => {
            stream["grpcSettings"] =
                json!({ "serviceName": param(parts, "serviceName").unwrap_or("") });
        }
        _ => {}
    }
    stream
}

fn parse_vless(rest: &str) -> Result<Value, String> {
    let parts = split_parts(rest)?;
    if parts.user.is_empty() {
        return Err("vless: нет идентификатора".into());
    }
    let mut user = json!({ "id": parts.user, "encryption": "none" });
    if let Some(flow) = param(&parts, "flow") {
        user["flow"] = Value::from(flow);
    }
    Ok(json!({
        "protocol": "vless",
        "settings": { "vnext": [{ "address": parts.host, "port": parts.port, "users": [user] }] },
        "streamSettings": stream_settings(&parts),
    }))
}

fn parse_trojan(rest: &str) -> Result<Value, String> {
    let parts = split_parts(rest)?;
    if parts.user.is_empty() {
        return Err("trojan: нет пароля".into());
    }
    let mut stream = stream_settings(&parts);
    if !stream.get("security").is_some() {
        stream["security"] = Value::from("tls");
    }
    Ok(json!({
        "protocol": "trojan",
        "settings": { "servers": [{ "address": parts.host, "port": parts.port, "password": parts.user }] },
        "streamSettings": stream,
    }))
}

fn parse_ss(rest: &str) -> Result<Value, String> {
    // SIP002 is `ss://base64(method:pass)@host:port`; some providers give the
    // legacy fully base64-encoded `ss://base64(method:pass@host:port)`.
    let without_name = rest.split('#').next().unwrap_or(rest);
    if !without_name.contains('@') {
        if let Some(decoded) = decode_base64(without_name) {
            if let Some((auth, host_port)) = decoded.rsplit_once('@') {
                let (method, password) = auth
                    .split_once(':')
                    .ok_or_else(|| "ss: ожидался формат method:password".to_string())?;
                let (host, port) = split_host_port(host_port)?;
                return Ok(shadowsocks(method, password, &host, port));
            }
        }
    }
    let parts = split_parts(rest)?;
    let decoded = decode_base64(&parts.user)
        .ok_or_else(|| "ss: не удалось прочитать метод и пароль".to_string())?;
    let (method, password) = decoded
        .split_once(':')
        .ok_or_else(|| "ss: ожидался формат method:password".to_string())?;
    Ok(shadowsocks(method, password, &parts.host, parts.port))
}

fn shadowsocks(method: &str, password: &str, host: &str, port: u16) -> Value {
    json!({
        "protocol": "shadowsocks",
        "settings": { "servers": [{
            "address": host,
            "port": port,
            "method": method,
            "password": password
        }] },
    })
}

fn parse_vmess(rest: &str) -> Result<Value, String> {
    let decoded =
        decode_base64(rest).ok_or_else(|| "vmess: ссылка не является base64".to_string())?;
    let value: Value =
        serde_json::from_str(&decoded).map_err(|_| "vmess: внутри ссылки не JSON".to_string())?;
    let host = value.get("add").and_then(Value::as_str).unwrap_or_default();
    let port = value.get("port").and_then(|port| {
        port.as_u64()
            .map(|number| number as u16)
            .or_else(|| port.as_str().and_then(|text| text.parse().ok()))
    });
    let id = value.get("id").and_then(Value::as_str).unwrap_or_default();
    let (Some(port), false) = (port, host.is_empty() || id.is_empty()) else {
        return Err("vmess: в ссылке нет адреса, порта или id".into());
    };
    let network = value.get("net").and_then(Value::as_str).unwrap_or("tcp");
    let security = value.get("tls").and_then(Value::as_str).unwrap_or("");
    let mut stream = json!({ "network": network });
    if !security.is_empty() && security != "none" {
        let mut tls = json!({ "allowInsecure": false });
        if let Some(sni) = value.get("sni").and_then(Value::as_str) {
            tls["serverName"] = Value::from(sni);
        }
        stream["security"] = Value::from("tls");
        stream["tlsSettings"] = tls;
    }
    if network == "ws" {
        stream["wsSettings"] = json!({
            "path": value.get("path").and_then(Value::as_str).unwrap_or("/"),
            "headers": { "Host": value.get("host").and_then(Value::as_str).unwrap_or_default() }
        });
    }
    Ok(json!({
        "protocol": "vmess",
        "settings": { "vnext": [{
            "address": host,
            "port": port,
            "users": [{
                "id": id,
                "alterId": value.get("aid").and_then(Value::as_u64).unwrap_or(0),
                "security": value.get("scy").and_then(Value::as_str).unwrap_or("auto")
            }]
        }] },
        "streamSettings": stream,
    }))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                out.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Standard or URL-safe base64, padding optional.
fn decode_base64(input: &str) -> Option<String> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return None;
    }
    let mut bytes = Vec::with_capacity(cleaned.len());
    for (index, ch) in cleaned.bytes().enumerate() {
        let value = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        bytes.push((index, value));
    }
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for (_, value) in bytes {
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_padded_and_urlsafe_base64() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), "hello");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), "hello");
    }

    #[test]
    fn parses_vless_reality_into_one_config() {
        let link = "vless://11111111-1111-1111-1111-111111111111@example.com:443\
            ?type=tcp&security=reality&sni=cdn.example.com&fp=chrome&pbk=KEY&sid=ab#Германия";
        let configs = parse_response(link).expect("parsed");
        assert_eq!(configs.len(), 1);
        let outbounds = configs[0]["outbounds"].as_array().unwrap();
        assert_eq!(outbounds[0]["protocol"], "vless");
        assert_eq!(outbounds[0]["streamSettings"]["security"], "reality");
    }

    #[test]
    fn parses_trojan_and_shadowsocks() {
        let trojan = "trojan://pass@example.com:443?sni=example.com#DE";
        let ss = "ss://YWVzLTI1Ni1nY206cGFzcw==@example.com:8388#SS";
        let configs = parse_response(&format!("{trojan}\n{ss}")).expect("parsed");
        let outbounds = configs[0]["outbounds"].as_array().unwrap();
        assert_eq!(outbounds[0]["protocol"], "trojan");
        assert_eq!(outbounds[1]["protocol"], "shadowsocks");
    }

    #[test]
    fn clash_subscription_gets_a_readable_error() {
        let error = parse_response("{\"proxies\":[]}").unwrap_err();
        assert!(error.contains("Clash"));
    }

    #[test]
    fn base64_list_is_detected() {
        let lists =
            "dmxlc3M6Ly8xMTExMTExMS0xMTExLTExMTEtMTExMS0xMTExMTExMTExMTFAZXhhbXBsZS5jb206NDQzI0RF";
        let configs = parse_response(lists).expect("parsed");
        assert_eq!(configs[0]["outbounds"][0]["protocol"], "vless");
    }
}
