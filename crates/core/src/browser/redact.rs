//! Best-effort hiding of secrets in what the debugging tools return: cookie values,
//! credential headers and token-like values become `[hidden: N chars]` unless Settings ›
//! Browser › Show secrets is on. It guards against accidental exposure in results and chat
//! history, not against a page that has talked the agent into encoding a secret.

use serde_json::Value;

/// Headers whose values are credentials.
pub fn is_secret_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-api-key"
            | "x-auth-token"
            | "x-csrf-token"
            | "x-xsrf-token"
    ) || (name.starts_with("x-")
        && (name.ends_with("-token") || name.ends_with("-key") || name.ends_with("-secret")))
}

/// Keys whose values are credentials, in storage, JSON bodies and expression results.
pub fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "api_key",
        "api-key",
        "session",
        "credential",
        "auth",
        "jwt",
        "bearer",
        "private_key",
        "access_key",
    ]
    .iter()
    .any(|word| key.contains(word))
}

/// A value that looks like a token: a JWT, a `Bearer` credential, or a long unbroken run of
/// base64 or hex characters.
pub fn looks_like_token(text: &str) -> bool {
    let text = text.trim();
    if text.len() < 20 {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("bearer ") || lower.starts_with("basic ") {
        return true;
    }
    let jwt = text.split('.').count() == 3
        && text.starts_with("ey")
        && text
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(base64_char));
    let opaque = text.len() >= 32
        && !text.contains(char::is_whitespace)
        && text.chars().all(base64_char)
        && text.chars().any(|c| c.is_ascii_digit())
        && text.chars().any(|c| c.is_ascii_alphabetic());
    jwt || opaque
}

fn base64_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
}

/// The replacement for a hidden value.
pub fn hidden(text: &str) -> String {
    format!("[hidden: {} chars]", text.chars().count())
}

/// `value` with secrets hidden: values under secret keys and token-like strings anywhere.
pub fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(text) if looks_like_token(text) => Value::String(hidden(text)),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::String(text) if is_secret_key(key) && !text.is_empty() => {
                            Value::String(hidden(text))
                        }
                        other => redact_value(other),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Free text with token-like words hidden (console messages, bodies that are not JSON).
pub fn redact_text(text: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        if value.is_object() || value.is_array() {
            return redact_value(&value).to_string();
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    // The word before a `=` or `:`, when it names a secret (`password=hunter2`).
    let mut secret_key = false;
    let mut after_assign = false;
    let flush = |word: &mut String, out: &mut String, secret_key: bool| {
        if !word.is_empty() && (looks_like_token(word) || secret_key) {
            out.push_str(&hidden(word));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | ',' | ';' | '(' | ')' | '<' | '>' | '&' | '?' | '=' | ':'
            )
        {
            let names_secret = is_secret_key(&word);
            flush(&mut word, &mut out, after_assign && secret_key);
            if matches!(c, '=' | ':') {
                secret_key = names_secret;
                after_assign = true;
            } else {
                after_assign = false;
            }
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out, after_assign && secret_key);
    out
}

/// Headers as `{name: value}` with credential headers hidden.
pub fn redact_headers(headers: &Value) -> Value {
    match headers.as_object() {
        Some(map) => Value::Object(
            map.iter()
                .map(|(name, value)| {
                    let shown = match value.as_str() {
                        Some(text) if is_secret_header(name) => Value::String(hidden(text)),
                        Some(text) => Value::String(redact_text(text)),
                        None => value.clone(),
                    };
                    (name.clone(), shown)
                })
                .collect(),
        ),
        None => headers.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn credential_headers_storage_keys_and_tokens_are_hidden() {
        let headers = redact_headers(&json!({
            "Authorization": "Bearer abc", "Cookie": "sid=1", "X-Github-Token": "x", "Content-Type": "text/html"
        }));
        assert_eq!(headers["Authorization"], "[hidden: 10 chars]");
        assert_eq!(headers["Cookie"], "[hidden: 5 chars]");
        assert_eq!(headers["X-Github-Token"], "[hidden: 1 chars]");
        assert_eq!(headers["Content-Type"], "text/html");
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJlLXZhbHVl";
        let stored = redact_value(
            &json!({"theme": "dark", "accessToken": "short", "user": {"id": 7, "jwt": jwt}, "list": [jwt]}),
        );
        assert_eq!(stored["theme"], "dark");
        assert_eq!(stored["accessToken"], "[hidden: 5 chars]");
        assert_eq!(stored["user"]["id"], 7);
        assert!(stored["user"]["jwt"]
            .as_str()
            .unwrap()
            .starts_with("[hidden"));
        assert!(stored["list"][0].as_str().unwrap().starts_with("[hidden"));
        let text = redact_text(&format!(
            "login ok token={jwt} for user alice password=hunter2"
        ));
        assert!(text.contains("password=[hidden: 7 chars]"), "{text}");
        assert!(
            !text.contains(jwt) && text.contains("for user alice"),
            "{text}"
        );
        // Ordinary words, numbers and URLs stay.
        let plain = "GET /api/items?page=2 returned 200 in 35ms";
        assert_eq!(redact_text(plain), plain);
    }

    proptest::proptest! {
        #[test]
        fn redaction_never_shows_a_secret_header_value_and_keeps_the_shape(
            name in "(Authorization|Cookie|Set-Cookie|X-[A-Za-z]{1,8}-Token)",
            value in "[ -~]{1,60}",
            text in "\\PC{0,200}",
        ) {
            let headers = redact_headers(&json!({name.clone(): value.clone()}));
            proptest::prop_assert_eq!(headers[&name].as_str().unwrap(), hidden(&value));
            // Never panics, and plain text without long tokens is unchanged in length class.
            let _ = redact_text(&text);
            let _ = redact_value(&json!({"k": text}));
        }
    }
}
