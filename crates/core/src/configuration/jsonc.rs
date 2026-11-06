use super::*;

pub fn parse(content: &str) -> Result<Value> {
    let bytes = strip_jsonc(content)?;
    let value: Value = serde_json::from_slice(&bytes).context("invalid configuration JSONC")?;
    if let Some((message, line)) = repeated_key(&bytes) {
        anyhow::bail!("{message} (line {line})");
    }
    ensure!(value.is_object(), "configuration must be an object");
    Ok(value)
}

/// `content` (a JSONC configuration) with its top-level `key` set to `value`. Everything
/// else, comments included, is left as written; only the old value of `key` (with any
/// comments inside it) is replaced. Settings use this for field-level writes.
pub fn set_top_level(content: &str, key: &str, value: &Value) -> Result<String> {
    let rendered = serde_json::to_string_pretty(value)?.replace('\n', "\n  ");
    if content.trim().is_empty() {
        return Ok(format!(
            "{{\n  {}: {rendered}\n}}\n",
            serde_json::to_string(key)?
        ));
    }
    parse(content)?;
    let bytes = strip_jsonc(content)?;
    let (mut depth, mut i) = (0usize, 0usize);
    let mut found: Option<(usize, usize)> = None;
    let mut last_close = None;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                let end = i;
                i += 1;
                if depth != 1 {
                    continue;
                }
                let mut after = i;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if bytes.get(after) != Some(&b':') {
                    continue;
                }
                let name: String = serde_json::from_slice(&bytes[start..=end])?;
                let mut value_start = after + 1;
                while bytes[value_start].is_ascii_whitespace() {
                    value_start += 1;
                }
                // The value ends at the next ',' or '}' back at this depth.
                let (mut inner, mut j, mut in_string) = (0usize, value_start, false);
                while j < bytes.len() {
                    let c = bytes[j];
                    if in_string {
                        if c == b'\\' {
                            j += 1;
                        } else if c == b'"' {
                            in_string = false;
                        }
                    } else if c == b'"' {
                        in_string = true;
                    } else if c == b'{' || c == b'[' {
                        inner += 1;
                    } else if c == b'}' || c == b']' {
                        if inner == 0 {
                            break;
                        }
                        inner -= 1;
                    } else if c == b',' && inner == 0 {
                        break;
                    }
                    j += 1;
                }
                let mut value_end = j;
                while value_end > value_start && bytes[value_end - 1].is_ascii_whitespace() {
                    value_end -= 1;
                }
                if name == key {
                    found = Some((value_start, value_end));
                }
                i = j;
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    last_close = Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    let updated = if let Some((start, end)) = found {
        format!("{}{rendered}{}", &content[..start], &content[end..])
    } else {
        let close = last_close.context("configuration must be an object")?;
        let before = bytes[..close]
            .iter()
            .rposition(|c| !c.is_ascii_whitespace());
        let entry = format!("{}: {rendered}", serde_json::to_string(key)?);
        match before {
            // An object with no settings yet: the entry goes first, comments stay after it.
            Some(position) if bytes[position] == b'{' => format!(
                "{}\n  {entry},{}",
                &content[..=position],
                &content[position + 1..]
            ),
            Some(position) => format!(
                "{},\n  {entry}{}",
                &content[..=position],
                &content[position + 1..]
            ),
            None => bail_unbalanced()?,
        }
    };
    let check = parse(&updated).context("updated configuration is not valid JSONC")?;
    ensure!(
        check.get(key) == Some(value),
        "updated configuration lost the new value"
    );
    Ok(updated)
}

fn bail_unbalanced() -> Result<String> {
    anyhow::bail!("configuration must be an object")
}

/// Parses JSONC (comments, trailing commas) of at most 1 MiB; an error carries the 1-based
/// line it was found on (0 when the input is too large).
pub fn parse_jsonc(content: &str) -> std::result::Result<Value, (String, usize)> {
    let bytes = strip_jsonc(content).map_err(|e| (e.to_string(), 0))?;
    serde_json::from_slice(&bytes).map_err(|e| {
        let message = e.to_string();
        let message = message
            .split(" at line ")
            .next()
            .unwrap_or(&message)
            .to_owned();
        (message, e.line())
    })
}

/// The first key an object sets twice in valid JSON `bytes`, as a message and its 1-based
/// line. JSON keeps only the last value, so two uncommented lines for one block would
/// silently drop the first.
pub(super) fn repeated_key(bytes: &[u8]) -> Option<(String, usize)> {
    // One entry per open object (its keys) or array (None).
    let mut open: Vec<Option<std::collections::HashSet<&[u8]>>> = Vec::new();
    let mut expect_key = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                let key = &bytes[start..i.min(bytes.len())];
                if let (true, Some(Some(keys))) = (expect_key, open.last_mut()) {
                    if !keys.insert(key) {
                        let line = 1 + bytes[..start].iter().filter(|b| **b == b'\n').count();
                        let name = String::from_utf8_lossy(key);
                        return Some((
                            format!("\"{name}\" is set twice; put both settings in one \"{name}\""),
                            line,
                        ));
                    }
                }
                expect_key = false;
            }
            b'{' => {
                open.push(Some(Default::default()));
                expect_key = true;
            }
            b'[' => {
                open.push(None);
                expect_key = false;
            }
            b'}' | b']' => {
                open.pop();
                expect_key = false;
            }
            b',' => expect_key = matches!(open.last(), Some(Some(_))),
            _ => {}
        }
        i += 1;
    }
    None
}

/// JSONC text as plain JSON bytes: comments and trailing commas become spaces,
/// so byte offsets, lines and columns stay those of the original file.
fn strip_jsonc(content: &str) -> Result<Vec<u8>> {
    ensure!(content.len() <= 1024 * 1024, "configuration exceeds 1 MiB");
    let mut bytes = content.as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;
    let mut escaped = false;
    while i < bytes.len() {
        if quoted {
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == b'"' {
                quoted = false;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            quoted = true;
            i += 1;
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                bytes[i] = b' ';
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    bytes[i] = b' ';
                    bytes[i + 1] = b' ';
                    i += 2;
                    closed = true;
                    break;
                }
                if bytes[i] != b'\n' {
                    bytes[i] = b' ';
                }
                i += 1;
            }
            ensure!(closed, "unterminated configuration comment");
            continue;
        }
        i += 1;
    }
    quoted = false;
    escaped = false;
    for i in 0..bytes.len() {
        if quoted {
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == b'"' {
                quoted = false;
            }
            continue;
        }
        if bytes[i] == b'"' {
            quoted = true;
            continue;
        }
        if bytes[i] == b','
            && bytes[i + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'}' || *b == b']')
        {
            bytes[i] = b' ';
        }
    }
    Ok(bytes)
}

/// A problem in one configuration file, with its position when known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileIssue {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Syntax and schema check of one configuration file on its own (comments,
/// JSON syntax, unknown fields, wrong types), with line and column.
pub fn check_file(content: &str) -> Option<FileIssue> {
    let bytes = match strip_jsonc(content) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Some(FileIssue {
                message: format!("{error:#}"),
                line: None,
                column: None,
            })
        }
    };
    let positioned = |error: serde_json::Error| {
        Some(FileIssue {
            // The position is reported separately; drop serde's trailing copy.
            message: {
                let text = error.to_string();
                let suffix = format!(" at line {} column {}", error.line(), error.column());
                text.strip_suffix(&suffix)
                    .map(str::to_owned)
                    .unwrap_or(text)
            },
            line: Some(error.line()).filter(|line| *line > 0),
            column: (error.line() > 0).then_some(error.column()),
        })
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => return positioned(error),
    };
    if let Some((message, line)) = repeated_key(&bytes) {
        return Some(FileIssue {
            message,
            line: Some(line),
            column: None,
        });
    }
    if !value.is_object() {
        return Some(FileIssue {
            message: "configuration must be an object".into(),
            line: Some(1),
            column: None,
        });
    }
    if let Err(error) = serde_json::from_slice::<RuntimeConfig>(&bytes) {
        return positioned(error);
    }
    None
}

/// Line of the setting a validation message names (`sandbox.backend must ...`
/// points at the line with `"backend"`), when the file mentions it.
pub fn locate_error_line(content: &str, message: &str) -> Option<usize> {
    let field = message
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .find(|word| word.contains('.') && word.chars().next().is_some_and(char::is_alphabetic))?;
    let key = field.rsplit('.').next().filter(|key| !key.is_empty())?;
    let quoted = format!("\"{key}\"");
    content
        .lines()
        .position(|line| line.contains(&quoted))
        .map(|index| index + 1)
}
