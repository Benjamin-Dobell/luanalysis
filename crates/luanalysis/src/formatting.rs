//! Source-preserving Lua formatting shared by every editor adapter.
//!
//! Executable tokens and literals retain their exact spelling. Standalone long
//! comments may wrap after verifying their content and native annotation structure.

mod comments;

use std::collections::{BTreeMap, BTreeSet};
use luanalysis_syntax::{Expr, ExprKind, Function, Span, Stmt, StmtKind, TableFieldKind, Token, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Newline {
    Lf,
    CrLf,
    Cr,
}

impl Newline {
    fn text(self) -> &'static str {
        match self { Self::Lf => "\n", Self::CrLf => "\r\n", Self::Cr => "\r" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormattingOptions {
    pub tab_size: usize,
    pub insert_spaces: bool,
    /// Legacy formatter option; existing line breaks are always retained.
    pub keep_multiple_statements_in_one_line: bool,
    /// Width for standalone prose/annotation wrapping; zero disables wrapping.
    pub max_line_length: usize,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub trim_final_newlines: Option<bool>,
    /// Absent: retain the first line ending in the buffer (LF for a new file).
    pub newline: Option<Newline>,
}

impl Default for FormattingOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            keep_multiple_statements_in_one_line: false,
            max_line_length: 120,
            trim_trailing_whitespace: None,
            insert_final_newline: None,
            trim_final_newlines: None,
            newline: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatEdit {
    pub span: Span,
    pub new_text: String,
}

/// Format a complete, syntactically valid Lua buffer. Annotation errors are harmless:
/// comments remain unchanged unless equivalent wrapping is proved. A range bounds every edit strictly; it does
/// not expand to surrounding statements or lines. `None` means unsafe/invalid input.
///
/// Returned edits are ordered, non-overlapping, and use original UTF-8 byte offsets.
/// Hosts must map these offsets using the negotiated LSP position encoding and reject
/// the response if their document snapshot changed while it was computed.
pub fn format(source: &str, options: &FormattingOptions, range: Option<Span>) -> Option<Vec<FormatEdit>> {
    let bounds = range.unwrap_or(Span::new(0, source.len()));
    if bounds.start > bounds.end || bounds.end > source.len()
        || !source.is_char_boundary(bounds.start) || !source.is_char_boundary(bounds.end)
    {
        return None;
    }
    if range.is_some() && bounds.start == bounds.end { return Some(Vec::new()); }
    let lexed = luanalysis_syntax::lex(source);
    if !lexed.diagnostics.is_empty() { return None; }
    let file = luanalysis_syntax::parse(source);
    if file.has_lua_syntax_errors() { return None; }
    let tokens: Vec<_> = lexed.tokens.iter().filter(|token| significant(token)).collect();
    let mut layout = Layout {
        source,
        keep_multiple: options.keep_multiple_statements_in_one_line,
        ..Layout::default()
    };
    layout.statements(&file.statements, false);
    layout.delimiters(source, &tokens);
    let newline = newline(source, options).text();
    let mut edits = Vec::new();
    let mut previous: Option<&Token> = None;
    for &token in &tokens {
        let start = previous.map_or(0, |token| token.span.end);
        let gap = &source[start..token.span.start];
        // Lexer trivia must contain only whitespace. Keeping this guard makes a future
        // lexer extension safe without silently deleting an unrecognised source fragment.
        if !gap.chars().all(char::is_whitespace) { return None; }
        let indent = layout.indent_at(token.span.start);
        let protected_inline_doc = !has_newline(gap)
            && (documentation(token) || previous.is_some_and(documentation));
        let leading_semicolon = previous.is_some_and(|left| spelling(source, left) == ";"
            && spelling(source, token) == "("
            && source[source[..left.span.start].rfind(['\r', '\n']).map_or(0, |at| at + 1)..left.span.start].trim().is_empty());
        let required_break = !protected_inline_doc && !leading_semicolon && previous.is_some()
            && (layout.break_before.contains(&token.span.start)
                || previous.is_some_and(line_comment));
        let replacement = if has_newline(gap) || required_break {
            let count = newline_count(gap).max(1);
            line_gap(gap, count, indent, newline, options)
        } else if let Some(left) = previous {
            if spaced(source, left, token, &layout) { " ".to_owned() } else { String::new() }
        } else {
            indentation(indent, options)
        };
        push_edit(&mut edits, source, Span::new(start, token.span.start), replacement, bounds);
        previous = Some(token);
    }
    let tail_start = previous.map_or(0, |token| token.span.end);
    let tail = &source[tail_start..];
    if !tail.chars().all(char::is_whitespace) { return None; }
    let replacement = final_gap(tail, newline, options);
    push_edit(&mut edits, source, Span::new(tail_start, source.len()), replacement, bounds);
    edits.extend(comments::edits(source, &tokens, options, bounds, newline,
        |at| indentation(layout.indent_at(at), options)));
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
    // Lexical equivalence is checked after applying the actual bounded edits, not just
    // against the proposed full formatting. This catches token fusion at range edges.
    let formatted = apply_edits(source, &edits);
    let after = luanalysis_syntax::lex(&formatted);
    let after_tokens: Vec<_> = after.tokens.iter().filter(|token| significant(token)).collect();
    if !after.diagnostics.is_empty() || tokens.len() != after_tokens.len()
        || tokens.iter().zip(after_tokens.iter()).any(|(before, after)| {
            if source[before.span.start..before.span.end] == formatted[after.span.start..after.span.end] {
                return false;
            }
            match (&before.kind, &after.kind) {
                (TokenKind::Comment(before), TokenKind::Comment(after)) => !comments::equivalent(before, after),
                _ => true,
            }
        })
        || luanalysis_syntax::parse(&formatted).has_lua_syntax_errors()
    {
        return None;
    }
    Some(edits)
}

/// Safe on-type formatting supports the newline trigger only. It changes leading
/// indentation on the caret's line, including a blank line in an unfinished block.
/// It never inserts `end`, `until`, quotes, or other source tokens. Unterminated
/// strings/comments and a caret inside a multiline token produce no edits.
pub fn on_type(source: &str, offset: usize, trigger: &str, options: &FormattingOptions) -> Option<Vec<FormatEdit>> {
    if trigger != "\n" || offset > source.len() || !source.is_char_boundary(offset) { return None; }
    let lexed = luanalysis_syntax::lex(source);
    if !lexed.diagnostics.is_empty() { return Some(Vec::new()); }
    let tokens: Vec<_> = lexed.tokens.iter().filter(|token| significant(token)).collect();
    if tokens.iter().any(|token| token.span.start < offset && offset < token.span.end) {
        return Some(Vec::new());
    }
    let start = source[..offset].rfind(['\r', '\n']).map_or(0, |position| position + 1);
    // An LSP newline request is made after the line ending, never between CR and LF.
    if source.as_bytes().get(offset) == Some(&b'\n') && source.as_bytes().get(offset.wrapping_sub(1)) == Some(&b'\r') {
        return Some(Vec::new());
    }
    let end = start + source[start..].find(|ch| ch != ' ' && ch != '\t').unwrap_or(source.len() - start);
    if offset > end || tokens.iter().any(|token| token.span.start < start && start < token.span.end) {
        return Some(Vec::new());
    }
    let file = luanalysis_syntax::parse(source);
    let depth = if !file.has_lua_syntax_errors() {
        let mut layout = Layout { source, ..Layout::default() };
        layout.statements(&file.statements, false);
        layout.delimiters(source, &tokens);
        layout.indent_at(end)
    } else {
        incomplete_indent(source, &tokens, start)
    };
    let replacement = indentation(depth, options);
    let mut edits = Vec::new();
    push_edit(&mut edits, source, Span::new(start, end), replacement, Span::new(0, source.len()));
    Some(edits)
}

fn significant(token: &Token) -> bool {
    !matches!(token.kind, TokenKind::Newline | TokenKind::Eof)
}

fn documentation(token: &Token) -> bool {
    matches!(&token.kind, TokenKind::Comment(comment) if comment.kind == luanalysis_syntax::CommentKind::Documentation)
}

fn line_comment(token: &Token) -> bool {
    matches!(&token.kind, TokenKind::Comment(comment) if matches!(comment.kind,
        luanalysis_syntax::CommentKind::Line | luanalysis_syntax::CommentKind::Shebang)
        || (comment.kind == luanalysis_syntax::CommentKind::Documentation && !comment.text.starts_with("--[")))
}

fn spelling<'a>(source: &'a str, token: &Token) -> &'a str {
    &source[token.span.start..token.span.end]
}

fn spaced(source: &str, left: &Token, right: &Token, layout: &Layout<'_>) -> bool {
    let a = spelling(source, left);
    let b = spelling(source, right);
    if layout.space_before.contains(&right.span.start) { return true; }
    if a == "<" && layout.attributes.contains(&right.span.start)
        || b == ">" && layout.attributes.contains(&left.span.start)
    { return false; }
    if a == "::" || b == "::" { return false; }
    if b == "," || b == ";" || b == ")" || b == "]" { return false; }
    if a == "," || a == ";" { return a != ";" || b != "("; }
    if a == "(" { return false; }
    // Authored Lua uses both {x = 1} and { x = 1 }. Preserve each existing
    // interior edge while normalising the operators/fields within the table.
    if a == "{" { return !source[left.span.end..right.span.start].is_empty(); }
    // `[ [=[x]=] ]` must not become a different long-string delimiter.
    if a == "[" { return b.starts_with('['); }
    if b == "[" { return false; }
    if a == "." || b == "." || a == ":" || b == ":" { return false; }
    if b == "}" { return !source[left.span.end..right.span.start].is_empty(); }
    if matches!(left.kind, TokenKind::Comment(_)) || matches!(right.kind, TokenKind::Comment(_)) { return true; }
    if layout.unary.contains(&left.span.start) {
        return a == "not" || a == "-" && b.starts_with('-');
    }
    if b == "(" {
        return matches!(&left.kind, TokenKind::Keyword(word) if word != "function")
            || a == "=" || layout.binary.contains(&left.span.start);
    }
    if layout.binary.contains(&left.span.start) || layout.binary.contains(&right.span.start)
        || a == "=" || b == "="
        || matches!(a, "<" | ">") || matches!(b, "<" | ">")
    { return true; }
    // Lua's shorthand calls need a gap (`print "text"`, `f { ... }`).
    if matches!(right.kind, TokenKind::String(_)) || b == "{" { return true; }
    matches!(left.kind, TokenKind::Identifier(_) | TokenKind::Keyword(_) | TokenKind::Number(_) | TokenKind::String(_))
        && matches!(right.kind, TokenKind::Identifier(_) | TokenKind::Keyword(_) | TokenKind::Number(_) | TokenKind::String(_))
        || matches!(left.kind, TokenKind::Keyword(_)) || matches!(right.kind, TokenKind::Keyword(_))
}

#[derive(Default)]
struct Layout<'a> {
    source: &'a str,
    keep_multiple: bool,
    blocks: Vec<Span>,
    continuations: Vec<Span>,
    function_ends: BTreeMap<usize, usize>,
    indent_starts: Vec<usize>,
    indent_ends: Vec<usize>,
    break_before: BTreeSet<usize>,
    space_before: BTreeSet<usize>,
    unary: BTreeSet<usize>,
    binary: BTreeSet<usize>,
    attributes: BTreeSet<usize>,
}

impl Layout<'_> {
    fn indent_at(&self, offset: usize) -> usize {
        self.indent_starts.partition_point(|start| *start <= offset)
            .saturating_sub(self.indent_ends.partition_point(|end| *end <= offset))
    }

    fn delimiters(&mut self, source: &str, tokens: &[&Token]) {
        let mut stack: Vec<usize> = Vec::new();
        let mut closes = BTreeMap::new();
        for (index, token) in tokens.iter().enumerate() {
            if !matches!(token.kind, TokenKind::Symbol(_)) { continue; }
            match spelling(source, token) {
                "(" | "[" | "{" => stack.push(index),
                ")" | "]" | "}" => {
                    if let Some(open) = stack.pop() { closes.insert(open, index); }
                }
                _ => {}
            }
        }
        for (&open, &close) in &closes {
            let left = tokens[open];
            let right = tokens[close];
            if spelling(source, left) == "{" {
                // A table owns its fields' indentation. A surrounding call such as
                // `setmetatable(value, { ... })` must not add another level.
                self.continuations.push(Span::new(left.span.end, right.span.start));
                continue;
            }
            let mut continuation = None;
            if let Some(next) = tokens.get(open + 1).filter(|_| open + 1 < close) {
                if has_newline(&source[left.span.end..next.span.start]) {
                    continuation = Some(left.span.end);
                }
            }
            // Further arguments beginning on their own line also form a list.
            // Ignore delimiters and function bodies within an argument; their own
            // structural layout is responsible for those internal line breaks.
            let mut index = open + 1;
            while index < close {
                let token = tokens[index];
                if let Some(end) = self.function_ends.get(&token.span.start) {
                    index = tokens.partition_point(|token| token.span.start < *end);
                    continue;
                }
                if let Some(nested_close) = closes.get(&index) {
                    index = *nested_close + 1;
                    continue;
                }
                if matches!(spelling(source, token), "," | ";") && index + 1 < close
                    && has_newline(&source[token.span.end..tokens[index + 1].span.start])
                {
                    continuation = Some(continuation.map_or(token.span.end, |start: usize| start.min(token.span.end)));
                }
                index += 1;
            }
            if let Some(start) = continuation {
                self.continuations.push(Span::new(start, right.span.start));
            }
        }
        // A file can contain thousands of unrelated blocks. Prefix counts keep
        // indentation queries logarithmic instead of scanning every block per token.
        self.indent_starts = self.blocks.iter().chain(&self.continuations).map(|span| span.start).collect();
        self.indent_ends = self.blocks.iter().chain(&self.continuations).map(|span| span.end).collect();
        self.indent_starts.sort_unstable();
        self.indent_ends.sort_unstable();
    }

    fn body(&mut self, statements: &[Stmt], span: Span) {
        self.blocks.push(span);
        self.break_before.insert(span.end);
        self.statements(statements, true);
    }

    fn statements(&mut self, statements: &[Stmt], nested: bool) {
        for (index, statement) in statements.iter().enumerate() {
            if nested && index == 0 || index > 0 && !self.keep_multiple {
                self.break_before.insert(statement.span.start);
            }
            self.statement(statement);
        }
    }

    fn function(&mut self, function: &Function, anonymous: bool) {
        self.function_ends.insert(function.span.start, function.span.end);
        // Short callbacks and dispatch functions are intentional authored layout.
        // Named functions and compound bodies still receive normal block layout.
        let compact = anonymous && !has_newline(&self.source[function.span.start..function.span.end])
            && function.body.len() <= 1 && function.body.iter().all(simple_statement);
        if compact {
            if let Some(first) = function.body.first() { self.space_before.insert(first.span.start); }
            self.statements(&function.body, false);
        } else {
            self.body(&function.body, function.body_span);
        }
    }

    fn statement(&mut self, statement: &Stmt) {
        match &statement.kind {
            StmtKind::Local { attributes, values, .. } => {
                self.attributes.extend(attributes.iter().flatten().map(|name| name.span.start));
                for value in values { self.expr(value); }
            }
            StmtKind::Assign { targets, values, .. } => {
                for expression in targets.iter().chain(values) { self.expr(expression); }
            }
            StmtKind::LocalFunction { function, .. } => self.function(function, false),
            StmtKind::FunctionDeclaration { target, function, .. } => { self.expr(target); self.function(function, false); }
            StmtKind::Block { body, body_span } => self.body(body, *body_span),
            StmtKind::Return(values) => { for value in values { self.expr(value); } }
            StmtKind::Call(expression) => self.expr(expression),
            StmtKind::If { branches, else_branch } => {
                for (index, branch) in branches.iter().enumerate() {
                    if index > 0 { self.break_before.insert(branch.span.start); }
                    self.expr(&branch.condition);
                    self.body(&branch.body, branch.body_span);
                }
                if let Some(branch) = else_branch {
                    self.break_before.insert(branch.span.start);
                    self.body(&branch.body, branch.body_span);
                }
            }
            StmtKind::While { condition, body, body_span }
            | StmtKind::Repeat { condition, body, body_span } => {
                self.expr(condition);
                self.body(body, *body_span);
            }
            StmtKind::NumericFor { start, limit, step, body, body_span, .. } => {
                self.expr(start); self.expr(limit);
                if let Some(step) = step { self.expr(step); }
                self.body(body, *body_span);
            }
            StmtKind::GenericFor { values, body, body_span, .. } => {
                for value in values { self.expr(value); }
                self.body(body, *body_span);
            }
            StmtKind::Goto(_) | StmtKind::Label(_) | StmtKind::Break => {}
        }
    }

    fn expr(&mut self, expression: &Expr) {
        match &expression.kind {
            ExprKind::Unary { op_span, value, .. } => { self.unary.insert(op_span.start); self.expr(value); }
            ExprKind::Binary { .. } => {
                let mut start = None;
                self.binary_chain(expression, &mut start);
                if let Some(start) = start { self.continuations.push(Span::new(start, expression.span.end)); }
            }
            ExprKind::Parenthesized(value) => self.expr(value),
            ExprKind::Field { .. } | ExprKind::Index { .. }
            | ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                let mut start = None;
                self.postfix_chain(expression, &mut start);
                if let Some(start) = start { self.continuations.push(Span::new(start, expression.span.end)); }
            }
            ExprKind::Function(function) => self.function(function, true),
            ExprKind::Table(fields) => {
                for field in fields {
                    if let TableFieldKind::Keyed(key) = &field.kind { self.expr(key); }
                    self.expr(&field.value);
                }
            }
            ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_)
            | ExprKind::Name(_) | ExprKind::Vararg | ExprKind::Error => {}
        }
    }

    fn postfix_chain(&mut self, expression: &Expr, continuation: &mut Option<usize>) {
        match &expression.kind {
            ExprKind::Field { base, name } => {
                if has_newline(&self.source[base.span.end..name.span.start]) {
                    *continuation = Some(continuation.map_or(base.span.end, |old| old.min(base.span.end)));
                }
                self.postfix_chain(base, continuation);
            }
            ExprKind::MethodCall { receiver, name, args } => {
                if has_newline(&self.source[receiver.span.end..name.span.start]) {
                    *continuation = Some(continuation.map_or(receiver.span.end, |old| old.min(receiver.span.end)));
                }
                self.postfix_chain(receiver, continuation);
                for arg in args { self.expr(arg); }
            }
            ExprKind::Call { callee, args } => {
                self.postfix_chain(callee, continuation);
                for arg in args { self.expr(arg); }
            }
            ExprKind::Index { base, index } => {
                self.postfix_chain(base, continuation);
                self.expr(index);
            }
            _ => self.expr(expression),
        }
    }

    fn binary_chain(&mut self, expression: &Expr, continuation: &mut Option<usize>) {
        if let ExprKind::Binary { op_span, left, right, .. } = &expression.kind {
            self.binary.insert(op_span.start);
            for (gap, start) in [(Span::new(left.span.end, op_span.start), left.span.end),
                (Span::new(op_span.end, right.span.start), op_span.end)]
            {
                if has_newline(&self.source[gap.start..gap.end]) {
                    *continuation = Some(continuation.map_or(start, |old| old.min(start)));
                }
            }
            self.binary_chain(left, continuation);
            self.binary_chain(right, continuation);
        } else {
            self.expr(expression);
        }
    }
}

fn simple_statement(statement: &Stmt) -> bool {
    match &statement.kind {
        StmtKind::Return(values) | StmtKind::Local { values, .. } => values.iter().all(simple_expression),
        StmtKind::Assign { targets, values, .. } => targets.iter().chain(values).all(simple_expression),
        StmtKind::Call(expression) => simple_expression(expression),
        _ => false,
    }
}

fn simple_expression(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Function(_) | ExprKind::Error => false,
        ExprKind::Parenthesized(value) | ExprKind::Unary { value, .. } => simple_expression(value),
        ExprKind::Binary { left, right, .. } => simple_expression(left) && simple_expression(right),
        ExprKind::Field { base, .. } => simple_expression(base),
        ExprKind::Index { base, index } => simple_expression(base) && simple_expression(index),
        ExprKind::Call { callee, args } => simple_expression(callee) && args.iter().all(simple_expression),
        ExprKind::MethodCall { receiver, args, .. } => simple_expression(receiver) && args.iter().all(simple_expression),
        ExprKind::Table(fields) => fields.iter().all(|field| {
            (!matches!(&field.kind, TableFieldKind::Keyed(key) if !simple_expression(key)))
                && simple_expression(&field.value)
        }),
        _ => true,
    }
}

fn indentation(depth: usize, options: &FormattingOptions) -> String {
    // Bound hostile client settings without changing a valid tabSize of zero into an
    // allocation hazard. Lua's native parser already bounds syntactic nesting.
    if options.insert_spaces { " ".repeat(depth.saturating_mul(options.tab_size.clamp(1, 32))) }
    else { "\t".repeat(depth) }
}

fn has_newline(text: &str) -> bool { text.contains(['\r', '\n']) }

fn newline_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().filter(|&(index, byte)| *byte == b'\n' || *byte == b'\r' && bytes.get(index + 1) != Some(&b'\n')).count()
}

fn newline(source: &str, options: &FormattingOptions) -> Newline {
    options.newline.unwrap_or_else(|| {
        let bytes = source.as_bytes();
        match bytes.iter().position(|byte| matches!(byte, b'\r' | b'\n')) {
            Some(index) if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') => Newline::CrLf,
            Some(index) if bytes[index] == b'\r' => Newline::Cr,
            _ => Newline::Lf,
        }
    })
}

fn line_gap(original: &str, count: usize, depth: usize, newline: &str, options: &FormattingOptions) -> String {
    let mut output = String::new();
    if options.trim_trailing_whitespace == Some(false) {
        let bytes = original.as_bytes();
        let mut start = 0;
        let mut position = 0;
        while position < bytes.len() {
            if matches!(bytes[position], b'\r' | b'\n') {
                output.push_str(&original[start..position]);
                output.push_str(newline);
                if bytes[position] == b'\r' && bytes.get(position + 1) == Some(&b'\n') { position += 1; }
                position += 1;
                start = position;
            } else { position += 1; }
        }
        if !has_newline(original) { output.push_str(newline); }
    } else {
        output.push_str(&newline.repeat(count));
    }
    output.push_str(&indentation(depth, options));
    output
}

fn final_gap(original: &str, newline: &str, options: &FormattingOptions) -> String {
    let mut count = newline_count(original);
    if options.trim_final_newlines == Some(true) { count = count.min(1); }
    if options.insert_final_newline == Some(true) { count = count.max(1); }
    if options.trim_trailing_whitespace == Some(false) && options.trim_final_newlines != Some(true) {
        if count == 0 && options.insert_final_newline == Some(true) {
            return format!("{original}{newline}");
        }
        // Normalize existing line endings while retaining explicitly requested spaces.
        return original.replace("\r\n", "\n").replace('\r', "\n").replace('\n', newline);
    }
    newline.repeat(count)
}

fn push_edit(edits: &mut Vec<FormatEdit>, source: &str, span: Span, new_text: String, bounds: Span) {
    let old = &source[span.start..span.end];
    if old == new_text { return; }
    // Retain unchanged line endings at selection boundaries. A selection beginning at
    // the start of a line can then format that line's indentation without editing the
    // preceding newline, which lies outside the requested range.
    let mut prefix = old.chars().zip(new_text.chars()).take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8()).sum::<usize>();
    // CRLF is one editor line ending. Its midpoint has no representable LSP
    // position, even though it is a UTF-8 character boundary. Retain/rewrite the
    // complete pair when shrinking edits, including LF <-> CRLF conversion.
    if splits_crlf(old, prefix) || splits_crlf(&new_text, prefix) { prefix -= 1; }
    let mut suffix = old[prefix..].chars().rev().zip(new_text[prefix..].chars().rev())
        .take_while(|(a, b)| a == b).map(|(ch, _)| ch.len_utf8()).sum::<usize>();
    if splits_crlf(old, old.len() - suffix) || splits_crlf(&new_text, new_text.len() - suffix) { suffix -= 1; }
    let span = Span::new(span.start + prefix, span.end - suffix);
    if span.start >= bounds.start && span.end <= bounds.end {
        edits.push(FormatEdit { span, new_text: new_text[prefix..new_text.len() - suffix].to_owned() });
    }
}

fn splits_crlf(text: &str, offset: usize) -> bool {
    offset > 0 && text.as_bytes().get(offset - 1) == Some(&b'\r')
        && text.as_bytes().get(offset) == Some(&b'\n')
}

fn apply_edits(source: &str, edits: &[FormatEdit]) -> String {
    let mut result = String::with_capacity(source.len());
    let mut start = 0;
    for edit in edits {
        result.push_str(&source[start..edit.span.start]);
        result.push_str(&edit.new_text);
        start = edit.span.end;
    }
    result.push_str(&source[start..]);
    result
}

/// Incomplete buffers cannot supply reliable AST block spans. Token balancing here is
/// used only to indent a whitespace-only line, never to make structural edits.
fn incomplete_indent(source: &str, tokens: &[&Token], line_start: usize) -> usize {
    let mut depth = 0usize;
    let mut pending_do = 0usize;
    let mut pending_then = 0usize;
    let mut pending_function_params = false;
    let mut block_depth = 0usize;
    // A comma belongs to this delimiter only while still in the block where
    // it opened. Commas in an inline callback's local/return statements must
    // not activate the caller's argument-list continuation.
    let mut delimiters: Vec<(&str, bool, usize)> = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate().filter(|(_, token)| token.span.start < line_start) {
        let gap_end = tokens.get(index + 1).map_or(line_start, |next| next.span.start.min(line_start));
        let next_line = has_newline(&source[token.span.end..gap_end]);
        match spelling(source, token) {
            "function" => {
                // Parameters are a list, not yet the function's body. Enter its
                // scope for comma ownership, but activate body indentation only
                // after the matching parameter ')' has closed.
                block_depth += 1;
                pending_function_params = true;
            }
            "repeat" => { depth += 1; block_depth += 1; }
            "for" | "while" => { depth += 1; block_depth += 1; pending_do += 1; }
            "if" => { depth += 1; block_depth += 1; pending_then += 1; }
            "then" if pending_then > 0 => pending_then -= 1,
            "do" if pending_do > 0 => pending_do -= 1,
            "do" => { depth += 1; block_depth += 1; }
            "(" | "[" | "{" => {
                let text = spelling(source, token);
                let parameters = text == "(" && pending_function_params;
                if parameters { pending_function_params = false; }
                let indent = text == "{" || next_line;
                delimiters.push((if parameters { "params" } else { text }, indent, block_depth));
                depth += usize::from(indent);
            }
            "," | ";" if next_line => {
                if let Some((_, indent, owner)) = delimiters.last_mut() {
                    if *owner == block_depth && !*indent { *indent = true; depth += 1; }
                }
            }
            ")" | "]" | "}" => {
                if let Some((kind, indent, _)) = delimiters.pop() {
                    depth = depth.saturating_sub(usize::from(indent));
                    if kind == "params" && spelling(source, token) == ")" { depth += 1; }
                }
            }
            "end" | "until" => {
                depth = depth.saturating_sub(1);
                block_depth = block_depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    if let Some(next) = tokens.iter().copied().find(|token| token.span.start >= line_start) {
        if matches!(spelling(source, next), "end" | "until" | "else" | "elseif") {
            depth = depth.saturating_sub(1);
        } else if matches!(spelling(source, next), ")" | "]" | "}") {
            depth = depth.saturating_sub(usize::from(delimiters.last().is_some_and(|(_, indent, _)| *indent)));
        }
    }
    depth
}
