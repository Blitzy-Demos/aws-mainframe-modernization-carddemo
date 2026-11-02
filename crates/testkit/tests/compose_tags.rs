// Compose and testcontainers consistency gate.
//
// docker-compose.yml and the testcontainers stack of crates/testkit/src/containers.rs run one
// tag per image, the same database credentials and the same broker files, and the Compose
// `app` profile reaches its peers by service name: the API binds 0.0.0.0:8080, the Vite proxy
// targets http://api:8080, and every published port listens on 127.0.0.1 only. This suite
// proves all of it statically. It reads docker-compose.yml with the line-based parser below,
// std only and without Docker, and compares what it finds with the public constants of
// `carddemo_testkit::containers` and with the values the app profile must carry.
//
// The parser reads the block-style YAML subset Compose files use. It interprets five service
// properties (image, profiles, environment, ports and volumes) and skips every other property
// by indentation. Whatever it cannot pin to one static value fails the parse with its line
// number rather than passing silently: a variable reference without a default, an anchor,
// alias, merge key or tag, a block scalar or nested value where a scalar is expected, and an
// environment entry whose value would come from the shell.

use std::fs;

use carddemo_testkit::containers::{
    POSTGRES_DB, POSTGRES_IMAGE, POSTGRES_PASSWORD, POSTGRES_TAG, POSTGRES_USER,
    RABBITMQ_CONF_TARGET, RABBITMQ_DEFINITIONS_TARGET, RABBITMQ_IMAGE, RABBITMQ_PASSWORD,
    RABBITMQ_TAG, RABBITMQ_USER,
};
use carddemo_testkit::fixtures::repo_root;

// Repository-relative path of the Compose file under test.
const COMPOSE_FILE: &str = "docker-compose.yml";

// The services the local runtime defines.
const SERVICES: [&str; 5] = ["postgres", "rabbitmq", "api", "mq", "web"];

// The services of the `app` profile, started only by `docker compose --profile app`.
const APP_SERVICES: [&str; 3] = ["api", "mq", "web"];

// The infrastructure services, which start without a profile so that
// `docker compose up -d postgres rabbitmq` serves processes run on the host.
const INFRA_SERVICES: [&str; 2] = ["postgres", "rabbitmq"];

// Image of the `api` and `mq` services: the toolchain rust-toolchain.toml pins.
const RUST_IMAGE: &str = "rust:1.88.0-bookworm";

// Image of the `web` service: the Node.js release web/package.json requires.
const NODE_IMAGE: &str = "node:22.23.4-bookworm";

// The app-profile connection URLs, which name the `postgres` and `rabbitmq` services. The
// user and password are the local-only defaults the plan fixes for docker-compose.yml,
// .env.example and infra/rabbitmq/definitions.json, written out so a drift on either side of
// the comparison with the containers constants fails.
const APP_DATABASE_URL: &str = "postgres://carddemo:carddemo@postgres:5432/carddemo";
const APP_AMQP_URL: &str = "amqp://carddemo:carddemo@rabbitmq:5672/%2f";

// The repository files `TestStack::with_rabbitmq` mounts into its broker; containers.rs keeps
// them private, so they are restated here.
const DEFINITIONS_SOURCE: &str = "infra/rabbitmq/definitions.json";
const CONF_SOURCE: &str = "infra/rabbitmq/rabbitmq.conf";

// The host IP every published port must bind.
const LOCALHOST: &str = "127.0.0.1";

// The ports each publishing service must offer, as `host_ip:published:target`.
const PUBLISHED: [(&str, &[&str]); 4] = [
    ("postgres", &["127.0.0.1:5432:5432"]),
    (
        "rabbitmq",
        &["127.0.0.1:5672:5672", "127.0.0.1:15672:15672"],
    ),
    ("api", &["127.0.0.1:8080:8080"]),
    ("web", &["127.0.0.1:5173:5173"]),
];

// One significant line of a YAML document, with its 1-based number, its indentation in
// spaces, and its text without indentation, comment or trailing blanks. Blank lines, comment
// lines, the document marker and block-scalar content are not kept.
struct Line {
    number: usize,
    indent: usize,
    text: String,
}

// One mount of a service: its source as written, without a leading `./` (a named volume or a
// repository-relative path), and its path in the container.
#[derive(Debug, PartialEq, Eq)]
struct Volume {
    source: String,
    target: String,
}

// The interpreted properties of one entry under the top-level `services` key, and the line
// that opens it.
#[derive(Default)]
struct Service {
    name: String,
    image: Option<String>,
    profiles: Vec<String>,
    environment: Vec<(String, String)>,
    ports: Vec<String>,
    volumes: Vec<Volume>,
    line: usize,
}

impl Service {
    // The service as assertion messages name it.
    fn label(&self) -> String {
        format!("service {} ({COMPOSE_FILE}:{})", self.name, self.line)
    }

    // The value the service's environment gives `key`, if any.
    fn env(&self, key: &str) -> Option<&str> {
        self.environment
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

// ---------------------------------------------------------------------------------------
// Lines
// ---------------------------------------------------------------------------------------

// The significant lines of `text`.
//
// Fails, naming the line, on a tab in an indentation, which YAML forbids, and on an anchor,
// alias, merge key or tag, which this parser does not resolve.
fn lex(text: &str) -> Result<Vec<Line>, String> {
    let mut lines = Vec::new();
    // Indentation of the line whose value opened the current block scalar.
    let mut block_scalar: Option<usize> = None;
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let body = raw.trim_start_matches(' ');
        let indent = raw.len() - body.len();
        if let Some(opener) = block_scalar {
            if body.trim().is_empty() || indent > opener {
                continue;
            }
            block_scalar = None;
        }
        if body.starts_with('\t') {
            return Err(format!("line {number}: tab in the indentation"));
        }
        let content = scan(body).map_err(|kind| {
            format!(
                "line {number}: YAML {kind} found; the parser resolves no anchors, aliases, \
                 merge keys or tags, so {COMPOSE_FILE} must not use them: {}",
                body.trim_end()
            )
        })?;
        if content.is_empty() || content == "---" {
            continue;
        }
        if opens_block_scalar(&content) {
            block_scalar = Some(indent);
        }
        lines.push(Line {
            number,
            indent,
            text: content,
        });
    }
    Ok(lines)
}

// The content of `body`, a line without its indentation, before any comment and without
// trailing blanks; or the kind of construct the parser refuses when one starts a node: an
// anchor (`&name`), an alias (`*name`), a merge key (`<<:`) or a tag (`!name`).
//
// As in YAML, a quote opens a quoted scalar only where a node starts: at the line start, after
// a sequence dash or a mapping colon, or inside a flow collection after its bracket or a
// comma. Elsewhere it is a plain character. A `#` outside quotes starts a comment at the line
// start or after a blank.
fn scan(body: &str) -> Result<String, &'static str> {
    let chars: Vec<char> = body.chars().collect();
    let mut node_start = true;
    let mut flow_depth = 0_usize;
    let mut quote: Option<char> = None;
    let mut end = chars.len();
    let mut at = 0;
    while at < chars.len() {
        let ch = chars[at];
        let next = chars.get(at + 1).copied();
        if let Some(open) = quote {
            let escaped =
                (open == '"' && ch == '\\') || (open == '\'' && ch == '\'' && next == Some('\''));
            if escaped {
                at += 2;
                continue;
            }
            if ch == open {
                quote = None;
            }
            at += 1;
            continue;
        }
        if ch == '#' && (at == 0 || matches!(chars[at - 1], ' ' | '\t')) {
            end = at;
            break;
        }
        at += 1;
        if matches!(ch, ' ' | '\t') {
            continue;
        }
        let blank_or_end = next.is_none_or(|after| matches!(after, ' ' | '\t'));
        if node_start {
            match ch {
                '&' => return Err("anchor"),
                '*' => return Err("alias"),
                '!' => return Err("tag"),
                '<' if next == Some('<')
                    && chars[at + 1..].iter().find(|after| **after != ' ') == Some(&':') =>
                {
                    return Err("merge key");
                }
                '"' | '\'' => {
                    quote = Some(ch);
                    node_start = false;
                    continue;
                }
                '[' | '{' => {
                    flow_depth += 1;
                    continue;
                }
                '-' | '?' if flow_depth == 0 && blank_or_end => continue,
                _ => {}
            }
        }
        node_start = match ch {
            ':' => blank_or_end || (flow_depth > 0 && matches!(next, Some(',' | ']' | '}'))),
            ',' => flow_depth > 0,
            ']' | '}' if flow_depth > 0 => {
                flow_depth -= 1;
                false
            }
            _ => false,
        };
    }
    let content: String = chars[..end].iter().collect();
    Ok(content.trim_end().to_owned())
}

// Whether `content` ends in a block scalar header (`|` or `>`, with optional chomping and
// indentation indicators) as the value of a mapping key or a sequence entry.
fn opens_block_scalar(content: &str) -> bool {
    let (before, last) = content.rsplit_once(' ').unwrap_or(("", content));
    let before = before.trim_end();
    let mut header = last.chars();
    matches!(header.next(), Some('|' | '>'))
        && header.all(|indicator| matches!(indicator, '+' | '-' | '0'..='9'))
        && (before.is_empty() || before.ends_with(':') || before == "-" || before.ends_with(" -"))
}

// ---------------------------------------------------------------------------------------
// Structure
// ---------------------------------------------------------------------------------------

// Whether `text` is a block sequence entry.
fn is_sequence_entry(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

// Groups `lines`, the lines of one block, into entries. Each line at the block's indentation,
// that of its first line, opens an entry, and the deeper lines after it are nested under it.
// Under a key with an empty value, sequence entries at the key's own indentation are nested
// too, since YAML lets a sequence under a mapping key start in the key's column.
//
// Fails, naming the line, on a line indented less than the block's first line.
fn entries(lines: &[Line]) -> Result<Vec<(&Line, &[Line])>, String> {
    let Some(first) = lines.first() else {
        return Ok(Vec::new());
    };
    let indent = first.indent;
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(head) = lines.get(at) {
        if head.indent != indent {
            return Err(format!(
                "line {}: indented {} spaces in a block indented {indent}",
                head.number, head.indent
            ));
        }
        let opens_sequence = split_key(&head.text).is_some_and(|(_, value)| value.is_empty());
        let end = lines[at + 1..]
            .iter()
            .position(|line| {
                line.indent < indent
                    || (line.indent == indent && !(opens_sequence && is_sequence_entry(&line.text)))
            })
            .map_or(lines.len(), |offset| at + 1 + offset);
        found.push((head, &lines[at + 1..end]));
        at = end;
    }
    Ok(found)
}

// Byte index of the quote that closes the quoted scalar opening `text`, whose first character
// is `quote`. In double quotes a backslash escapes the next character; in single quotes two
// quotes stand for one.
fn closing_quote(text: &str, quote: char) -> Option<usize> {
    let mut chars = text.char_indices().skip(1).peekable();
    while let Some((at, ch)) = chars.next() {
        if quote == '"' && ch == '\\' {
            chars.next();
        } else if ch == quote {
            if quote == '\'' && chars.peek().is_some_and(|&(_, after)| after == '\'') {
                chars.next();
            } else {
                return Some(at);
            }
        }
    }
    None
}

// Splits `text` at its mapping indicator, a colon followed by a blank or ending the text, into
// the raw key, quoted or plain, and the trimmed value after it. None for a sequence entry, a
// flow collection or a scalar.
fn split_key(text: &str) -> Option<(&str, &str)> {
    if is_sequence_entry(text) || text.starts_with(['[', '{']) {
        return None;
    }
    if let Some(quote) = text
        .chars()
        .next()
        .filter(|first| matches!(first, '"' | '\''))
    {
        let close = closing_quote(text, quote)?;
        let value = text[close + 1..].trim_start().strip_prefix(':')?;
        return (value.is_empty() || value.starts_with(' '))
            .then(|| (&text[..=close], value.trim()));
    }
    let bytes = text.as_bytes();
    let colon = (0..bytes.len())
        .find(|&at| bytes[at] == b':' && bytes.get(at + 1).is_none_or(|&after| after == b' '))?;
    Some((text[..colon].trim_end(), text[colon + 1..].trim()))
}

// The key on `line`, `text`, unquoted, and its raw value; `what` names the expected key in the
// error for a line that holds none.
fn key_value<'a>(text: &'a str, line: usize, what: &str) -> Result<(String, &'a str), String> {
    let (raw_key, value) =
        split_key(text).ok_or_else(|| format!("line {line}: expected {what}, found {text}"))?;
    Ok((unquote(raw_key, line)?, value))
}

// ---------------------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------------------

// The services of the Compose document `text`, in file order.
//
// Fails, naming the line, on anything the parser cannot read to one static value.
fn parse(text: &str) -> Result<Vec<Service>, String> {
    let lines = lex(text)?;
    if let Some(first) = lines.first().filter(|first| first.indent > 0) {
        return Err(format!(
            "line {}: the document starts indented",
            first.number
        ));
    }
    let mut block = None;
    for (head, nested) in entries(&lines)? {
        let (key, value) = key_value(&head.text, head.number, "a top-level key")?;
        if key != "services" {
            continue;
        }
        if block.is_some() {
            return Err(format!(
                "line {}: a second top-level services key",
                head.number
            ));
        }
        if !value.is_empty() {
            return Err(format!(
                "line {}: services must be a block mapping, found {value}",
                head.number
            ));
        }
        block = Some(nested);
    }
    let block = block.ok_or_else(|| "no top-level services key".to_owned())?;
    let mut services: Vec<Service> = Vec::new();
    for (head, properties) in entries(block)? {
        let service = parse_service(head, properties)?;
        if services.iter().any(|seen| seen.name == service.name) {
            return Err(format!(
                "line {}: service {} is defined twice",
                head.number, service.name
            ));
        }
        services.push(service);
    }
    Ok(services)
}

// The service whose name opens `head` and whose property lines are `properties`.
fn parse_service(head: &Line, properties: &[Line]) -> Result<Service, String> {
    let (name, value) = key_value(&head.text, head.number, "a service name")?;
    if !value.is_empty() {
        return Err(format!(
            "line {}: service {name} must be a block mapping, found {value}",
            head.number
        ));
    }
    let mut service = Service {
        name,
        line: head.number,
        ..Service::default()
    };
    let mut seen: Vec<String> = Vec::new();
    for (property, nested) in entries(properties)? {
        let line = property.number;
        let (key, value) = key_value(&property.text, line, "a service property")?;
        if seen.contains(&key) {
            return Err(format!(
                "line {line}: property {key} of service {} is defined twice",
                service.name
            ));
        }
        match key.as_str() {
            "image" => service.image = Some(inline_scalar(value, nested, line)?),
            "profiles" => service.profiles = scalar_list(value, nested, line)?,
            "environment" => service.environment = environment(value, nested, line)?,
            "ports" => service.ports = ports(value, nested, line)?,
            "volumes" => service.volumes = volumes(value, nested, line)?,
            _ => {}
        }
        seen.push(key);
    }
    Ok(service)
}

// One item of a block sequence: the number of the line holding its dash, its text after the
// dash, and the lines nested under it.
struct Item<'a> {
    number: usize,
    text: &'a str,
    nested: &'a [Line],
}

// The items of the block sequence `nested`.
fn sequence(nested: &[Line]) -> Result<Vec<Item<'_>>, String> {
    entries(nested)?
        .into_iter()
        .map(|(head, sub)| {
            if !is_sequence_entry(&head.text) {
                return Err(format!(
                    "line {}: expected a sequence entry, found {}",
                    head.number, head.text
                ));
            }
            Ok(Item {
                number: head.number,
                text: head.text[1..].trim(),
                nested: sub,
            })
        })
        .collect()
}

// The scalar items of a sequence value: a one-line flow sequence on the key's line, or a block
// sequence of one-line scalars nested under it.
fn scalar_list(value: &str, nested: &[Line], line: usize) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return sequence(nested)?
            .into_iter()
            .map(|item| inline_scalar(item.text, item.nested, item.number))
            .collect();
    }
    match nested.first() {
        Some(extra) => Err(format!(
            "line {}: lines nested under the one-line value on line {line}",
            extra.number
        )),
        None => flow_list(value, line),
    }
}

// The pairs of a mapping whose first pair may share the line `line` of its sequence dash
// (`first`, empty when the dash stands alone) and whose further pairs are `nested`. A key whose
// value is a nested block, such as the bind options of a long-form volume, is not kept.
fn scalar_map(first: &str, line: usize, nested: &[Line]) -> Result<Vec<(String, String)>, String> {
    let mut rows: Vec<(&str, usize, &[Line])> = Vec::new();
    if !first.is_empty() {
        rows.push((first, line, &[]));
    }
    for (head, sub) in entries(nested)? {
        rows.push((&head.text, head.number, sub));
    }
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (text, number, sub) in rows {
        let (key, value) = key_value(text, number, "a key")?;
        if pairs.iter().any(|(seen, _)| *seen == key) {
            return Err(format!("line {number}: key {key} is defined twice"));
        }
        if value.is_empty() && !sub.is_empty() {
            continue;
        }
        pairs.push((key, inline_scalar(value, sub, number)?));
    }
    Ok(pairs)
}

// The value of `name` among `pairs`.
fn field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

// Whether a sequence item is a mapping: a dash standing alone or followed by a key.
fn is_mapping_item(item: &str) -> bool {
    item.is_empty() || split_key(item).is_some()
}

// The variables of an environment value, in file order: the list form `- KEY=VALUE`, whose
// entry may be quoted as a whole, or the mapping form `KEY: VALUE`.
fn environment(value: &str, nested: &[Line], line: usize) -> Result<Vec<(String, String)>, String> {
    if !value.is_empty() {
        return Err(format!(
            "line {line}: expected the environment as a block list or mapping, found {value}"
        ));
    }
    if !nested
        .first()
        .is_some_and(|first| is_sequence_entry(&first.text))
    {
        return scalar_map("", line, nested);
    }
    sequence(nested)?
        .into_iter()
        .map(|item| {
            let entry = inline_scalar(item.text, item.nested, item.number)?;
            let (key, value) = entry.split_once('=').ok_or_else(|| {
                format!(
                    "line {}: {entry} has no =VALUE, so its value would come from the shell",
                    item.number
                )
            })?;
            Ok((key.to_owned(), value.to_owned()))
        })
        .collect()
}

// The published ports of a ports value, each as `[host_ip:][published:]target[/protocol]`:
// short-form strings as written, long-form mappings normalized to that form.
fn ports(value: &str, nested: &[Line], line: usize) -> Result<Vec<String>, String> {
    if !value.is_empty() {
        return scalar_list(value, nested, line);
    }
    sequence(nested)?
        .into_iter()
        .map(|item| {
            if !is_mapping_item(item.text) {
                return inline_scalar(item.text, item.nested, item.number);
            }
            let pairs = scalar_map(item.text, item.number, item.nested)?;
            let target = field(&pairs, "target")
                .ok_or_else(|| format!("line {}: long-form port without target", item.number))?;
            let port = match (field(&pairs, "host_ip"), field(&pairs, "published")) {
                (Some(host_ip), Some(published)) => format!("{host_ip}:{published}:{target}"),
                (Some(host_ip), None) => format!("{host_ip}::{target}"),
                (None, Some(published)) => format!("{published}:{target}"),
                (None, None) => target.to_owned(),
            };
            let protocol = field(&pairs, "protocol").filter(|protocol| *protocol != "tcp");
            Ok(match protocol {
                Some(protocol) => format!("{port}/{protocol}"),
                None => port,
            })
        })
        .collect()
}

// The mounts of a volumes value: short-form `SOURCE:TARGET[:MODE]` strings and long-form
// mappings with `source` and `target` keys.
fn volumes(value: &str, nested: &[Line], line: usize) -> Result<Vec<Volume>, String> {
    if !value.is_empty() {
        return scalar_list(value, nested, line)?
            .iter()
            .map(|short| short_volume(short, line))
            .collect();
    }
    sequence(nested)?
        .into_iter()
        .map(|item| {
            if !is_mapping_item(item.text) {
                let short = inline_scalar(item.text, item.nested, item.number)?;
                return short_volume(&short, item.number);
            }
            let pairs = scalar_map(item.text, item.number, item.nested)?;
            let target = field(&pairs, "target")
                .ok_or_else(|| format!("line {}: long-form volume without target", item.number))?;
            Ok(volume(field(&pairs, "source").unwrap_or_default(), target))
        })
        .collect()
}

// The mount the short-form volume `short` on line `line` declares.
fn short_volume(short: &str, line: usize) -> Result<Volume, String> {
    match short.split(':').collect::<Vec<_>>().as_slice() {
        [target] => Ok(volume("", target)),
        [source, target] | [source, target, _] => Ok(volume(source, target)),
        _ => Err(format!(
            "line {line}: expected SOURCE:TARGET[:MODE], found {short}"
        )),
    }
}

// The mount of `source` at `target`, the source without a leading `./`.
fn volume(source: &str, target: &str) -> Volume {
    Volume {
        source: source.strip_prefix("./").unwrap_or(source).to_owned(),
        target: target.to_owned(),
    }
}

// ---------------------------------------------------------------------------------------
// Scalars
// ---------------------------------------------------------------------------------------

// The value `value` of the key on line `line`, which must be a one-line scalar with no lines
// nested under it. A key without a value has no static value, so it fails too.
fn inline_scalar(value: &str, nested: &[Line], line: usize) -> Result<String, String> {
    if let Some(extra) = nested.first() {
        return Err(format!(
            "line {}: expected the value on line {line} to be a one-line scalar",
            extra.number
        ));
    }
    if value.is_empty() {
        return Err(format!("line {line}: the value is missing"));
    }
    scalar(value, line)
}

// The static value of the scalar `raw` on line `line`: unquoted, then interpolated.
fn scalar(raw: &str, line: usize) -> Result<String, String> {
    if opens_block_scalar(raw) || raw.starts_with(['[', '{']) {
        return Err(format!(
            "line {line}: expected a one-line scalar, found {raw}"
        ));
    }
    interpolate(&unquote(raw, line)?, line)
}

// The text of the scalar `raw`: a double-quoted scalar with its `\"` and `\\` escapes decoded,
// a single-quoted scalar with each `''` read as one quote, or a plain scalar as written.
fn unquote(raw: &str, line: usize) -> Result<String, String> {
    let Some(quote) = raw
        .chars()
        .next()
        .filter(|first| matches!(first, '"' | '\''))
    else {
        return Ok(raw.to_owned());
    };
    let close = closing_quote(raw, quote)
        .ok_or_else(|| format!("line {line}: unterminated quoted scalar {raw}"))?;
    if !raw[close + 1..].trim().is_empty() {
        return Err(format!(
            "line {line}: text after the closing quote of {raw}"
        ));
    }
    let body = &raw[1..close];
    if quote == '\'' {
        return Ok(body.replace("''", "'"));
    }
    let mut text = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            text.push(ch);
            continue;
        }
        match chars.next() {
            Some(escaped @ ('"' | '\\')) => text.push(escaped),
            other => {
                let shown: String = other.into_iter().collect();
                return Err(format!(
                    "line {line}: unsupported escape \\{shown} in {raw}"
                ));
            }
        }
    }
    Ok(text)
}

// `value` with its Compose interpolation resolved: `$$` is a literal `$`, and
// `${NAME:-default}` or `${NAME-default}` is its default. Any other reference, `${NAME}`,
// `$NAME` or a required or alternative form, takes its value from the shell, so it fails.
fn interpolate(value: &str, line: usize) -> Result<String, String> {
    let mut text = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        text.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            text.push('$');
            rest = tail;
        } else if let Some(body) = after.strip_prefix('{') {
            let close = matching_brace(body).ok_or_else(|| {
                format!("line {line}: unterminated variable reference in {value}")
            })?;
            text.push_str(&resolve_variable(&body[..close], value, line)?);
            rest = &body[close + 1..];
        } else if after.starts_with(|first: char| first == '_' || first.is_ascii_alphabetic()) {
            return Err(format!(
                "line {line}: variable reference without a default in {value}; its value \
                 would come from the shell, so the expectation is not statically known"
            ));
        } else {
            text.push('$');
            rest = after;
        }
    }
    text.push_str(rest);
    Ok(text)
}

// Byte index of the brace that closes a variable reference whose body starts `body`, counting
// the braces of references nested in a default.
fn matching_brace(body: &str) -> Option<usize> {
    let mut depth = 0_usize;
    for (at, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(at),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

// The default of the braced variable reference `body`, itself interpolated.
fn resolve_variable(body: &str, value: &str, line: usize) -> Result<String, String> {
    let name_end = body
        .find(|ch: char| ch != '_' && !ch.is_ascii_alphanumeric())
        .unwrap_or(body.len());
    let (name, operator) = body.split_at(name_end);
    let default = operator
        .strip_prefix(":-")
        .or_else(|| operator.strip_prefix('-'));
    match default {
        Some(default) if !name.is_empty() => interpolate(default, line),
        _ => Err(format!(
            "line {line}: ${{{body}}} in {value} has no default; its value would come from \
             the shell, so the expectation is not statically known"
        )),
    }
}

// The items of the one-line flow sequence `raw`, such as `["app", tools]`.
fn flow_list(raw: &str, line: usize) -> Result<Vec<String>, String> {
    let inner = raw
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| format!("line {line}: expected a one-line flow sequence, found {raw}"))?;
    let mut items = Vec::new();
    let mut rest = inner.trim();
    while !rest.is_empty() {
        let end = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                closing_quote(rest, quote)
                    .ok_or_else(|| format!("line {line}: unterminated quoted item in {raw}"))?
                    + 1
            }
            _ => rest.find(',').unwrap_or(rest.len()),
        };
        let item = rest[..end].trim();
        items.push(scalar(item, line)?);
        rest = rest[end..].trim_start();
        match rest.strip_prefix(',') {
            Some(after) => rest = after.trim_start(),
            None if rest.is_empty() => {}
            None => {
                return Err(format!(
                    "line {line}: expected a comma after {item} in {raw}"
                ));
            }
        }
    }
    Ok(items)
}

// ---------------------------------------------------------------------------------------
// The Compose file under test
// ---------------------------------------------------------------------------------------

// The services of docker-compose.yml. Fails the calling test when the file is missing or the
// parser rejects it, naming the line.
fn compose_services() -> Vec<Service> {
    let path = repo_root().join(COMPOSE_FILE);
    assert!(path.is_file(), "{} does not exist", path.display());
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    parse(&text).unwrap_or_else(|error| panic!("{COMPOSE_FILE}: {error}"))
}

// The service `name` of `services`.
fn service<'a>(services: &'a [Service], name: &str) -> &'a Service {
    services
        .iter()
        .find(|service| service.name == name)
        .unwrap_or_else(|| panic!("{COMPOSE_FILE} defines no service {name}"))
}

// Asserts that `service` sets the environment variable `key` to `expected`.
fn assert_env(service: &Service, key: &str, expected: &str) {
    assert_eq!(
        service.env(key),
        Some(expected),
        "{}: environment {key}: expected {expected}, found {:?}",
        service.label(),
        service.env(key)
    );
}

// ---------------------------------------------------------------------------------------
// Parser self-checks
// ---------------------------------------------------------------------------------------

// Every form the parser reads, in one document: list and mapping environments, quoted and
// plain values, block and flow profiles, short and long ports and volumes, comments after
// values, interpolation defaults, a skipped block scalar and skipped nested properties.
const SAMPLE: &str = r#"---
# A sample in the forms a Compose file may use.
name: sample
services:
  alpha:
    image: "example/alpha:1.0" # comment after a quoted value
    profiles:
      - app
      - 'tools'
    environment:
      - PLAIN=value # comment after a plain value
      - "QUOTED=with # hash"
      - 'SINGLE=it''s'
      - DEFAULTED=${UNSET_VARIABLE:-fallback}
      - DASH_DEFAULT=${UNSET_VARIABLE-other}
      - DOLLARS=$${LITERAL}
    command: |
      echo "*not an alias*" # inside a block scalar
      &not-an-anchor
    ports:
      - "127.0.0.1:5432:5432"
      - 8080:80
      - target: 5672
        published: "5673"
        host_ip: 127.0.0.1
    volumes:
      - ./infra/conf.json:/etc/conf.json:ro
      - data:/var/lib/data
      - type: bind
        source: ./infra/other.conf
        target: /etc/other.conf
        read_only: true
    healthcheck:
      test: ["CMD", "check", "--flag"]
      interval: 5s

  beta:
    image: example/beta:${BETA_TAG:-2.0}
    profiles: ["app", tools]
    environment:
      MAP_PLAIN: value#not-a-comment
      MAP_DOUBLE: "say \"hi\" \\ done"
      MAP_SINGLE: 'single # kept'
      "QUOTED_KEY": 'v'
      EMPTY: ""
    depends_on:
      alpha:
        condition: service_started
volumes:
  data:
"#;

#[test]
fn parser_finds_all_services() {
    let services = compose_services();
    for name in SERVICES {
        let found = service(&services, name);
        assert!(
            found
                .image
                .as_deref()
                .is_some_and(|image| !image.is_empty()),
            "{}: image: expected one, found {:?}",
            found.label(),
            found.image
        );
    }
}

#[test]
fn parser_reads_every_supported_form() {
    let services = parse(SAMPLE).unwrap_or_else(|error| panic!("sample: {error}"));
    let names: Vec<&str> = services.iter().map(|found| found.name.as_str()).collect();
    assert_eq!(names, ["alpha", "beta"], "services of the sample");

    let alpha = service(&services, "alpha");
    assert_eq!(alpha.line, 5, "line that opens alpha");
    assert_eq!(alpha.image.as_deref(), Some("example/alpha:1.0"));
    assert_eq!(alpha.profiles, ["app", "tools"]);
    let environment: Vec<(&str, &str)> = alpha
        .environment
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        environment,
        [
            ("PLAIN", "value"),
            ("QUOTED", "with # hash"),
            ("SINGLE", "it's"),
            ("DEFAULTED", "fallback"),
            ("DASH_DEFAULT", "other"),
            ("DOLLARS", "${LITERAL}"),
        ]
    );
    assert_eq!(
        alpha.ports,
        ["127.0.0.1:5432:5432", "8080:80", "127.0.0.1:5673:5672"]
    );
    assert_eq!(
        alpha.volumes,
        [
            volume("infra/conf.json", "/etc/conf.json"),
            volume("data", "/var/lib/data"),
            volume("infra/other.conf", "/etc/other.conf"),
        ]
    );

    let beta = service(&services, "beta");
    assert_eq!(beta.image.as_deref(), Some("example/beta:2.0"));
    assert_eq!(beta.profiles, ["app", "tools"]);
    let environment: Vec<(&str, &str)> = beta
        .environment
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        environment,
        [
            ("MAP_PLAIN", "value#not-a-comment"),
            ("MAP_DOUBLE", "say \"hi\" \\ done"),
            ("MAP_SINGLE", "single # kept"),
            ("QUOTED_KEY", "v"),
            ("EMPTY", ""),
        ]
    );
    assert!(beta.ports.is_empty() && beta.volumes.is_empty());
}

#[test]
fn parser_rejects_values_it_cannot_pin() {
    let cases = [
        ("bare variable", "services:\n  a:\n    image: ${IMAGE}\n", 3),
        (
            "required variable",
            "services:\n  a:\n    image: ${IMAGE:?set it}\n",
            3,
        ),
        (
            "unbraced variable",
            "services:\n  a:\n    image: $IMAGE\n",
            3,
        ),
        (
            "anchor",
            "x-base: &base\n  image: a\nservices:\n  a:\n    image: b\n",
            1,
        ),
        (
            "alias",
            "services:\n  a:\n    image: b\n    environment: *env\n",
            4,
        ),
        (
            "merge key",
            "services:\n  a:\n    environment:\n      <<: *env\n",
            4,
        ),
        (
            "flow alias",
            "services:\n  a:\n    profiles: [app, *more]\n",
            3,
        ),
        ("tag", "services:\n  a:\n    ports: !reset []\n", 3),
        (
            "shell value",
            "services:\n  a:\n    environment:\n      - TOKEN\n",
            4,
        ),
        (
            "null value",
            "services:\n  a:\n    environment:\n      TOKEN:\n",
            4,
        ),
        (
            "block scalar",
            "services:\n  a:\n    image: |\n      name:tag\n",
            3,
        ),
        ("tab indentation", "services:\n\ta:\n", 2),
    ];
    for (case, text, line) in cases {
        let error = parse(text)
            .err()
            .unwrap_or_else(|| panic!("{case}: the parser accepted {text:?}"));
        assert!(
            error.starts_with(&format!("line {line}:")),
            "{case}: expected an error naming line {line}, found {error}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Compose against the test stack and the app profile
// ---------------------------------------------------------------------------------------

#[test]
fn images_match_testcontainers_tags() {
    assert_eq!(
        (POSTGRES_IMAGE, POSTGRES_TAG),
        ("postgres", "16.15"),
        "containers::POSTGRES_IMAGE and POSTGRES_TAG must name the pinned PostgreSQL image"
    );
    assert_eq!(
        (RABBITMQ_IMAGE, RABBITMQ_TAG),
        ("rabbitmq", "3.13.7-management"),
        "containers::RABBITMQ_IMAGE and RABBITMQ_TAG must name the pinned RabbitMQ image"
    );
    let postgres = format!("{POSTGRES_IMAGE}:{POSTGRES_TAG}");
    let rabbitmq = format!("{RABBITMQ_IMAGE}:{RABBITMQ_TAG}");
    let services = compose_services();
    for (name, expected) in [
        ("postgres", postgres.as_str()),
        ("rabbitmq", rabbitmq.as_str()),
        ("api", RUST_IMAGE),
        ("mq", RUST_IMAGE),
        ("web", NODE_IMAGE),
    ] {
        let found = service(&services, name);
        assert_eq!(
            found.image.as_deref(),
            Some(expected),
            "{}: image: expected {expected}, found {:?}",
            found.label(),
            found.image
        );
    }
}

#[test]
fn app_profile_membership() {
    let services = compose_services();
    for name in APP_SERVICES {
        let found = service(&services, name);
        assert!(
            found.profiles.iter().any(|profile| profile == "app"),
            "{}: profiles: expected to include app, found {:?}",
            found.label(),
            found.profiles
        );
    }
    for name in INFRA_SERVICES {
        let found = service(&services, name);
        assert!(
            found.profiles.is_empty(),
            "{}: profiles: expected none, so that `docker compose up -d postgres rabbitmq` \
             starts it for the host-toolchain mode, found {:?}",
            found.label(),
            found.profiles
        );
    }
}

#[test]
fn app_profile_uses_service_names() {
    let database_url =
        format!("postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@postgres:5432/{POSTGRES_DB}");
    let amqp_url = format!("amqp://{RABBITMQ_USER}:{RABBITMQ_PASSWORD}@rabbitmq:5672/%2f");
    assert_eq!(
        database_url, APP_DATABASE_URL,
        "the containers database constants must give the app-profile database URL"
    );
    assert_eq!(
        amqp_url, APP_AMQP_URL,
        "the containers broker constants must give the app-profile AMQP URL"
    );
    let services = compose_services();
    for name in ["api", "mq"] {
        let found = service(&services, name);
        assert_env(found, "CARDDEMO_DATABASE_URL", APP_DATABASE_URL);
        assert_env(found, "CARDDEMO_AMQP_URL", APP_AMQP_URL);
        assert_env(found, "SQLX_OFFLINE", "true");
    }
    assert_env(
        service(&services, "api"),
        "CARDDEMO_HTTP_ADDR",
        "0.0.0.0:8080",
    );
    assert_env(
        service(&services, "web"),
        "CARDDEMO_API_PROXY_TARGET",
        "http://api:8080",
    );
    let postgres = service(&services, "postgres");
    for (key, expected) in [
        ("POSTGRES_DB", POSTGRES_DB),
        ("POSTGRES_USER", POSTGRES_USER),
        ("POSTGRES_PASSWORD", POSTGRES_PASSWORD),
    ] {
        assert_env(postgres, key, expected);
    }
}

#[test]
fn published_ports_are_localhost() {
    let services = compose_services();
    for (name, expected) in PUBLISHED {
        let found = service(&services, name);
        for port in expected {
            assert!(
                found.ports.iter().any(|published| published == port),
                "{}: ports: expected {port}, found {:?}",
                found.label(),
                found.ports
            );
        }
    }
    let mq = service(&services, "mq");
    assert!(
        mq.ports.is_empty(),
        "{}: ports: expected none, found {:?}",
        mq.label(),
        mq.ports
    );
    for found in &services {
        for port in &found.ports {
            let on_localhost = port
                .strip_prefix(LOCALHOST)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| rest.contains(':'));
            assert!(
                on_localhost,
                "{}: ports: {port} is published without the {LOCALHOST} host IP",
                found.label()
            );
        }
    }
}

#[test]
fn rabbitmq_mounts_match_test_stack() {
    let services = compose_services();
    let rabbitmq = service(&services, "rabbitmq");
    for (source, target) in [
        (DEFINITIONS_SOURCE, RABBITMQ_DEFINITIONS_TARGET),
        (CONF_SOURCE, RABBITMQ_CONF_TARGET),
    ] {
        assert!(
            rabbitmq.volumes.contains(&volume(source, target)),
            "{}: volumes: expected {source} mounted at {target}, found {:?}",
            rabbitmq.label(),
            rabbitmq.volumes
        );
        let path = repo_root().join(source);
        assert!(
            path.is_file(),
            "{}: volumes: the mount source {} does not exist",
            rabbitmq.label(),
            path.display()
        );
    }
    let conf_path = repo_root().join(CONF_SOURCE);
    let conf = fs::read_to_string(&conf_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", conf_path.display()));
    let loaded: Vec<&str> = conf
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "load_definitions").then(|| value.trim())
        })
        .collect();
    assert_eq!(
        loaded,
        [RABBITMQ_DEFINITIONS_TARGET],
        "{CONF_SOURCE}: load_definitions: expected exactly one, importing the mounted \
         definitions at {RABBITMQ_DEFINITIONS_TARGET}"
    );
}
