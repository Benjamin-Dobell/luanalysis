//! Conservative wrapping of standalone prose and annotation comments, and `nil` first in the
//! unions of any annotation comment.
//!
//! Executable tokens never pass through this module. Each replacement must lex as
//! one comment and retain the comment's content and parsed annotation structure, its unions'
//! members aside, which only move.

use luanalysis_syntax::{Comment, CommentKind, Span, Token, TokenKind};
use super::{FormatEdit, FormattingOptions};

pub(super) fn edits(
    source: &str,
    tokens: &[&Token],
    options: &FormattingOptions,
    bounds: Span,
    newline: &str,
    indent_at: impl Fn(usize) -> String,
) -> Vec<FormatEdit> {
    let mut edits = Vec::new();
    for token in tokens {
        let TokenKind::Comment(comment) = &token.kind else { continue; };
        if comment.span.start < bounds.start || comment.span.end > bounds.end { continue; }
        // A union's written `nil` comes first, in any annotation comment; only a standalone one
        // wraps.
        let reordered = nil_first(comment);
        let text = reordered.as_deref().unwrap_or(&comment.text);
        let wrapped = if standalone(source, comment.span.start) && ends_line(source, comment.span.end) {
            let indent = indent_at(comment.span.start);
            match comment.kind {
                CommentKind::Documentation if text.starts_with("---") =>
                    documentation(text, &indent, options, newline),
                CommentKind::Line if options.max_line_length > 0 =>
                    prose(text, &indent, options, newline),
                _ => None,
            }
        } else { None };
        let Some(new_text) = wrapped.or(reordered).filter(|text| *text != comment.text) else { continue; };
        let lexed = luanalysis_syntax::lex(&new_text);
        let comments: Vec<_> = lexed.tokens.iter().filter_map(|token| match &token.kind {
            TokenKind::Comment(comment) => Some(comment),
            TokenKind::Newline | TokenKind::Eof => None,
            _ => None,
        }).collect();
        if lexed.diagnostics.is_empty() && comments.len() == 1
            && lexed.tokens.iter().all(|token| matches!(token.kind,
                TokenKind::Comment(_) | TokenKind::Newline | TokenKind::Eof))
            && equivalent_formatted(comment, comments[0])
        {
            edits.push(FormatEdit { span: comment.span, new_text });
        }
    }
    edits
}

/// `comment` with each union's written `nil` first: `nil | string` for `string|nil`'s
/// `nil|string`. Its members keep their text and the union its separators, so nothing else moves
/// and no span does; a union written across lines keeps its order. None when nothing moves, or
/// when the result would not be the same annotation reordered.
fn nil_first(comment: &Comment) -> Option<String> {
    let mut unions = luanalysis_syntax::nil_later_unions(comment);
    if unions.is_empty() { return None; }
    let base = comment.span.start;
    let mut text = comment.text.clone();
    // Innermost first: a union within another's member moves inside that member's text.
    unions.sort_by_key(|union| union.members.last().unwrap().end - union.members[0].start);
    for union in unions {
        let members: Vec<_> = union.members.iter().map(|span| (span.start - base, span.end - base)).collect();
        let separators: Vec<String> = members.windows(2).map(|pair| text.get(pair[0].1..pair[1].0).map(str::to_owned)).collect::<Option<_>>()?;
        if separators.iter().any(|separator| separator.contains(['\r', '\n'])) { continue; }
        let mut order = vec![union.nil];
        order.extend((0..members.len()).filter(|&index| index != union.nil));
        let mut moved = String::new();
        for (position, &index) in order.iter().enumerate() {
            if position > 0 { moved.push_str(&separators[position - 1]); }
            moved.push_str(text.get(members[index].0..members[index].1)?);
        }
        text.replace_range(members[0].0..members.last().unwrap().1, &moved);
    }
    let same_text = |text: &str| { let mut chars: Vec<_> = text.chars().filter(|ch| !ch.is_whitespace()).collect(); chars.sort_unstable(); chars };
    let diagnostics = |text: &str| luanalysis_syntax::parse(&format!("{text}\nlocal __formatter_comment_target = nil\n")).diagnostics.len();
    (text != comment.text && same_text(&text) == same_text(&comment.text)
        && relexed(&text).is_some_and(|after| after.kind == comment.kind)
        && diagnostics(&text) == diagnostics(&comment.text))
        .then_some(text)
}

/// The one comment `text` lexes as.
fn relexed(text: &str) -> Option<Comment> {
    let lexed = luanalysis_syntax::lex(text);
    let mut comments = lexed.tokens.into_iter().filter_map(|token| match token.kind {
        TokenKind::Comment(comment) => Some(comment),
        _ => None,
    });
    let comment = comments.next()?;
    (lexed.diagnostics.is_empty() && comments.next().is_none()).then_some(comment)
}

/// [`equivalent`], once `before`'s unions have their `nil` first, as the formatter writes them.
pub(super) fn equivalent_formatted(before: &Comment, after: &Comment) -> bool {
    match nil_first(before).and_then(|text| relexed(&text)) {
        Some(reordered) => equivalent(&reordered, after),
        None => equivalent(before, after),
    }
}

fn standalone(source: &str, start: usize) -> bool {
    let line = source[..start].rfind(['\r', '\n']).map_or(0, |at| at + 1);
    source[line..start].chars().all(|ch| matches!(ch, ' ' | '\t'))
}

fn ends_line(source: &str, end: usize) -> bool {
    let tail = &source[end..];
    let next = tail.find(['\r', '\n']).unwrap_or(tail.len());
    tail[..next].chars().all(|ch| matches!(ch, ' ' | '\t'))
}

fn columns(text: &str, tab_size: usize) -> usize {
    text.chars().fold(0, |column, ch| {
        if ch == '\t' { column + tab_size.max(1) - column % tab_size.max(1) } else { column + 1 }
    })
}

fn documentation(text: &str, indent: &str, options: &FormattingOptions, newline: &str) -> Option<String> {
    // Four or more dashes deliberately disable a tag or make a separator.
    if text.starts_with("----") { return None; }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<_> = normalized.split('\n').collect();
    let annotation = lines[0].strip_prefix("---")?.trim_start().starts_with('@');
    if annotation {
        let tag = lines[0][3..].trim_start().strip_prefix('@')?
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_').next()?;
        // Directives and unknown tags may assign meaning to physical lines.
        if !matches!(tag, "type" | "not" | "param" | "return" | "generic" | "vararg"
            | "alias" | "shape" | "class" | "module" | "field" | "function"
            | "overload" | "operator" | "as" | "cast" | "see" | "enum" | "deprecated")
        { return None; }
    }
    let mut output = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let body = line.trim_start_matches([' ', '\t']).strip_prefix("---")?;
        if body.starts_with('-') { return None; }
        let mut rendered = format!("---{body}");
        let prefix_width = columns(indent, options.tab_size) + 3;
        if options.max_line_length > 0
            && prefix_width + columns(body, options.tab_size) > options.max_line_length
            && (annotation || clear_prose(body))
        {
            let leading = &body[..body.len() - body.trim_start_matches([' ', '\t']).len()];
            let continuation = if annotation { "    " } else { leading };
            let wrapped = wrap(body, options.max_line_length.saturating_sub(prefix_width),
                continuation, annotation);
            rendered = wrapped.into_iter().map(|line| format!("---{line}"))
                .collect::<Vec<_>>().join(&format!("{newline}{indent}"));
        }
        if index > 0 { output.push(indent.to_owned()); }
        output.push(rendered);
        if index + 1 < lines.len() { output.push(newline.to_owned()); }
    }
    Some(output.concat())
}

fn prose(text: &str, indent: &str, options: &FormattingOptions, newline: &str) -> Option<String> {
    let body = text.strip_prefix("-- ")?;
    if !clear_prose(body) || columns(indent, options.tab_size) + columns(text, options.tab_size) <= options.max_line_length {
        return None;
    }
    let unit = if options.insert_spaces { " ".repeat(options.tab_size.max(1)) } else { "\t".to_owned() };
    let body_indent = format!("{indent}{unit}");
    let width = options.max_line_length.saturating_sub(columns(&body_indent, options.tab_size));
    let lines = wrap(body, width, "", false);
    if lines.len() < 2 { return None; }
    let mut equals = String::new();
    while body.contains(&format!("]{equals}]")) { equals.push('='); }
    Some(format!("--[{equals}[{newline}{body_indent}{}{newline}{indent}]{equals}]",
        lines.join(&format!("{newline}{body_indent}"))))
}

fn clear_prose(body: &str) -> bool {
    let trimmed = body.trim();
    let Some(first) = trimmed.chars().next() else { return false; };
    if !first.is_alphabetic() || trimmed.split_whitespace().count() < 5
        || body.starts_with([' ', '\t']) && body.len() - body.trim_start().len() > 1
        || trimmed.contains("SPDX") || trimmed.contains("Copyright")
        || trimmed.contains(" = ") || trimmed.contains(";")
        || trimmed.contains("```") || trimmed.contains("http://") || trimmed.contains("https://")
    { return false; }
    let first_word = trimmed.split_whitespace().next().unwrap_or_default().trim_end_matches(':').to_ascii_lowercase();
    !matches!(first_word.as_str(), "local" | "function" | "return" | "for" | "while" | "repeat"
        | "until" | "end" | "elseif" | "else" | "do" | "region" | "endregion"
        | "luacheck" | "stylua" | "selene" | "luacov" | "luacov:disable" | "luacov:enable")
}

/// Wrap at existing whitespace, and (for types) beside punctuation that already
/// separates tokens. Quoted literals and editor insertion hints remain intact.
fn wrap(text: &str, width: usize, continuation: &str, annotation: bool) -> Vec<String> {
    if width == 0 { return vec![text.to_owned()]; }
    let mut result = Vec::new();
    let mut remaining = text;
    let mut prefix = "";
    while prefix.chars().count() + remaining.chars().count() > width {
        let limit = width.saturating_sub(prefix.chars().count());
        let points = break_points(remaining, annotation);
        let chosen = points.iter().copied().filter(|&(end, _)| remaining[..end].chars().count() <= limit)
            .next_back().or_else(|| points.first().copied());
        let Some((end, next)) = chosen else { break; };
        if end == 0 || next >= remaining.len() || remaining[next..].trim_start().starts_with('@') { break; }
        result.push(format!("{prefix}{}", remaining[..end].trim_end()));
        remaining = remaining[next..].trim_start_matches([' ', '\t']);
        prefix = continuation;
    }
    result.push(format!("{prefix}{remaining}"));
    result
}

fn break_points(text: &str, annotation: bool) -> Vec<(usize, usize)> {
    let mut points = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut whitespace = None;
    let mut long_close: Option<String> = None;
    let mut long_end = 0;
    for (index, ch) in text.char_indices() {
        if let Some(close) = &long_close {
            if index < long_end { continue; }
            if text[index..].starts_with(close) {
                long_end = index + close.len();
                long_close = None;
            }
            continue;
        }
        if index < long_end { continue; }
        if let Some(open) = quote {
            if escaped { escaped = false; }
            else if ch == '\\' { escaped = true; }
            else if ch == open { quote = None; }
            continue;
        }
        if ch == '`' || annotation && (ch == '"' || ch == '\'' && !text[..index].ends_with(char::is_alphanumeric)) {
            if let Some(start) = whitespace.take() { points.push((start, index)); }
            quote = Some(ch);
            continue;
        }
        if annotation && ch == '[' {
            let tail = &text[index + 1..];
            let equals = tail.len() - tail.trim_start_matches('=').len();
            if tail[equals..].starts_with('[') {
                if let Some(start) = whitespace.take() { points.push((start, index)); }
                long_close = Some(format!("]{}]", "=".repeat(equals)));
                long_end = index + equals + 2;
                continue;
            }
        }
        if ch.is_whitespace() {
            whitespace.get_or_insert(index);
            continue;
        }
        if let Some(start) = whitespace.take() {
            if start > 0 { points.push((start, index)); }
        }
        if annotation && matches!(ch, ',' | '(' | '{' | '<') {
            let next = index + ch.len_utf8();
            points.push((next, next));
        } else if annotation && matches!(ch, '|' | '&') && index > 0
            // A `---|` line gives `#` a member-description meaning. Leave
            // existing trailing descriptions on a non-member continuation.
            && !(ch == '|' && text[index..].contains('#'))
        {
            points.push((index, index));
        }
    }
    points.sort_unstable();
    points.dedup();
    points
}

/// Comment text may differ only in whitespace outside quoted annotation tokens,
/// and a standalone ordinary comment may acquire long-comment delimiters.
/// The isolated parse additionally catches line-sensitive annotation changes,
/// such as a union member's `#` description or a newly started tag.
pub(super) fn equivalent(before: &Comment, after: &Comment) -> bool {
    let documentation = before.kind == CommentKind::Documentation;
    if documentation != (after.kind == CommentKind::Documentation)
        || !documentation && !matches!((before.kind, after.kind),
            (CommentKind::Line, CommentKind::Long) | (CommentKind::Line, CommentKind::Line)
            | (CommentKind::Long, CommentKind::Long))
    { return false; }
    let Some(before_body) = content(before) else { return false; };
    let Some(after_body) = content(after) else { return false; };
    let annotation = documentation && before_body.trim_start().starts_with('@');
    if canonical(&before_body, annotation) != canonical(&after_body, annotation) { return false; }
    if !documentation { return true; }
    annotation_shape(&before.text) == annotation_shape(&after.text)
        && documentation_shape(&before.text) == documentation_shape(&after.text)
}

fn content(comment: &Comment) -> Option<String> {
    if comment.text.starts_with("---") {
        return comment.text.replace("\r\n", "\n").replace('\r', "\n").split('\n')
            .map(|line| line.trim_start_matches([' ', '\t']).strip_prefix("---").map(str::to_owned))
            .collect::<Option<Vec<_>>>().map(|lines| lines.join("\n"));
    }
    let text = comment.text.strip_prefix("--")?;
    if comment.kind == CommentKind::Line { return Some(text.to_owned()); }
    let opening = text.strip_prefix('[')?;
    let equals = opening.len() - opening.trim_start_matches('=').len();
    let body = opening[equals..].strip_prefix('[')?;
    body.strip_suffix(&format!("]{}]", "=".repeat(equals))).map(str::to_owned)
}

fn canonical(body: &str, annotation: bool) -> String {
    if !annotation { return body.split_whitespace().collect::<Vec<_>>().join(" "); }
    let mut output = String::new();
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in body.char_indices() {
        if let Some(open) = quote {
            output.push(ch);
            if escaped { escaped = false; }
            else if ch == '\\' { escaped = true; }
            else if ch == open { quote = None; }
        } else if matches!(ch, '"' | '`') || ch == '\'' && !body[..index].ends_with(char::is_alphanumeric) {
            quote = Some(ch);
            output.push(ch);
        } else if !ch.is_whitespace() { output.push(ch); }
    }
    output
}

fn annotation_shape(comment: &str) -> String {
    let mut file = luanalysis_syntax::parse(&format!("{comment}\nlocal __formatter_comment_target = nil\n"));
    file.comments.clear();
    without_spans(&format!("{file:?}"))
}

// Debug prints every syntax field, including overload/owner IDs and completion
// metadata. Ignore only the exact derived representation of byte-offset spans;
// quoted strings are copied verbatim, so their contents cannot impersonate spans.
fn without_spans(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if rest.starts_with("Span { start: ") {
            if let Some(end) = rest.find(" }") {
                let span = &rest[14..end];
                if span.chars().all(|ch| ch.is_ascii_digit() || ", end: ".contains(ch)) {
                    output.push_str("Span");
                    rest = &rest[end + 2..];
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        output.push(ch);
        rest = &rest[ch.len_utf8()..];
        if ch == '"' {
            let mut escaped = false;
            for (index, next) in rest.char_indices() {
                output.push(next);
                if escaped { escaped = false; }
                else if next == '\\' { escaped = true; }
                else if next == '"' { rest = &rest[index + 1..]; break; }
            }
        }
    }
    output
}

fn documentation_shape(comment: &str) -> luanalysis_syntax::Documentation {
    let source = format!("{comment}\nlocal __formatter_comment_target = nil\n");
    let file = luanalysis_syntax::parse(&source);
    let mut documentation = luanalysis_syntax::documentation_above(&file, &source, source.rfind("local ").unwrap());
    fn prose(text: &mut String) { *text = text.split_whitespace().collect::<Vec<_>>().join(" "); }
    prose(&mut documentation.description);
    for (_, text) in &mut documentation.params { prose(text); }
    for (_, text) in &mut documentation.fields { prose(text); }
    for text in &mut documentation.returns { prose(text); }
    if let Some(text) = &mut documentation.deprecated { prose(text); }
    documentation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(source: &str, width: usize) -> String {
        let lexed = luanalysis_syntax::lex(source);
        let tokens: Vec<_> = lexed.tokens.iter().collect();
        let options = FormattingOptions { max_line_length: width, ..FormattingOptions::default() };
        let mut output = source.to_owned();
        for edit in edits(source, &tokens, &options, Span::new(0, source.len()), "\n", |_| String::new()).into_iter().rev() {
            output.replace_range(edit.span.start..edit.span.end, &edit.new_text);
        }
        output
    }

    #[test]
    fn a_unions_nil_moves_first_keeping_each_members_text_and_the_separators() {
        assert_eq!(rewrite("---@param x string|nil the value, or nil\n", 0), "---@param x nil|string the value, or nil\n");
        assert_eq!(rewrite("---@type A | B | nil | C\n", 0), "---@type nil | A | B | C\n");
        assert_eq!(rewrite("---@type (string | nil)[] | nil\n", 0), "---@type nil | (nil | string)[]\n");
        assert_eq!(rewrite("---@type fun(x: number|nil): string | nil\n", 0), "---@type fun(x: nil|number): nil | string\n");
        assert_eq!(rewrite("---@param done fun(deck: Deck) | nil\n", 0), "---@param done nil | fun(deck: Deck)\n");
        assert_eq!(rewrite("---@alias Json Container | number | nil | Null\n", 0), "---@alias Json nil | Container | number | Null\n");
        assert_eq!(rewrite("---@type number | string | nil, number\n", 0), "---@type nil | number | string, number\n");
        // A `T?` writes no nil, and nil first moves nothing.
        for unchanged in ["---@return string?\n", "---@type nil | string\n", "---@param x? string\n"] {
            assert_eq!(rewrite(unchanged, 0), unchanged);
        }
    }

    #[test]
    fn a_reordered_union_still_wraps() {
        let source = "---@param value string|number|nil a value that takes up enough room to need wrapping here\n";
        let wrapped = rewrite(source, 40);
        assert!(wrapped.starts_with("---@param value nil|string|number"), "{wrapped}");
        assert!(wrapped.lines().count() > 1, "{wrapped}");
    }

    #[test]
    fn overloads_wrap_without_losing_generic_or_pack_structure() {
        let source = "---@overload fun<V, CovariantA : V, CovariantB : V>(arr: V[], a: CovariantA[], b: CovariantB[]): (V, number) | (nil, string)\nlocal function select(arr, a, b) end\n";
        let result = rewrite(source, 65);
        assert!(result.contains("\n---    "), "{result}");
        assert!(result.lines().all(|line| line.chars().count() <= 65), "{result}");
        assert_eq!(rewrite(&result, 65), result);
        let before = luanalysis_syntax::parse(source);
        let after = luanalysis_syntax::parse(&result);
        assert_eq!(before.overload_annotations.len(), 1);
        assert_eq!(after.overload_annotations[0].signature.generics.len(), 3);
        assert_eq!(after.overload_annotations[0].signature.params.len(), 3);
        assert!(after.overload_annotations[0].signature.packs.is_some());
    }

    #[test]
    fn prose_becomes_one_long_comment_and_preserves_words() {
        let source = "-- This module operates on tables that contain only positive consecutive integer keys starting at one.\nlocal t = {}\n";
        let result = rewrite(source, 50);
        assert!(result.starts_with("--[[\n    This module"), "{result}");
        assert!(result.contains("\n]]\nlocal"));
        assert_eq!(rewrite(&result, 50), result);
        assert_eq!(luanalysis_syntax::parse(&result).comments.len(), 1);
    }

    #[test]
    fn quoted_literals_hints_and_structured_comments_are_preserved() {
        let source = "---@type 'a very long quoted string that must remain one exact literal' | `a completion hint with spaces` | number\nlocal value\n";
        let result = rewrite(source, 55);
        assert!(result.contains("'a very long quoted string that must remain one exact literal'"));
        assert!(result.contains("`a completion hint with spaces`"));
        for source in [
            "-- local x = one + two + three + four + five + six + seven + eight\n",
            "-- Copyright the owner of this library and all its many contributors\n",
            "-- region a long title containing many words that must stay on one physical line\n",
            "-- endregion a long title containing many words that must stay on one physical line\n",
            "-- luacheck: globals these names must stay on their original line for the linter\n",
            "-- stylua: ignore this declaration and all the text after this directive\n",
            "-- selene: allow these names in the statement on the following physical line\n",
            "---@diagnostic disable-next-line: missing-fields, undefined-global, unused-local\nlocal x\n",
            "local x = nil -- This inline comment contains enough words that it exceeds the configured width\n",
        ] { assert_eq!(rewrite(source, 40), source); }
    }

    #[test]
    fn union_description_and_annotation_attachment_cannot_change() {
        let before = luanalysis_syntax::lex("---@type string | number # a description which belongs to the annotation\n");
        let after = luanalysis_syntax::lex("---@type string\n---| number # a description which belongs to the annotation\n");
        let TokenKind::Comment(before) = &before.tokens[0].kind else { panic!() };
        let TokenKind::Comment(after) = &after.tokens[0].kind else { panic!() };
        assert!(!equivalent(before, after));
    }

    #[test]
    fn derived_debug_strings_cannot_impersonate_spans() {
        assert_eq!(without_spans(r#"Name { text: "Span { start: 10, end: 20 }", span: Span { start: 4, end: 7 } }"#),
            r#"Name { text: "Span { start: 10, end: 20 }", span: Span }"#);
    }

    #[test]
    fn comment_ranges_and_continuation_indentation_are_strict() {
        let source = "    ---@param callback fun(\r\n---    value: number\r\n---): boolean\r\n    local function each(callback) end\r\n";
        let lexed = luanalysis_syntax::lex(source);
        let tokens: Vec<_> = lexed.tokens.iter().collect();
        let options = FormattingOptions::default();
        let result = edits(source, &tokens, &options, Span::new(0, source.len()), "\r\n", |_| "    ".to_owned());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].new_text, "---@param callback fun(\r\n    ---    value: number\r\n    ---): boolean");
        assert!(edits(source, &tokens, &options, Span::new(5, source.len()), "\r\n", |_| "    ".to_owned()).is_empty());
    }

    #[test]
    fn ordinary_block_delimiters_do_not_close_inside_prose() {
        let source = "-- This text discusses the ]] delimiter and has enough words to need another line without becoming executable code.\nlocal value\n";
        let result = rewrite(source, 55);
        assert!(result.starts_with("--[=[\n"), "{result}");
        assert!(!luanalysis_syntax::parse(&result).has_lua_syntax_errors());
    }
}
