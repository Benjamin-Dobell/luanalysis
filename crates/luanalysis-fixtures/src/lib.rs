//! Imports IntelliJ diagnostic markers while preserving the original Lua bytes.
//!
//! This is a fixture format reader, not an XML or Lua parser. Only the known
//! diagnostic tags are removed; Lua's generic/attribute/comparison syntax stays.

use std::fmt;

pub const LEGACY_REPOSITORY: &str = "https://github.com/Benjamin-Dobell/IntelliJ-Luanalysis";
pub const LEGACY_REVISION: &str = "d796bb67d5135d152b9f627b69e57377c83d1a66";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    WeakWarning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::WeakWarning => "weak_warning",
            Self::Info => "info",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "weak_warning" => Some(Self::WeakWarning),
            "info" => Some(Self::Info),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedDiagnostic {
    pub severity: Severity,
    pub description: Option<String>,
    /// Half-open UTF-8 byte offsets in the cleaned Lua source.
    pub span: Span,
    /// Whole marker pair, including both tags, in the original fixture bytes.
    pub original_span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureOrigin {
    pub repository: String,
    pub revision: String,
    pub path: String,
}

impl FixtureOrigin {
    pub fn legacy(path: impl Into<String>) -> Self {
        Self {
            repository: LEGACY_REPOSITORY.into(),
            revision: LEGACY_REVISION.into(),
            path: path.into(),
        }
    }
}

/// Mapping between the marked-up fixture and its unchanged, non-marker bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceMap {
    original_to_clean: Vec<usize>,
    clean_to_original: Vec<usize>,
}

impl SourceMap {
    /// Marker bytes map to the clean insertion point at which they were removed.
    pub fn clean_offset(&self, original_offset: usize) -> Option<usize> {
        self.original_to_clean.get(original_offset).copied()
    }

    /// The corresponding copied byte. The final boundary maps to original EOF.
    pub fn original_offset(&self, clean_offset: usize) -> Option<usize> {
        self.clean_to_original.get(clean_offset).copied()
    }

    /// A nonempty clean range maps from its first to its last copied byte.
    /// Intervening markers may lie inside this original range.
    pub fn original_span(&self, clean: Span) -> Option<Span> {
        if clean.start > clean.end || clean.end >= self.clean_to_original.len() {
            return None;
        }
        let start = self.original_offset(clean.start)?;
        let end = if clean.start == clean.end {
            start
        } else {
            self.original_offset(clean.end - 1)? + 1
        };
        Some(Span { start, end })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixture {
    pub source: String,
    /// Opening-marker order; nested or identical ranges remain distinct.
    pub diagnostics: Vec<ExpectedDiagnostic>,
    pub source_map: SourceMap,
    pub origin: Option<FixtureOrigin>,
}

impl Fixture {
    pub fn with_origin(mut self, origin: FixtureOrigin) -> Self {
        self.origin = Some(origin);
        self
    }

    pub fn snippet(&self, diagnostic: &ExpectedDiagnostic) -> &str {
        &self.source[diagnostic.span.start..diagnostic.span.end]
    }

    /// A portable import artifact. Description text is provenance, not a native
    /// checker's required diagnostic wording or automatically inferred code.
    pub fn to_json(&self) -> String {
        let origin = self.origin.as_ref().map_or("null".into(), |origin| {
            format!(
                "{{\"repository\":{},\"revision\":{},\"path\":{}}}",
                json_string(&origin.repository),
                json_string(&origin.revision),
                json_string(&origin.path)
            )
        });
        let diagnostics = self.diagnostics.iter().map(|diagnostic| {
            let description = diagnostic.description.as_ref().map_or("null".into(), |text| json_string(text));
            format!(
                "{{\"severity\":{},\"description\":{},\"start\":{},\"end\":{},\"snippet\":{},\"original_start\":{},\"original_end\":{}}}",
                json_string(diagnostic.severity.as_str()), description,
                diagnostic.span.start, diagnostic.span.end, json_string(self.snippet(diagnostic)),
                diagnostic.original_span.start, diagnostic.original_span.end
            )
        }).collect::<Vec<_>>().join(",");
        format!(
            "{{\"format\":1,\"origin\":{origin},\"source\":{},\"diagnostics\":[{diagnostics}]}}\n",
            json_string(&self.source)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ImportError {}

fn error(offset: usize, message: impl Into<String>) -> ImportError {
    ImportError {
        offset,
        message: message.into(),
    }
}

struct Marker {
    severity: Severity,
    description: Option<String>,
    closing: bool,
    end: usize,
}

struct OpenMarker {
    severity: Severity,
    description: Option<String>,
    clean_start: usize,
    original_start: usize,
    ordinal: usize,
}

/// Import one marked UTF-8 fixture. Ordinary source bytes and newlines are kept.
pub fn import(marked: &str) -> Result<Fixture, ImportError> {
    let mut source = String::with_capacity(marked.len());
    let mut original_to_clean = vec![0; marked.len() + 1];
    let mut clean_to_original = Vec::with_capacity(marked.len() + 1);
    let mut open: Vec<OpenMarker> = Vec::new();
    let mut diagnostics = Vec::new();
    let mut ordinal = 0;
    let mut cursor = 0;
    while cursor < marked.len() {
        if let Some(marker) = marker_at(marked, cursor)? {
            for entry in &mut original_to_clean[cursor..marker.end] {
                *entry = source.len();
            }
            if marker.closing {
                let Some(opened) = open.pop() else {
                    return Err(error(
                        cursor,
                        format!("unexpected closing {} marker", marker.severity.as_str()),
                    ));
                };
                if opened.severity != marker.severity {
                    return Err(error(
                        cursor,
                        format!(
                            "closing {} marker crosses open {} marker at byte {}",
                            marker.severity.as_str(),
                            opened.severity.as_str(),
                            opened.original_start
                        ),
                    ));
                }
                diagnostics.push((
                    opened.ordinal,
                    ExpectedDiagnostic {
                        severity: opened.severity,
                        description: opened.description,
                        span: Span {
                            start: opened.clean_start,
                            end: source.len(),
                        },
                        original_span: Span {
                            start: opened.original_start,
                            end: marker.end,
                        },
                    },
                ));
            } else {
                open.push(OpenMarker {
                    severity: marker.severity,
                    description: marker.description,
                    clean_start: source.len(),
                    original_start: cursor,
                    ordinal,
                });
                ordinal += 1;
            }
            cursor = marker.end;
        } else {
            let character = marked[cursor..].chars().next().expect("cursor before EOF");
            for byte in 0..character.len_utf8() {
                original_to_clean[cursor + byte] = source.len() + byte;
                clean_to_original.push(cursor + byte);
            }
            source.push(character);
            cursor += character.len_utf8();
        }
    }
    if let Some(opened) = open.last() {
        return Err(error(
            opened.original_start,
            format!("unclosed {} marker", opened.severity.as_str()),
        ));
    }
    original_to_clean[marked.len()] = source.len();
    clean_to_original.push(marked.len());
    diagnostics.sort_by_key(|(ordinal, _)| *ordinal);
    Ok(Fixture {
        source,
        diagnostics: diagnostics
            .into_iter()
            .map(|(_, diagnostic)| diagnostic)
            .collect(),
        source_map: SourceMap {
            original_to_clean,
            clean_to_original,
        },
        origin: None,
    })
}

fn marker_at(source: &str, start: usize) -> Result<Option<Marker>, ImportError> {
    let bytes = source.as_bytes();
    if bytes[start] != b'<' {
        return Ok(None);
    }
    let mut cursor = start + 1;
    let closing = bytes.get(cursor) == Some(&b'/');
    if closing {
        cursor += 1;
    }
    let name_start = cursor;
    while bytes
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        cursor += 1;
    }
    let Some(severity) = Severity::from_tag(&source[name_start..cursor]) else {
        return Ok(None);
    };
    if bytes
        .get(cursor)
        .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>' && *byte != b'/')
    {
        return Ok(None);
    }
    let mut description = None;
    loop {
        skip_space(bytes, &mut cursor);
        match bytes.get(cursor) {
            Some(b'>') => {
                return Ok(Some(Marker {
                    severity,
                    description,
                    closing,
                    end: cursor + 1,
                }));
            }
            None => return Err(error(start, "unterminated diagnostic marker")),
            Some(b'/') => {
                return Err(error(
                    cursor,
                    "self-closing diagnostic markers are not supported",
                ));
            }
            _ if closing => {
                return Err(error(
                    cursor,
                    "closing diagnostic marker cannot have attributes",
                ));
            }
            _ => {}
        }
        let attribute_start = cursor;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            cursor += 1;
        }
        if cursor == attribute_start {
            return Err(error(cursor, "expected diagnostic marker attribute"));
        }
        let attribute = &source[attribute_start..cursor];
        if attribute != "descr" {
            return Err(error(
                attribute_start,
                format!("unsupported diagnostic attribute {attribute:?}"),
            ));
        }
        if description.is_some() {
            return Err(error(attribute_start, "duplicate descr attribute"));
        }
        skip_space(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'=') {
            return Err(error(cursor, "expected '=' after descr"));
        }
        cursor += 1;
        skip_space(bytes, &mut cursor);
        let quote = match bytes.get(cursor) {
            Some(b'\'' | b'"') => bytes[cursor],
            _ => return Err(error(cursor, "descr must have a quoted value")),
        };
        cursor += 1;
        let value_start = cursor;
        let mut value = String::new();
        let mut value_offsets = Vec::new();
        loop {
            let Some(&byte) = bytes.get(cursor) else {
                return Err(error(value_start, "unterminated descr value"));
            };
            if byte == quote {
                cursor += 1;
                break;
            }
            if byte == b'\\' {
                if let Some(&next) = bytes.get(cursor + 1) {
                    if next == quote || next == b'\\' {
                        value.push(next as char);
                        value_offsets.push(cursor + 1);
                        cursor += 2;
                        continue;
                    }
                }
            }
            let character = source[cursor..]
                .chars()
                .next()
                .expect("value cursor before EOF");
            value.push(character);
            value_offsets.extend(cursor..cursor + character.len_utf8());
            cursor += character.len_utf8();
        }
        description = Some(decode_entities(&value, &value_offsets)?);
        if bytes
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>' && *byte != b'/')
        {
            return Err(error(
                cursor,
                "expected whitespace or end of diagnostic marker",
            ));
        }
    }
}

fn skip_space(source: &[u8], cursor: &mut usize) {
    while source.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
        *cursor += 1;
    }
}

fn decode_entities(value: &str, original_offsets: &[usize]) -> Result<String, ImportError> {
    let mut decoded = String::new();
    let mut cursor = 0;
    while cursor < value.len() {
        if value.as_bytes()[cursor] == b'&' {
            if let Some(relative_end) = value[cursor + 1..].find(';') {
                let end = cursor + 1 + relative_end;
                let entity = &value[cursor + 1..end];
                if !entity.is_empty()
                    && entity
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'#')
                {
                    let character = match entity {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        numeric if numeric.starts_with("#x") || numeric.starts_with("#X") => {
                            u32::from_str_radix(&numeric[2..], 16)
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or_else(|| {
                                    error(
                                        original_offsets[cursor],
                                        "invalid hexadecimal description entity",
                                    )
                                })?
                        }
                        numeric if numeric.starts_with('#') => numeric[1..]
                            .parse::<u32>()
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| {
                                error(
                                    original_offsets[cursor],
                                    "invalid decimal description entity",
                                )
                            })?,
                        unknown => {
                            return Err(error(
                                original_offsets[cursor],
                                format!("unknown description entity &{unknown};"),
                            ));
                        }
                    };
                    decoded.push(character);
                    cursor = end + 1;
                    continue;
                }
            }
        }
        let character = value[cursor..]
            .chars()
            .next()
            .expect("entity cursor before EOF");
        decoded.push(character);
        cursor += character.len_utf8();
    }
    Ok(decoded)
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

/// Native matching inputs. Codes are selected by explicit case policy, not by
/// comparing the old IntelliJ prose with the native checker's messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticMatch {
    pub severity: Severity,
    pub span: Span,
    pub code: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    pub missing: Vec<usize>,
    pub unexpected: Vec<usize>,
}

impl Comparison {
    pub fn is_match(&self) -> bool {
        self.missing.is_empty() && self.unexpected.is_empty()
    }
}

/// Exact ranges and multiplicity; no containment/overlap or unsupported waiver.
pub fn compare_exact(expected: &[DiagnosticMatch], actual: &[DiagnosticMatch]) -> Comparison {
    let mut used = vec![false; actual.len()];
    let mut result = Comparison::default();
    // Match explicit codes before wildcard legacy expectations at the same span.
    let mut order: Vec<_> = (0..expected.len()).collect();
    order.sort_by_key(|index| expected[*index].code.is_none());
    for index in order {
        let expected = &expected[index];
        let found = actual.iter().enumerate().position(|(index, actual)| {
            !used[index]
                && expected.severity == actual.severity
                && expected.span == actual.span
                && expected
                    .code
                    .as_ref()
                    .is_none_or(|code| actual.code.as_ref() == Some(code))
        });
        if let Some(found) = found {
            used[found] = true;
        } else {
            result.missing.push(index);
        }
    }
    result.missing.sort_unstable();
    result.unexpected = used
        .iter()
        .enumerate()
        .filter_map(|(index, used)| (!used).then_some(index))
        .collect();
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseStatus {
    SharedSemantics,
    TsDefaultDifference { reason: &'static str },
    Pending { feature: &'static str },
}

/// Explicit legacy environment/expectation selection for a native test case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaseConfig {
    pub id: &'static str,
    pub origin_path: &'static str,
    pub lua_version: &'static str,
    pub strict_nil: bool,
    pub unknown_callable: bool,
    pub unknown_indexable: bool,
    pub checked_severities: &'static [Severity],
    pub status: CaseStatus,
}

impl CaseConfig {
    /// A pending case must not be treated as a successful compatibility result.
    pub fn require_supported(self) -> Result<(), &'static str> {
        match self.status {
            CaseStatus::Pending { feature } => Err(feature),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
