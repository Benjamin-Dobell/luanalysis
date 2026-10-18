//! What the language server asks beyond checking. As tsgo answers a completion from its checker
//! on the current program, a request runs one native session over the project with the editor's
//! text and evaluates only what the answer needs.
//!
//! An editor's text is usually incomplete where it is being typed (`player.`), and only a source
//! without syntax errors is bound and checked. The word at the cursor is therefore replaced with a
//! placeholder name, and if that does not parse, the text is repaired around it: a call is added
//! (`player.placeholder()` is a statement), and failing that, the text after the cursor is dropped
//! and the open brackets and blocks before it are closed.

use crate::incremental::ProjectWorkspace;
use crate::project_check::ProjectCheckOptions;
use crate::{bind, describe_native};
use luanalysis_engine::{NativeCheckMode, Type};
use luanalysis_project::{FileId, Project};
use luanalysis_syntax::{CommentKind, Documentation, Expr, ExprKind, Name, Span, TokenKind};
use std::ops::Range;

/// The name that stands for the word being completed.
pub const PLACEHOLDER: &str = "__luanalysis_completion__";

const KEYWORDS: [&str; 22] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionContext {
    /// A name: locals in scope, globals and keywords.
    Name,
    /// A member after `.`, or after `:` when `method` is set.
    Member { method: bool },
    /// A tag's name after `---@`.
    Tag,
    /// A type in an annotation: `---@param point Po`.
    AnnotationType,
    /// The name of a global function being defined: `function onObjectDr`.
    FunctionName,
}

/// Where a completion was requested, and the text to check in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionSite {
    pub context: CompletionContext,
    /// The editor's text with the word at the cursor replaced by `PLACEHOLDER`, repaired to parse.
    pub text: String,
    /// `PLACEHOLDER` in `text`.
    pub placeholder: Span,
    /// The word at the cursor in the editor's text, which a completion replaces.
    pub word: Range<usize>,
    /// Whether the word alone begins a statement of the main chunk, on a line of its own.
    pub statement: bool,
    /// Whether a parameter list already follows the word.
    pub parameters_follow: bool,
    /// Whether the word is a value that may have an expected type: an argument, an assigned
    /// value, a table constructor's field or a returned value.
    pub value: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Variable,
    Function,
    Field,
    Method,
    Keyword,
    Class,
    TypeParameter,
    /// A function implementation, of a host's callback or of an expected function value.
    Snippet,
    /// A literal value the expected type allows.
    Value,
}

/// A function's implementation, which a completion inserts with an empty body: what precedes its
/// parameter list (`function onSave`, the name after a written `function`, or `function` for a
/// function value), and its parameters' names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Implementation {
    pub head: String,
    pub parameters: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// The item's type, when known.
    pub detail: Option<String>,
    /// What the item's declaration documents, as Markdown.
    pub documentation: Option<String>,
    /// A callable's required parameters, which a call snippet fills in, and whether it takes
    /// more (optional or variadic ones) after them.
    pub parameters: Option<(Vec<String>, bool)>,
    /// A function's implementation, which the completion inserts.
    pub implementation: Option<Implementation>,
    /// Whether its declaration is `---@deprecated`.
    pub deprecated: bool,
}

/// The completion site at a byte offset of `text`, or `None` where nothing is completed: inside
/// a comment, a string or a number, at a name being declared, or where no repair parses.
pub fn completion_site(text: &str, offset: usize) -> Option<CompletionSite> {
    let offset = floor_char_boundary(text, offset);
    let bytes = text.as_bytes();
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut start = offset;
    while start > 0 && is_word(bytes[start - 1]) { start -= 1; }
    let mut end = offset;
    while end < bytes.len() && is_word(bytes[end]) { end += 1; }
    if bytes.get(start).is_some_and(u8::is_ascii_digit) { return None; }
    let tokens = luanalysis_syntax::lex(text).tokens;
    if let Some(context) = annotation_context(text, &tokens, start, offset) {
        return Some(CompletionSite { context, text: text.to_owned(), placeholder: Span { start, end: start }, word: start..end, statement: false, parameters_follow: false, value: false });
    }
    for token in &tokens {
        let inside = match &token.kind {
            TokenKind::Comment(comment) => token.span.start < offset
                && (offset < token.span.end || offset == token.span.end && comment.kind != CommentKind::Long),
            TokenKind::String(_) | TokenKind::Number(_) | TokenKind::CompletionHint(_) => token.span.start < offset && offset < token.span.end,
            _ => false,
        };
        if inside { return None; }
    }
    // The last significant token before the word decides the context.
    let previous = tokens.iter()
        .filter(|token| token.span.end <= start && !matches!(token.kind, TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof))
        .next_back();
    let context = match previous.map(|token| &token.kind) {
        Some(TokenKind::Symbol(symbol)) if symbol == "." => CompletionContext::Member { method: false },
        Some(TokenKind::Symbol(symbol)) if symbol == ":" => CompletionContext::Member { method: true },
        // A global function's name may implement a host's callback; a local's is its own.
        Some(TokenKind::Keyword(keyword)) if keyword == "function" => {
            let before = tokens.iter()
                .filter(|token| previous.is_some_and(|previous| token.span.end <= previous.span.start)
                    && !matches!(token.kind, TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof))
                .next_back();
            if matches!(before.map(|token| &token.kind), Some(TokenKind::Keyword(keyword)) if keyword == "local") { return None; }
            CompletionContext::FunctionName
        }
        // Any other name being declared is not completed.
        Some(TokenKind::Keyword(keyword)) if keyword == "local" || keyword == "for" => return None,
        _ => CompletionContext::Name,
    };
    let value = context == CompletionContext::Name && match previous.map(|token| &token.kind) {
        Some(TokenKind::Symbol(symbol)) => matches!(symbol.as_str(), "=" | "(" | "," | "{" | ";"),
        Some(TokenKind::Keyword(keyword)) => keyword == "return",
        _ => false,
    };
    let head = &text[..start];
    let tail = &text[end..];
    let line_head = &head[head.rfind('\n').map_or(0, |newline| newline + 1)..];
    let line_tail = &tail[..tail.find('\n').unwrap_or(tail.len())];
    let statement = context == CompletionContext::Name && line_head.trim().is_empty() && line_tail.trim().is_empty() && closers(head).is_empty();
    let parameters_follow = tail.trim_start().starts_with('(');
    let mut candidates = vec![
        format!("{head}{PLACEHOLDER}{tail}"),
        format!("{head}{PLACEHOLDER}(){tail}"),
    ];
    // A function being named is completed as an empty one, keeping the rest of the text.
    if context == CompletionContext::FunctionName { candidates.insert(1, format!("{head}{PLACEHOLDER}() end{tail}")); }
    for call in ["", "()"] {
        let prefix = format!("{head}{PLACEHOLDER}{call}");
        let closers = closers(&prefix);
        candidates.push(prefix + &closers);
    }
    let text = candidates.into_iter().find(|candidate| !luanalysis_syntax::parse(candidate).has_lua_syntax_errors())?;
    Some(CompletionSite {
        context,
        text,
        placeholder: Span { start, end: start + PLACEHOLDER.len() },
        word: start..end,
        statement,
        parameters_follow,
        value,
    })
}

/// The completion an annotation takes at `offset`, when it is in a documentation comment: a tag
/// name after `@`, or a type where the tag takes one. A tag's own names (`@param name`,
/// `@class Name`) and its description are not completed.
fn annotation_context(text: &str, tokens: &[luanalysis_syntax::Token], start: usize, offset: usize) -> Option<CompletionContext> {
    let comment = tokens.iter().find(|token| matches!(&token.kind, TokenKind::Comment(comment) if comment.kind == CommentKind::Documentation)
        && token.span.start < offset && offset <= token.span.end)?;
    let line_start = text[..start].rfind('\n').map_or(0, |newline| newline + 1).max(comment.span.start);
    let line = &text[line_start..start];
    let content = line.trim_start().trim_start_matches('-').trim_start_matches(['[', '=']).trim_start();
    let Some(tag) = content.strip_prefix('@') else {
        // A line continuing a tag continues its type.
        return (!content.trim().is_empty() || line.trim_start().starts_with("---")).then_some(CompletionContext::AnnotationType);
    };
    let name_end = tag.find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_').unwrap_or(tag.len());
    if name_end == tag.len() { return Some(CompletionContext::Tag); }
    let (name, rest) = tag.split_at(name_end);
    // The words a tag names before its type: `@param name`, `@field name`, `@cast name`.
    let words = rest.split_whitespace().count() + usize::from(rest.ends_with(char::is_whitespace));
    let typed = match name {
        "type" | "return" | "vararg" | "not" | "as" | "overload" => true,
        "param" | "field" | "cast" => words >= 2,
        "class" | "shape" | "module" => rest.contains(':'),
        "alias" => words >= 2,
        "generic" => rest.contains(':'),
        _ => false,
    };
    typed.then_some(CompletionContext::AnnotationType)
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) { offset -= 1; }
    offset
}

/// What closes the brackets and blocks still open at the end of `prefix`.
fn closers(prefix: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Open { Paren, Brace, Bracket, Block, Repeat, Condition, LoopHead }
    let mut open = Vec::new();
    for token in luanalysis_syntax::lex(prefix).tokens {
        match &token.kind {
            TokenKind::Keyword(keyword) => match keyword.as_str() {
                "function" => open.push(Open::Block),
                "do" => {
                    if open.last() == Some(&Open::LoopHead) { open.pop(); }
                    open.push(Open::Block);
                }
                "then" => {
                    if open.last() == Some(&Open::Condition) { open.pop(); }
                    open.push(Open::Block);
                }
                "if" => open.push(Open::Condition),
                "elseif" => {
                    if open.last() == Some(&Open::Block) { open.pop(); }
                    open.push(Open::Condition);
                }
                "while" | "for" => open.push(Open::LoopHead),
                "repeat" => open.push(Open::Repeat),
                "until" | "end" => { open.pop(); }
                _ => {}
            },
            TokenKind::Symbol(symbol) => match symbol.as_str() {
                "(" => open.push(Open::Paren),
                "{" => open.push(Open::Brace),
                "[" => open.push(Open::Bracket),
                ")" | "}" | "]" => { open.pop(); }
                _ => {}
            },
            _ => {}
        }
    }
    open.iter().rev().map(|open| match open {
        Open::Paren => ")",
        Open::Brace => "}",
        Open::Bracket => "]",
        Open::Block => "\nend",
        Open::Repeat => "\nuntil true",
        Open::Condition => " then end",
        Open::LoopHead => " do end",
    }).collect()
}

/// The completions at `site` in `file`, whose text in `project` is `site.text`. `deliver`
/// receives them as soon as they are known, before the session's state is dropped.
pub fn complete(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    site: &CompletionSite,
    deliver: impl FnOnce(Vec<CompletionItem>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(completions(project, registry, file, site, queries));
    });
}

fn completions(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<CompletionItem> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    registry.prepare_documentation_members(queries);
    let mut items = Vec::new();
    match site.context {
        CompletionContext::Member { method } => {
            let mut receiver = None;
            crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
                ExprKind::Field { base, name } | ExprKind::MethodCall { receiver: base, name, .. } if name.span == site.placeholder => {
                    receiver = Some(base.span);
                }
                _ => {}
            });
            let Some(receiver) = receiver else { return items };
            let (receiver_type, members) = receiver_members(registry, file, receiver, queries);
            for (name, value) in members {
                if !is_identifier(&name) || items.iter().any(|item: &CompletionItem| item.label == name) { continue; }
                let callable = matches!(value, Type::Any) || !queries.call_signature_types(&value).is_empty();
                if method && !callable { continue; }
                let kind = match (callable, method) {
                    (false, _) => CompletionKind::Field,
                    (true, true) => CompletionKind::Method,
                    (true, false) => CompletionKind::Function,
                };
                let documentation = receiver_type.as_ref()
                    .and_then(|receiver| member_documentation(project, registry, receiver, &name, &value, queries));
                let deprecated = documentation.as_ref().is_some_and(|documentation| documentation.deprecated.is_some());
                let documentation = documentation.map(|documentation| documentation_markdown(&documentation, true));
                let parameters = callable.then(|| required_parameters(&value, usize::from(method), queries)).flatten();
                items.push(CompletionItem { label: name, kind, detail: Some(describe_native(&value, queries, &mut Vec::new())), documentation, parameters, implementation: None, deprecated });
            }
        }
        CompletionContext::FunctionName => items.extend(callback_implementations(project, registry, file, site, false, queries)),
        CompletionContext::Tag => {
            for tag in ["param", "return", "type", "class", "shape", "field", "alias", "generic", "overload", "vararg", "function", "operator", "not", "see", "module", "enum", "cast", "as"] {
                items.push(CompletionItem { label: tag.to_owned(), kind: CompletionKind::Keyword, detail: None, documentation: None, parameters: None, implementation: None, deprecated: false });
            }
        }
        CompletionContext::AnnotationType => {
            let block = luanalysis_syntax::type_names(source.syntax(), &project.file(file).map(|file| file.source.clone()).unwrap_or_default())
                .into_iter().find(|name| name.block.start <= site.word.start && site.word.start <= name.block.end).map(|name| name.block);
            for other in &project.files {
                let Some(syntax) = registry.source(other.id).map(|source| source.syntax()) else { continue };
                for name in luanalysis_syntax::type_names(syntax, &other.source) {
                    let kind = match name.kind {
                        luanalysis_syntax::TypeNameKind::Declaration => CompletionKind::Class,
                        luanalysis_syntax::TypeNameKind::Generic if other.id == file && Some(name.block) == block => CompletionKind::TypeParameter,
                        _ => continue,
                    };
                    if name.name == PLACEHOLDER || items.iter().any(|item: &CompletionItem| item.label == name.name) { continue; }
                    let documentation = (kind == CompletionKind::Class).then(|| luanalysis_syntax::documentation_at(syntax, &other.source, name.span.start))
                        .map(|documentation| { let mut summary = documentation; summary.fields.clear(); documentation_markdown(&summary, false) })
                        .filter(|markdown| !markdown.is_empty());
                    items.push(CompletionItem { label: name.name, kind, detail: None, documentation, parameters: None, implementation: None, deprecated: false });
                }
            }
            for builtin in ["nil", "boolean", "number", "integer", "string", "table", "function", "userdata", "thread", "any", "unknown", "never", "void", "self", "fun"] {
                if !items.iter().any(|item| item.label == builtin) {
                    items.push(CompletionItem { label: builtin.to_owned(), kind: CompletionKind::Keyword, detail: None, documentation: None, parameters: None, implementation: None, deprecated: false });
                }
            }
        }
        CompletionContext::Name => {
            // Names need no check: a local's detail is its declared type.
            let bound = source.bound();
            for binding in visible_bindings(bound, site.placeholder.start) {
                let declaration = bound.declaration(binding);
                let detail = source.resolve_binding(binding, queries)
                    .map(|binding| describe_native(&binding.constraint, queries, &mut Vec::new()));
                let kind = match declaration.kind {
                    bind::DeclarationKind::LocalFunction => CompletionKind::Function,
                    _ => CompletionKind::Variable,
                };
                let documentation = binding_documentation(project, registry, file, binding);
                let deprecated = documentation.as_ref().is_some_and(|documentation| documentation.deprecated.is_some());
                let documentation = documentation.map(|documentation| documentation_markdown(&documentation, true));
                let parameters = (kind == CompletionKind::Function).then(|| {
                    let value = source.resolve_binding(binding, queries)?.constraint;
                    required_parameters(&value, 0, queries)
                }).flatten();
                items.push(CompletionItem { label: declaration.name.clone(), kind, detail, documentation, parameters, implementation: None, deprecated });
            }
            if let Some(environment) = source.global_environment()
                && let Ok(environment) = queries.global_environment_type(environment)
            {
                for (name, value) in queries.type_members(&environment) {
                    if !is_identifier(&name) || items.iter().any(|item| item.label == name) { continue; }
                    let kind = if queries.call_signature_types(&value).is_empty() { CompletionKind::Variable } else { CompletionKind::Function };
                    let documentation = global_documentation(project, registry, &name)
                        .or_else(|| value_documentation(project, registry, &value, queries));
                    let deprecated = documentation.as_ref().is_some_and(|documentation| documentation.deprecated.is_some());
                    let documentation = documentation.map(|documentation| documentation_markdown(&documentation, true));
                    let parameters = (kind == CompletionKind::Function).then(|| required_parameters(&value, 0, queries)).flatten();
                    items.push(CompletionItem { label: name, kind, detail: Some(describe_native(&value, queries, &mut Vec::new())), documentation, parameters, implementation: None, deprecated });
                }
            }
            // A statement of its own may begin a callback's implementation.
            if site.statement { items.extend(callback_implementations(project, registry, file, site, true, queries)); }
            // A value may be expected to be a function or one of some literals.
            if site.value { items.splice(0..0, expected_values(registry, file, site, queries)); }
            items.extend(KEYWORDS.iter().map(|keyword| CompletionItem { label: (*keyword).to_owned(), kind: CompletionKind::Keyword, detail: None, documentation: None, parameters: None, implementation: None, deprecated: false }));
        }
    }
    items
}

/// Implementations of the callbacks a declaration library declares as values of a function type
/// (`---@type nil | fun(...)` above `name = nil`), for a global function's name or a statement
/// of its own. A callback with several signatures, one the workspace already implements, and one
/// a local of the same name hides are not offered.
fn callback_implementations(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    keyword: bool,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<CompletionItem> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let bound = source.bound();
    let shadowed: Vec<String> = visible_bindings(bound, site.placeholder.start).into_iter().map(|binding| bound.declaration(binding).name.clone()).collect();
    let implemented = implemented_globals(project, registry, file, site.word.start);
    let mut items = Vec::new();
    for (declared_in, name, key, statement) in registry.ambient_values() {
        if shadowed.contains(&name) || implemented.contains(&name) || items.iter().any(|item: &CompletionItem| item.label == name) { continue; }
        let Ok(declared) = queries.value_declaration_type(key) else { continue };
        let callable = queries.exclude_type(&declared, &Type::Nil);
        let heads = queries.call_signature_types(&callable);
        let [head] = heads.as_slice() else { continue };
        let Some((parameters, _)) = crate::describe_signature(head, queries) else { continue };
        let names: Vec<String> = parameters.iter().map(|parameter| parameter.split(':').next().unwrap_or_default().trim_end_matches('?').trim().to_owned()).collect();
        let documentation = registry.source(declared_in).zip(project.file(declared_in))
            .map(|(declaring, text)| luanalysis_syntax::documentation_above(declaring.syntax(), &text.source, statement.start))
            .filter(|documentation| !documentation.is_empty())
            .map(|documentation| documentation_markdown(&documentation, true));
        // With a parameter list already written, only the name is completed.
        let head = if keyword { format!("function {name}") } else { name.clone() };
        let implementation = (!site.parameters_follow).then(|| Implementation { head, parameters: names.clone() });
        let kind = if implementation.is_some() { CompletionKind::Snippet } else { CompletionKind::Function };
        items.push(CompletionItem {
            detail: Some(format!("Implement {name}({})", names.join(", "))),
            label: name,
            kind,
            documentation,
            parameters: None,
            implementation,
            deprecated: false,
        });
    }
    items
}

/// The values the type expected at `site` allows: a function implementing each signature it has,
/// with an empty body, and each literal it may be. `callback_function = ` given
/// `nil | fun(object: tts__Object)` completes `function(object) end`.
fn expected_values(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<CompletionItem> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    source.probe_expected(site.placeholder);
    registry.check_owned_roots(queries, &[file]);
    let mut pending = source.probed_expected(site.placeholder);
    // An argument's parameter and an assigned field's type are expected too, though checking
    // types an argument the call cannot take yet, or a store to a table it infers, without them.
    pending.extend(written_expected(registry, file, site, queries));
    pending.reverse();
    let mut seen = Vec::new();
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut add = |item: CompletionItem| if !items.iter().any(|other| other.label == item.label) { items.push(item) };
    while let Some(ty) = pending.pop() {
        if seen.len() > 64 || seen.contains(&ty) { continue; }
        seen.push(ty.clone());
        let literal = |label: String| CompletionItem { label, kind: CompletionKind::Value, detail: None, documentation: None, parameters: None, implementation: None, deprecated: false };
        let any_function = match queries.inspect_type(&ty) {
            Type::Union(parts) => { pending.extend(parts.into_iter().rev()); continue; }
            Type::StringLiteral(bytes) => { add(literal(lua_string(&bytes))); continue; }
            Type::NumberLiteral(value) => { add(literal(if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { value.to_string() })); continue; }
            inspected => inspected == Type::RuntimeFunction,
        };
        for head in queries.call_signature_types(&ty) {
            let Some((parameters, _)) = crate::describe_signature(&head, queries) else { continue };
            let names: Vec<String> = parameters.iter().map(|parameter| parameter.split(':').next().unwrap_or_default().trim_end_matches('?').trim().to_owned()).collect();
            // Any function at all (`function`) takes whatever it is given.
            let detail = if any_function { "function".to_owned() } else { describe_native(&head, queries, &mut Vec::new()) };
            add(CompletionItem {
                label: format!("function({})", names.join(", ")),
                kind: CompletionKind::Snippet,
                detail: Some(detail),
                documentation: None,
                parameters: None,
                implementation: Some(Implementation { head: "function".to_owned(), parameters: names }),
                deprecated: false,
            });
        }
    }
    items
}

/// The types the value at `site` is expected to have as what it is written as: an argument, the
/// type of the parameter it is passed to in each signature of the call; a value assigned to a
/// field, the field's type.
fn written_expected(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &CompletionSite,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<Type> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let mut call = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
            if let Some(index) = args.iter().position(|argument| argument.span == site.placeholder) { call = Some((expression.clone(), index)); }
        }
        _ => {}
    });
    if let Some((call, index)) = call {
        // A method call passes its receiver as the first argument.
        let (callee, position) = match &call.kind {
            ExprKind::Call { callee, .. } => (expression_type(registry, file, callee.span, queries), index),
            ExprKind::MethodCall { receiver, name, .. } => {
                let member = expression_type(registry, file, receiver.span, queries).and_then(|receiver| {
                    let receiver = queries.exclude_type(&receiver, &Type::Nil);
                    queries.type_members(&receiver).into_iter().find(|(member, _)| *member == name.text).map(|(_, value)| value)
                });
                (member, index + 1)
            }
            _ => return Vec::new(),
        };
        let Some(callee) = callee else { return Vec::new() };
        let mut expected = Vec::new();
        for head in queries.call_signature_types(&callee) {
            let Some(signature) = queries.function_signature(&head) else { continue };
            match signature.params.get(position) {
                Some(parameter) => expected.push(parameter.ty.clone()),
                None => if let Some(rest) = queries.signature_rest_pack(&head) {
                    expected.push(queries.pack_slot(&rest, position - signature.params.len()));
                },
            }
        }
        return expected;
    }
    // `receiver.name = |`: the field just before the `=`.
    let assigned = |end: usize| site.text.get(end..site.placeholder.start).is_some_and(|between| between.trim() == "=");
    let mut field = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
        if let ExprKind::Field { base, name } = &expression.kind && assigned(expression.span.end) {
            field = Some((base.span, name.text.clone()));
        }
    });
    let Some((base, name)) = field else { return Vec::new() };
    let Some(receiver) = expression_type(registry, file, base, queries) else { return Vec::new() };
    let receiver = queries.exclude_type(&receiver, &Type::Nil);
    queries.type_members(&receiver).into_iter().filter(|(member, _)| *member == name).map(|(_, value)| value).collect()
}

/// `bytes` as a Lua string literal: UTF-8 text as it is, other bytes and control characters
/// escaped.
fn lua_string(bytes: &[u8]) -> String {
    let mut literal = String::from("\"");
    let escape = |literal: &mut String, character: char| match character {
        '"' => literal.push_str("\\\""),
        '\\' => literal.push_str("\\\\"),
        '\n' => literal.push_str("\\n"),
        '\r' => literal.push_str("\\r"),
        '\t' => literal.push_str("\\t"),
        character if character.is_control() => {
            for byte in character.to_string().bytes() { literal.push_str(&format!("\\{byte:03}")) }
        }
        character => literal.push(character),
    };
    match std::str::from_utf8(bytes) {
        Ok(text) => for character in text.chars() { escape(&mut literal, character) },
        Err(_) => for &byte in bytes {
            if byte.is_ascii() { escape(&mut literal, byte as char) } else { literal.push_str(&format!("\\{byte:03}")) }
        },
    }
    literal.push('"');
    literal
}

/// The globals the workspace's scripts implement at the top level of their main chunks
/// (`function name()`, `name = function() end`, or through `_G`), except at `except` in `file`.
fn implemented_globals(project: &Project, registry: &crate::native_functions::project_provider::ProjectRegistry, file: FileId, except: usize) -> Vec<String> {
    let mut names = Vec::new();
    for other in project.files.iter().filter(|other| other.role != luanalysis_project::SourceRole::Declaration) {
        let Some(source) = registry.source(other.id) else { continue };
        let bound = source.bound();
        let global = |target: &Expr| -> Option<Name> {
            match &target.kind {
                ExprKind::Name(name) if bound.binding_at(name.span.start).is_none() => Some(name.clone()),
                ExprKind::Field { base, name } if matches!(&base.kind, ExprKind::Name(root) if root.text == "_G" && bound.binding_at(root.span.start).is_none()) => Some(name.clone()),
                _ => None,
            }
        };
        for statement in &source.syntax().statements {
            let implemented = match &statement.kind {
                luanalysis_syntax::StmtKind::FunctionDeclaration { target, .. } => global(target),
                luanalysis_syntax::StmtKind::Assign { targets, values, .. } => targets.iter().zip(values)
                    .find(|(_, value)| matches!(value.kind, ExprKind::Function(_)))
                    .and_then(|(target, _)| global(target)),
                _ => None,
            };
            if let Some(name) = implemented
                && !(other.id == file && name.span.start <= except && except <= name.span.end)
            {
                names.push(name.text);
            }
        }
    }
    names
}

/// The names of the required parameters of a callable's first signature, after the `hidden`
/// ones a method call passes itself, and whether it takes more after them.
fn required_parameters(value: &Type, hidden: usize, queries: &mut dyn luanalysis_engine::TypeQueries) -> Option<(Vec<String>, bool)> {
    let head = queries.call_signature_types(value).into_iter().next()?;
    let (parameters, _) = crate::describe_signature(&head, queries)?;
    let parameters: Vec<&String> = parameters.iter().skip(hidden).collect();
    let required: Vec<String> = parameters.iter()
        .take_while(|parameter| !parameter.starts_with("...") && !parameter.split(':').next().is_some_and(|name| name.ends_with('?')))
        .map(|parameter| parameter.split(':').next().unwrap_or_default().trim().to_owned())
        .collect();
    let more = parameters.len() > required.len();
    Some((required, more))
}

/// The members of the receiver at `span`. Asked alone, the receiver has its type at its flow
/// position, as tsgo asks `getTypeAtLocation`. Where that names no member, such as a callback's
/// parameter outside the call that types it, the file is checked so that the receiver has its
/// type in each frame it is checked in.
fn receiver_members(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    span: Span,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> (Option<Type>, Vec<(String, Type)>) {
    let Some(source) = registry.source(file) else { return (None, Vec::new()) };
    let alone = source.source_check_expression(source.expression_key_for_span(span), None, NativeCheckMode::empty(), queries);
    let alone = queries.exclude_type(&alone, &Type::Nil);
    let found = queries.type_members(&alone);
    if !found.is_empty() { return (Some(alone), found); }
    source.probe_expression(span);
    registry.check_owned_roots(queries, &[file]);
    let mut receiver = None;
    let mut found: Vec<(String, Type)> = Vec::new();
    for ty in source.probed_types(span) {
        let ty = queries.exclude_type(&ty, &Type::Nil);
        for (name, value) in queries.type_members(&ty) {
            if !found.iter().any(|(other, _)| *other == name) { found.push((name, value)); }
        }
        receiver.get_or_insert(ty);
    }
    (receiver, found)
}

/// The locals in scope at `offset`, innermost first; a shadowed name appears once.
fn visible_bindings(bound: &bind::BoundFile, offset: usize) -> Vec<bind::BindingId> {
    let mut visible: Vec<bind::BindingId> = Vec::new();
    let mut scope = bound.scope_at(offset);
    while let Some(current) = scope {
        let node = bound.scope(current);
        for &binding in node.bindings.iter().rev() {
            let declaration = bound.declaration(binding);
            if declaration.activation > offset || declaration.name == PLACEHOLDER { continue; }
            if visible.iter().any(|&other| bound.declaration(other).name == declaration.name) { continue; }
            visible.push(binding);
        }
        scope = node.parent;
    }
    visible
}

fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !KEYWORDS.contains(&name)
        && name != PLACEHOLDER
}

/// Where signature help was requested: in the argument list of a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureSite {
    /// The editor's text, repaired to parse if it does not.
    pub text: String,
    /// The call's `(`, at the same offset in the editor's text and in `text`.
    pub open: usize,
    /// The argument the cursor is in, counted from 0.
    pub argument: usize,
}

/// A signature of the callee, with each parameter's place in its label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureInformation {
    /// `name(a: number, b?: string): boolean`.
    pub label: String,
    /// Each parameter's byte range in `label`.
    pub parameters: Vec<Range<usize>>,
    /// The parameter the cursor's argument is for; a rest parameter takes every argument after
    /// the others.
    pub active_parameter: usize,
    /// The function's description and results, as Markdown.
    pub documentation: Option<String>,
    /// Each parameter's description.
    pub parameter_documentation: Vec<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    /// The overload that the arguments before the cursor select.
    pub active_signature: usize,
    /// The parameter of the active signature that the cursor's argument is for.
    pub active_parameter: usize,
}

/// The call whose arguments surround a byte offset of `text`, or `None` outside one, in a
/// comment or a string, or in a function's parameter list.
pub fn signature_site(text: &str, offset: usize) -> Option<SignatureSite> {
    let offset = floor_char_boundary(text, offset);
    let head = &text[..offset];
    let tokens = luanalysis_syntax::lex(head).tokens;
    // A comment or a string still open at the cursor is a token that ends there, unclosed.
    let inside = tokens.iter().any(|token| token.span.end == offset && match &token.kind {
        TokenKind::Comment(comment) => comment.kind != CommentKind::Long || !head[token.span.start..].ends_with(']'),
        TokenKind::String(_) => !closed_string(&head[token.span.start..]),
        _ => false,
    });
    if inside { return None; }
    let significant: Vec<_> = tokens.iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof))
        .collect();
    // Open brackets, each with the commas directly inside it.
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (index, token) in significant.iter().enumerate() {
        let TokenKind::Symbol(symbol) = &token.kind else { continue };
        match symbol.as_str() {
            "(" | "{" | "[" => open.push((index, 0)),
            ")" | "}" | "]" => { open.pop(); }
            "," => if let Some((_, commas)) = open.last_mut() { *commas += 1; },
            _ => {}
        }
    }
    let &(index, argument) = open.iter().rev().find(|&&(index, _)| {
        matches!(&significant[index].kind, TokenKind::Symbol(symbol) if symbol == "(") && is_call_paren(&significant, index)
    })?;
    let paren = significant[index].span.start;
    let tail = &text[offset..];
    let mut candidates = vec![text.to_owned(), format!("{head}{PLACEHOLDER}{tail}")];
    for argument in ["", PLACEHOLDER] {
        let prefix = format!("{head}{argument}");
        let closers = closers(&prefix);
        candidates.push(prefix + &closers);
    }
    let text = candidates.into_iter().find(|candidate| !luanalysis_syntax::parse(candidate).has_lua_syntax_errors())?;
    Some(SignatureSite { text, open: paren, argument })
}

/// Whether a string token is closed, as `"text"` is and `"text` is not.
fn closed_string(token: &str) -> bool {
    let bytes = token.as_bytes();
    match bytes.first() {
        Some(&quote @ (b'"' | b'\'')) => {
            bytes.len() >= 2 && bytes[bytes.len() - 1] == quote
                && bytes[1..bytes.len() - 1].iter().rev().take_while(|&&byte| byte == b'\\').count() % 2 == 0
        }
        _ => token.ends_with(']'),
    }
}

/// Whether the `(` at `index` opens a call's arguments, not a parenthesized expression or a
/// function's parameters.
fn is_call_paren(tokens: &[&luanalysis_syntax::Token], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).map(|index| &tokens[index].kind) else { return false };
    let callee = match previous {
        TokenKind::Identifier(_) | TokenKind::String(_) => true,
        TokenKind::Symbol(symbol) => symbol == ")" || symbol == "]" || symbol == "}",
        _ => false,
    };
    if !callee { return false; }
    // `function name.path:method(` and `function(` declare parameters.
    let mut cursor = index;
    while cursor > 0 && matches!(&tokens[cursor - 1].kind, TokenKind::Identifier(_))
        || cursor > 1 && matches!(&tokens[cursor - 1].kind, TokenKind::Symbol(symbol) if symbol == "." || symbol == ":")
            && matches!(&tokens[cursor - 2].kind, TokenKind::Identifier(_))
    {
        cursor -= 1;
    }
    !(cursor > 0 && matches!(&tokens[cursor - 1].kind, TokenKind::Keyword(keyword) if keyword == "function"))
}

/// Signature help for the call at `site` in `file`, whose text in `project` is `site.text`.
pub fn signature_help(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    site: &SignatureSite,
    deliver: impl FnOnce(Option<SignatureHelp>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(signatures(project, registry, file, site, queries));
    });
}

fn signatures(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    site: &SignatureSite,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<SignatureHelp> {
    let source = registry.source(file)?;
    registry.prepare_documentation_members(queries);
    let text = &site.text;
    let opens_at = |end: usize| text.get(end..site.open).is_some_and(|between| between.trim().is_empty());
    // The innermost call whose arguments open at the site's `(`.
    let mut call: Option<(Span, Expr)> = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
        let found = match &expression.kind {
            ExprKind::Call { callee, .. } => opens_at(callee.span.end),
            ExprKind::MethodCall { name, .. } => opens_at(name.span.end),
            _ => false,
        };
        if found && call.as_ref().is_none_or(|(span, _)| expression.span.end - expression.span.start < span.end - span.start) {
            call = Some((expression.span, expression.clone()));
        }
    });
    let (_, call) = call?;
    let (callee, name, receiver, arguments) = match &call.kind {
        ExprKind::Call { callee, args } => (expression_type(registry, file, callee.span, queries)?, &text[callee.span.start..callee.span.end], None, args),
        ExprKind::MethodCall { receiver, name, args } => {
            let receiver_type = expression_type(registry, file, receiver.span, queries)?;
            let receiver_type = queries.exclude_type(&receiver_type, &Type::Nil);
            let member = queries.type_members(&receiver_type).into_iter().find(|(member, _)| *member == name.text)?.1;
            (member, &text[receiver.span.start..name.span.end], Some(receiver_type), args)
        }
        _ => return None,
    };
    let heads = queries.call_signature_types(&callee);
    if heads.is_empty() { return None; }
    let documentation = match (&call.kind, &receiver) {
        (ExprKind::MethodCall { name, .. }, Some(receiver)) => member_documentation(project, registry, receiver, &name.text, &callee, queries),
        _ => value_documentation(project, registry, &callee, queries),
    }.unwrap_or_default();
    let described = {
        let mut summary = documentation.clone();
        summary.params.clear();
        Some(documentation_markdown(&summary, true)).filter(|markdown| !markdown.is_empty())
    };
    // A method call passes its receiver as the first argument, which the signatures leave out.
    let hidden = usize::from(receiver.is_some());
    let name = if name.len() > 80 { "function" } else { name };
    let mut signatures = Vec::new();
    let mut shapes = Vec::new();
    for head in &heads {
        let Some((parameters, returns)) = crate::describe_signature(head, queries) else { continue };
        let shown = parameters.get(hidden..).unwrap_or_default();
        let mut label = format!("{name}(");
        let mut ranges = Vec::new();
        for (index, parameter) in shown.iter().enumerate() {
            if index > 0 { label.push_str(", "); }
            ranges.push(label.len()..label.len() + parameter.len());
            label.push_str(parameter);
        }
        label.push(')');
        if !returns.is_empty() {
            label.push_str(": ");
            label.push_str(&returns);
        }
        let rest = shown.last().is_some_and(|parameter| parameter.starts_with("..."));
        let active_parameter = if rest { site.argument.min(shown.len() - 1) } else { site.argument };
        let parameter_documentation = shown.iter().map(|parameter| {
            let name = parameter.split(':').next().unwrap_or_default().trim_end_matches('?');
            documentation.param(name).map(str::to_owned)
        }).collect();
        shapes.push((head.clone(), shown.len(), rest));
        signatures.push(SignatureInformation { label, parameters: ranges, active_parameter, documentation: described.clone(), parameter_documentation });
    }
    if signatures.is_empty() { return None; }
    // The overload that the complete arguments before the cursor's select: the first whose
    // call with them fails only for want of more arguments, as TypeScript resolves a call
    // while it is being typed. Failing that, the first that takes the cursor's argument.
    let mut given: Vec<Type> = receiver.into_iter().collect();
    for argument in arguments.iter().take(site.argument) {
        given.push(expression_type(registry, file, argument.span, queries).unwrap_or(Type::Any));
    }
    let takes = |&(_, count, rest): &(Type, usize, bool)| rest || site.argument < count;
    let applicable = |queries: &mut dyn luanalysis_engine::TypeQueries, head: &Type| {
        queries.call(head, &given).errors.iter().all(|error| match error {
            luanalysis_engine::CallError::Arity { actual, minimum, .. } => actual < minimum,
            luanalysis_engine::CallError::Argument { .. } | luanalysis_engine::CallError::NotCallable => false,
            luanalysis_engine::CallError::PackArity { .. } | luanalysis_engine::CallError::TypeArgument { .. }
                | luanalysis_engine::CallError::TypeArgumentArity { .. } => false,
            _ => true,
        })
    };
    let active_signature = (0..shapes.len()).find(|&index| takes(&shapes[index]) && applicable(queries, &shapes[index].0))
        .or_else(|| shapes.iter().position(takes))
        .unwrap_or(0);
    let active_parameter = signatures[active_signature].active_parameter;
    Some(SignatureHelp { signatures, active_signature, active_parameter })
}

/// The type of the expression at `span`, at its flow position, or as it is checked in the frames
/// that check it when it has no type alone (a callback's parameter).
fn expression_type(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    span: Span,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Type> {
    let source = registry.source(file)?;
    let alone = source.source_check_expression(source.expression_key_for_span(span), None, NativeCheckMode::empty(), queries);
    if !untyped(&alone, queries) { return Some(alone); }
    source.probe_expression(span);
    registry.check_owned_roots(queries, &[file]);
    source.probed_types(span).into_iter().next().or(Some(alone))
}

/// Whether `ty` says nothing of a value: `any` or `unknown`, as the value of an expression
/// checked alone, outside the function that gives it a type, often is.
fn untyped(ty: &Type, queries: &mut dyn luanalysis_engine::TypeQueries) -> bool {
    matches!(ty, Type::Any | Type::Unknown) || matches!(queries.inspect_type(ty), Type::Any | Type::Unknown)
}

/// Documentation as Markdown: the description, then each parameter's and result's.
pub fn documentation_markdown(documentation: &Documentation, parameters: bool) -> String {
    let mut sections = Vec::new();
    if let Some(deprecated) = &documentation.deprecated {
        sections.push(if deprecated.is_empty() { "**Deprecated.**".to_owned() } else { format!("**Deprecated:** {deprecated}") });
    }
    if !documentation.description.is_empty() { sections.push(documentation.description.clone()); }
    if parameters {
        let lines: Vec<String> = documentation.params.iter()
            .map(|(name, text)| format!("*@param* `{name}` — {}", text.replace('\n', " ")))
            .chain(documentation.returns.iter().map(|text| format!("*@return* — {}", text.replace('\n', " "))))
            .collect();
        if !lines.is_empty() { sections.push(lines.join("  \n")); }
    }
    sections.join("\n\n")
}

/// The documentation of a value's declaration: a native function's doc comments, or those of a
/// written signature (`---@function`, `---@overload`).
fn value_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    value: &Type,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Documentation> {
    let (file, span, inside) = if let Some(key) = queries.native_function_key(value) {
        let (file, span) = registry.function_site(key)?;
        (file, span, false)
    } else if let Type::NativeSignature(key) = value {
        let (file, span) = registry.signature_site(*key)?;
        (file, span, true)
    } else {
        return None;
    };
    let syntax = registry.source(file)?.syntax();
    let text = &project.file(file)?.source;
    let documentation = if inside {
        luanalysis_syntax::documentation_at(syntax, text, span.start)
    } else {
        luanalysis_syntax::documentation_above(syntax, text, span.start)
    };
    (!documentation.is_empty()).then_some(documentation)
}

/// Where member `name` of `receiver` is declared: in the receiver's class (or runtime table)
/// or one it extends, nearest first. Each site is a file, a span, and whether the span is a
/// documentation tag (`---@field`, `---@function`) rather than a statement.
pub(crate) fn member_sites(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    receiver: &Type,
    name: &str,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<(FileId, Span, bool)> {
    use luanalysis_engine::{LuaKey, NativeTypeProvider, TypeMemberKeyRecipe};
    let mut sites = Vec::new();
    let mut pending: Vec<Type> = vec![receiver.clone()];
    let mut seen = Vec::new();
    while let Some(ty) = pending.pop() {
        let Some(key) = queries.named_object_declaration(&ty).or_else(|| queries.runtime_table_declaration(&ty)) else {
            match if let Type::NativeTypeParameter(_) = ty { ty.clone() } else { queries.inspect_type(&ty) } {
                // Each alternative of a union, or part of an intersection, declares its own.
                Type::Union(parts) | Type::Intersection(parts) => pending.extend(parts),
                // A type parameter's members, such as a method's `self`'s, are its constraint's.
                Type::NativeTypeParameter(key) => pending.extend(queries.type_parameter_constraint(key)),
                _ => {}
            }
            continue;
        };
        if seen.contains(&key) || seen.len() > 32 { continue; }
        seen.push(key);
        // A table's constructor declares the fields it is built with; later stores declare the
        // others.
        if let Some(site) = registry.runtime_table_field_site(key, name) {
            if !sites.contains(&site) { sites.push(site); }
            continue;
        }
        for member in registry.type_declaration_members(key) {
            let named = matches!(registry.type_member_header(member).key, TypeMemberKeyRecipe::Fixed(LuaKey::String(bytes)) if bytes == name.as_bytes());
            if named && let Some(site) = registry.member_site(member) && !sites.contains(&site) { sites.push(site); }
        }
        if sites.is_empty() { pending.extend(registry.type_declaration_bases(key, queries)); }
    }
    sites
}

/// The documentation of member `name` of `receiver`: what its declaration's documentation says
/// (a `---@field`'s description, a `---@function`'s or a function statement's doc comments), or
/// failing that, its value's.
fn member_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    receiver: &Type,
    name: &str,
    value: &Type,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Documentation> {
    for (file, span, tag) in member_sites(registry, receiver, name, queries) {
        let (Some(source), Some(text)) = (registry.source(file), project.file(file)) else { continue };
        let documentation = if tag {
            let block = luanalysis_syntax::documentation_at(source.syntax(), &text.source, span.start);
            match block.field(name) {
                Some(field) => Documentation { description: field.to_owned(), ..Documentation::default() },
                None if block.fields.is_empty() => block,
                None => continue,
            }
        } else {
            luanalysis_syntax::documentation_above(source.syntax(), &text.source, span.start)
        };
        if !documentation.is_empty() { return Some(documentation); }
    }
    value_documentation(project, registry, value, queries)
}

/// The documentation of the global `name`: what its declaration's doc comments say, whether it
/// is declared by a statement or a `---@function` tag.
fn global_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    name: &str,
) -> Option<Documentation> {
    registry.global_sites(name).into_iter().find_map(|(file, span, tag)| {
        let (source, text) = (registry.source(file)?, project.file(file)?);
        let documentation = if tag {
            luanalysis_syntax::documentation_at(source.syntax(), &text.source, span.start)
        } else {
            luanalysis_syntax::documentation_above(source.syntax(), &text.source, span.start)
        };
        (!documentation.is_empty()).then_some(documentation)
    })
}

/// The documentation of a local or parameter: the doc comments above its declaration, or for a
/// parameter, its function's `---@param`.
fn binding_documentation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    binding: bind::BindingId,
) -> Option<Documentation> {
    let source = registry.source(file)?;
    let declaration = source.bound().declaration(binding);
    let documentation = luanalysis_syntax::documentation_above(source.syntax(), &project.file(file)?.source, declaration.span.start);
    match declaration.kind {
        bind::DeclarationKind::Parameter => documentation.param(&declaration.name)
            .map(|text| Documentation { description: text.to_owned(), ..Documentation::default() }),
        bind::DeclarationKind::Local | bind::DeclarationKind::LocalFunction => (!documentation.is_empty()).then_some(documentation),
        _ => None,
    }
}

/// The documentation for the name at `offset` in `file`, as Markdown: a member's, a local's or a
/// parameter's, or the declaration of the value a name holds.
pub fn hover_documentation(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    deliver: impl FnOnce(Option<String>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        let markdown = type_name_hover(project, registry, file, offset).or_else(|| {
            documentation_at_offset(project, registry, file, offset, queries)
                .map(|documentation| documentation_markdown(&documentation, true))
        });
        deliver(markdown.filter(|markdown| !markdown.is_empty()));
    });
}

/// A hover on a type name in an annotation: the tag that declares the type, and what its
/// documentation says.
fn type_name_hover(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
) -> Option<String> {
    let text = &project.file(file)?.source;
    let names = luanalysis_syntax::type_names(registry.source(file)?.syntax(), text);
    let at = names.iter().find(|name| name.span.start <= offset && offset <= name.span.end)?;
    let navigation = type_navigation(project, registry, file, at, &names, false)?;
    let (path, span, _) = navigation.occurrences.iter().find(|occurrence| occurrence.2)?;
    let declaration = project.files.iter().find(|other| other.path == *path)?;
    let line_start = declaration.source[..span.start].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = declaration.source[span.end..].find('\n').map_or(declaration.source.len(), |newline| span.end + newline);
    let tag = declaration.source[line_start..line_end].trim();
    let documentation = luanalysis_syntax::documentation_at(registry.source(declaration.id)?.syntax(), &declaration.source, span.start);
    let mut summary = documentation.clone();
    summary.fields.clear();
    let described = documentation_markdown(&summary, false);
    Some(if described.is_empty() { format!("```lua\n{tag}\n```") } else { format!("```lua\n{tag}\n```\n\n{described}") })
}

fn documentation_at_offset(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Documentation> {
    let source = registry.source(file)?;
    registry.prepare_documentation_members(queries);
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    let mut member = None;
    let mut name = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Field { base, name: field } | ExprKind::MethodCall { receiver: base, name: field, .. } if contains(field.span) => {
            member = Some((base.span, field.text.clone()));
        }
        ExprKind::Name(_) if contains(expression.span) => name = Some(expression.span),
        _ => {}
    });
    if let Some((receiver, field)) = member {
        let receiver = expression_type(registry, file, receiver, queries)?;
        let receiver = queries.exclude_type(&receiver, &Type::Nil);
        let value = queries.type_members(&receiver).into_iter().find(|(member, _)| *member == field)?.1;
        return member_documentation(project, registry, &receiver, &field, &value, queries);
    }
    if let Some(binding) = source.bound().binding_at(offset) {
        if let Some(documentation) = binding_documentation(project, registry, file, binding) { return Some(documentation); }
        if source.bound().declaration(binding).kind == bind::DeclarationKind::Parameter { return None; }
    }
    let name = name?;
    if let Some(text) = registry.source(file).zip(project.file(file)).and_then(|(_, text)| text.source.get(name.start..name.end))
        && let Some(documentation) = global_documentation(project, registry, text)
    {
        return Some(documentation);
    }
    let value = expression_type(registry, file, name, queries)?;
    value_documentation(project, registry, &value, queries)
}

/// A member or type name the editor navigates from, and where it is declared and used.
#[derive(Clone, Debug, Default)]
pub struct Navigation {
    /// The name at the position, and its span there.
    pub name: String,
    pub span: Span,
    /// Each declaration, and when references were asked for, each use: its file, its name's span,
    /// and whether it declares.
    pub occurrences: Vec<(std::path::PathBuf, Span, bool)>,
    /// The text of each file with an occurrence, which the spans index.
    pub texts: std::collections::HashMap<std::path::PathBuf, String>,
    /// Whether the name is a type's (which may contain `.`) rather than a member's.
    pub type_name: bool,
}

/// Where the member or type name at `offset` in `file` is declared, and with `references`, where
/// it is used throughout the project. Locals and globals are the symbol index's.
pub fn navigate(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    references: bool,
    deliver: impl FnOnce(Option<Navigation>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(navigation(project, registry, file, offset, references, queries));
    });
}

type Site = (FileId, Span, bool);

fn navigation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    references: bool,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Navigation> {
    let source = registry.source(file)?;
    let text = &project.file(file)?.source;
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    let names = luanalysis_syntax::type_names(source.syntax(), text);
    if let Some(at) = names.iter().find(|name| contains(name.span)) {
        return type_navigation(project, registry, file, at, &names, references);
    }
    registry.prepare_documentation_members(queries);
    let (name, span, sites) = member_target(registry, file, text, offset, queries)?;
    let mut navigation = Navigation { name: name.clone(), span, ..Navigation::default() };
    if references {
        let _ = member_occurrences(project, registry, &name, &sites, &mut navigation, None, queries);
    } else {
        for &(site_file, site, _) in &sites {
            let Some(site_text) = project.file(site_file) else { continue };
            let span = word_in(&site_text.source, site, &name).unwrap_or(site);
            add_occurrence(&mut navigation, &site_text.path, &site_text.source, span, true);
        }
    }
    Some(navigation)
}

fn add_occurrence(navigation: &mut Navigation, path: &std::path::Path, text: &str, span: Span, declaration: bool) {
    let occurrence = (path.to_path_buf(), span, declaration);
    if !navigation.occurrences.contains(&occurrence) { navigation.occurrences.push(occurrence); }
    navigation.texts.entry(path.to_path_buf()).or_insert_with(|| text.to_owned());
}

/// The first occurrence of `word` as a whole word in `span` of `text`.
fn word_in(text: &str, span: Span, word: &str) -> Option<Span> {
    let region = text.get(span.start..span.end)?;
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    region.match_indices(word).find(|&(index, _)| {
        let before = index.checked_sub(1).map(|index| region.as_bytes()[index]);
        let after = region.as_bytes().get(index + word.len()).copied();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    }).map(|(index, _)| Span { start: span.start + index, end: span.start + index + word.len() })
}

/// A type name's declarations and uses. A `---@generic` parameter is its documentation block's;
/// any other name is a type's, declared by `---@class`, `---@shape`, `---@alias` or `---@enum`
/// in any file of the project.
fn type_navigation(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    at: &luanalysis_syntax::TypeName,
    names: &[luanalysis_syntax::TypeName],
    references: bool,
) -> Option<Navigation> {
    use luanalysis_syntax::TypeNameKind;
    let source = project.file(file)?;
    let mut navigation = Navigation { name: at.name.clone(), span: at.span, type_name: true, ..Navigation::default() };
    let shadowed = |names: &[luanalysis_syntax::TypeName], block: Span| names.iter()
        .any(|name| name.kind == TypeNameKind::Generic && name.block == block && name.name == at.name);
    if shadowed(names, at.block) {
        for name in names.iter().filter(|name| name.block == at.block && name.name == at.name) {
            let declaration = name.kind == TypeNameKind::Generic;
            if references || declaration { add_occurrence(&mut navigation, &source.path, &source.source, name.span, declaration); }
        }
        return Some(navigation);
    }
    for other in &project.files {
        let Some(syntax) = registry.source(other.id).map(|source| source.syntax()) else { continue };
        let names = luanalysis_syntax::type_names(syntax, &other.source);
        for name in names.iter().filter(|name| name.name == at.name && name.kind != TypeNameKind::Generic) {
            let declaration = name.kind == TypeNameKind::Declaration;
            if (references || declaration) && !shadowed(&names, name.block) {
                add_occurrence(&mut navigation, &other.path, &other.source, name.span, declaration);
            }
        }
    }
    // A built-in type (`number`, `string`) has no declaration to navigate to.
    navigation.occurrences.iter().any(|(_, _, declaration)| *declaration).then_some(navigation)
}

/// The member named at `offset`: in `receiver.name`, `receiver:name(...)`, a table constructor's
/// `name = value`, a `---@field name` or a `---@function Owner.name`; with its span and the
/// sites that declare it.
fn member_target(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    text: &str,
    offset: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<(String, Span, Vec<Site>)> {
    let source = registry.source(file)?;
    let syntax = source.syntax();
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    for declaration in syntax.type_declarations.iter() {
        if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind {
            for field in fields {
                if let luanalysis_syntax::TypeFieldKey::Named(name) = &field.key && contains(name.span) {
                    return Some((name.text.clone(), name.span, vec![(file, field.span, true)]));
                }
            }
        }
    }
    for extension in &syntax.type_extensions {
        for field in &extension.fields {
            if let luanalysis_syntax::TypeFieldKey::Named(name) = &field.key && contains(name.span) {
                return Some((name.text.clone(), name.span, vec![(file, field.span, true)]));
            }
        }
    }
    for declaration in &syntax.function_declarations {
        if declaration.owner.is_some() && contains(declaration.name.span) {
            return Some((declaration.name.text.clone(), declaration.name.span, vec![(file, declaration.span, true)]));
        }
    }
    let mut found = None;
    member_uses(syntax, text, &mut |receiver, name| if contains(name.span) { found = Some((receiver, name)); });
    let (receiver, name) = found?;
    let receiver = expression_type(registry, file, receiver, queries)?;
    let receiver = queries.exclude_type(&receiver, &Type::Nil);
    let sites = member_sites(registry, &receiver, &name.text, queries);
    (!sites.is_empty()).then_some((name.text, name.span, sites))
}

/// Where the type of the value at `offset` in `file` is declared: each named type (class, shape,
/// alias) it is, of each alternative of a union and a type parameter's constraint.
pub fn type_definition(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    deliver: impl FnOnce(Option<Navigation>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(type_definitions(project, registry, file, offset, queries));
    });
}

fn type_definitions(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<Navigation> {
    let source = registry.source(file)?;
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    // The innermost expression at the position, or the local declared there.
    let mut innermost: Option<Span> = None;
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
        let span = match &expression.kind {
            ExprKind::MethodCall { name, .. } if contains(name.span) => return,
            _ => expression.span,
        };
        if contains(span) && innermost.is_none_or(|other| span.end - span.start < other.end - other.start) { innermost = Some(span); }
    });
    let ty = match innermost {
        Some(span) => expression_type(registry, file, span, queries)?,
        None => source.resolve_binding(source.bound().binding_at(offset)?, queries)?.constraint,
    };
    let mut navigation = Navigation::default();
    let mut pending = vec![ty];
    let mut seen = Vec::new();
    while let Some(ty) = pending.pop() {
        if seen.len() > 32 || seen.contains(&ty) { continue; }
        seen.push(ty.clone());
        if let Some(key) = queries.named_object_declaration(&ty) {
            for (site_file, span) in registry.type_declaration_sites(key) {
                if let Some(site) = project.file(site_file) { add_occurrence(&mut navigation, &site.path, &site.source, span, true); }
            }
            continue;
        }
        match if let Type::NativeTypeParameter(_) = ty { ty.clone() } else { queries.inspect_type(&ty) } {
            Type::Union(parts) | Type::Intersection(parts) => pending.extend(parts.into_iter().rev()),
            Type::NativeTypeParameter(key) => pending.extend(queries.type_parameter_constraint(key)),
            _ => {}
        }
    }
    (!navigation.occurrences.is_empty()).then_some(navigation)
}

/// Every use of the member declared at `sites` in the project: member reads and calls, table
/// constructor keys, `---@field` and `---@function` tags. A use's receiver is typed at its flow
/// position; one whose members are declared elsewhere is another member of the same name.
fn member_occurrences(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    name: &str,
    sites: &[Site],
    navigation: &mut Navigation,
    rename: Option<&str>,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Result<(), String> {
    let declares = |file: FileId, span: Span| sites.iter().any(|&(site_file, site, _)| site_file == file && site.start <= span.start && span.end <= site.end);
    for other in &project.files {
        let Some(source) = registry.source(other.id) else { continue };
        let syntax = source.syntax();
        for declaration in &syntax.type_declarations {
            if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind {
                for field in fields {
                    if let luanalysis_syntax::TypeFieldKey::Named(field_name) = &field.key
                        && field_name.text == name && sites.contains(&(other.id, field.span, true))
                    {
                        add_occurrence(navigation, &other.path, &other.source, field_name.span, true);
                    }
                }
            }
        }
        for extension in &syntax.type_extensions {
            for field in &extension.fields {
                if let luanalysis_syntax::TypeFieldKey::Named(field_name) = &field.key
                    && field_name.text == name && sites.contains(&(other.id, field.span, true))
                {
                    add_occurrence(navigation, &other.path, &other.source, field_name.span, true);
                }
            }
        }
        for declaration in &syntax.function_declarations {
            if declaration.name.text == name && sites.contains(&(other.id, declaration.span, true)) {
                add_occurrence(navigation, &other.path, &other.source, declaration.name.span, true);
            }
        }
        let mut candidates = Vec::new();
        member_uses(syntax, &other.source, &mut |receiver, member| if member.text == name { candidates.push((receiver, member.span)); });
        // Receivers without a type of their own, such as a callback's parameters or a method's
        // `self`, have their types in the frames they are checked in: one check of the file
        // probes them all.
        let mut typed = Vec::new();
        let mut probed = false;
        for (receiver, span) in candidates {
            let mut alone = source.source_check_expression(source.expression_key_for_span(receiver), None, NativeCheckMode::empty(), queries);
            // A name's declared type, such as a method's `self`'s or an annotated parameter's,
            // needs no check.
            if untyped(&alone, queries)
                && let Some(occurrence) = source.bound().occurrence_at(receiver.start).filter(|occurrence| occurrence.span == receiver)
                && let Some(binding) = occurrence.binding.and_then(|binding| source.resolve_binding(binding, queries))
                && !untyped(&binding.constraint, queries)
            {
                alone = binding.constraint;
            }
            if untyped(&alone, queries) {
                source.probe_expression(receiver);
                probed = true;
            }
            typed.push((receiver, span, alone));
        }
        if probed { registry.check_owned_roots(queries, &[other.id]); }
        for (receiver, span, alone) in typed {
            let receiver = match untyped(&alone, queries) {
                true => source.probed_types(receiver).into_iter().next().unwrap_or(alone),
                false => alone,
            };
            let receiver = queries.exclude_type(&receiver, &Type::Nil);
            let found = member_sites(registry, &receiver, name, queries);
            if found.iter().any(|site| sites.contains(site)) {
                add_occurrence(navigation, &other.path, &other.source, span, declares(other.id, span));
                let Some(rename) = rename else { continue };
                let (line, column) = crate::symbols::line_column(&other.source, span.start);
                let at = format!("{}:{line}:{column}", other.path.display());
                // A use that may be another member of the same name too would be renamed for one
                // and not the other.
                if found.iter().any(|site| !sites.contains(site)) {
                    return Err(format!("the {name} at {at} may be another table's {name} too, so renaming it is not safe"));
                }
                if !member_sites(registry, &receiver, rename, queries).is_empty() {
                    return Err(format!("the value at {at} already has a member named {rename}"));
                }
            }
        }
    }
    Ok(())
}

/// Each member name written in `syntax`, with the span of the value it is a member of: in
/// `receiver.name`, `receiver:name(...)`, `receiver["name"]`, and a table constructor's
/// `name = value` and `["name"] = value`.
fn member_uses(syntax: &luanalysis_syntax::File, text: &str, visit: &mut dyn FnMut(Span, Name)) {
    crate::module_requests::visit_expressions(&syntax.statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Field { base, name } | ExprKind::MethodCall { receiver: base, name, .. } => visit(base.span, name.clone()),
        ExprKind::Index { base, index } => if let Some(name) = string_key(text, index) { visit(base.span, name) },
        ExprKind::Table(fields) => for field in fields {
            match &field.kind {
                luanalysis_syntax::TableFieldKind::Named(name) => visit(expression.span, name.clone()),
                luanalysis_syntax::TableFieldKind::Keyed(key) => if let Some(name) = string_key(text, key) { visit(expression.span, name) },
                luanalysis_syntax::TableFieldKind::Sequence { .. } => {}
            }
        },
        _ => {}
    });
}

/// A constant string key that is a Lua name, `"name"` or `'name'`, as the name inside its quotes.
fn string_key(text: &str, key: &Expr) -> Option<Name> {
    let ExprKind::String(bytes) = &key.kind else { return None };
    let written = text.get(key.span.start..key.span.end)?;
    let inner = written.strip_prefix('"').and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| written.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')))?;
    (inner.as_bytes() == bytes.as_slice() && crate::symbols::is_name(inner))
        .then(|| Name { text: inner.to_owned(), span: Span::new(key.span.start + 1, key.span.end - 1) })
}

/// The member or type name at `offset` in `file`, and with `name`, every occurrence renaming it
/// to `name` edits; or why it cannot be renamed. A library's names, such as a host API's, are
/// not the project's to rename, and a rename must not merge two members or types, nor make a
/// use refer to something else.
pub fn rename(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    name: Option<String>,
    deliver: impl FnOnce(Result<Option<Navigation>, String>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(renaming(project, registry, file, offset, name.as_deref(), queries));
    });
}

fn renaming(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    name: Option<&str>,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Result<Option<Navigation>, String> {
    let (Some(source), Some(text)) = (registry.source(file), project.file(file)) else { return Ok(None) };
    let text = &text.source;
    let names = luanalysis_syntax::type_names(source.syntax(), text);
    if let Some(at) = names.iter().find(|type_name| type_name.span.start <= offset && offset <= type_name.span.end) {
        let Some(navigation) = type_navigation(project, registry, file, at, &names, name.is_some()) else { return Ok(None) };
        owned_by_library(project, &navigation)?;
        if let Some(name) = name.filter(|name| *name != at.name) { type_rename_conflict(project, registry, file, at, &names, &navigation, name)?; }
        return Ok(Some(navigation));
    }
    registry.prepare_documentation_members(queries);
    let Some((member, span, sites)) = member_target(registry, file, text, offset, queries) else { return Ok(None) };
    let mut navigation = Navigation { name: member.clone(), span, ..Navigation::default() };
    for &(site_file, site, _) in &sites {
        let Some(site_text) = project.file(site_file) else { continue };
        add_occurrence(&mut navigation, &site_text.path, &site_text.source, word_in(&site_text.source, site, &member).unwrap_or(site), true);
    }
    owned_by_library(project, &navigation)?;
    let Some(name) = name else { return Ok(Some(navigation)) };
    if !crate::symbols::is_name(name) { return Err(format!("{name:?} is not a Lua name")); }
    let rename = (name != member).then_some(name);
    member_occurrences(project, registry, &member, &sites, &mut navigation, rename, queries)?;
    owned_by_library(project, &navigation)?;
    // A declaring tag's type, such as a `---@field`'s class, must not already have the name.
    if let Some(name) = rename {
        for &(site_file, site, tag) in &sites {
            let Some(owner) = tag.then(|| registry.source(site_file)).flatten().and_then(|source| tag_owner(source.syntax(), site)) else { continue };
            if project.files.iter().any(|other| registry.source(other.id).is_some_and(|source| tag_declares(source.syntax(), &owner, name))) {
                return Err(format!("{owner} already has a member named {name}"));
            }
        }
    }
    Ok(Some(navigation))
}

/// Refuses a rename with an occurrence in a library, such as a `--library` declaration file
/// or the standard library, which the project does not own.
fn owned_by_library(project: &Project, navigation: &Navigation) -> Result<(), String> {
    let library = |path: &std::path::PathBuf| project.file_by_path(path).is_some_and(|file| {
        file.role == luanalysis_project::SourceRole::Declaration || !matches!(file.origin, luanalysis_project::SourceOrigin::Filesystem)
    });
    match navigation.occurrences.iter().any(|(path, ..)| library(path)) {
        true => Err(format!("{} is declared by a library and cannot be renamed", navigation.name)),
        false => Ok(()),
    }
}

/// The name of the type whose member the `---@field` or `---@function` at `site` declares.
fn tag_owner(syntax: &luanalysis_syntax::File, site: Span) -> Option<String> {
    for declaration in &syntax.type_declarations {
        if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind
            && fields.iter().any(|field| field.span == site)
        {
            return Some(declaration.name.text.clone());
        }
    }
    if let Some(extension) = syntax.type_extensions.iter().find(|extension| extension.fields.iter().any(|field| field.span == site)) {
        return Some(extension.owner.text.clone());
    }
    let declaration = syntax.function_declarations.iter().find(|declaration| declaration.span == site)?;
    match declaration.owner.as_ref()? {
        luanalysis_syntax::DocFunctionOwner::Explicit(owner) => Some(owner.text.clone()),
        luanalysis_syntax::DocFunctionOwner::Contextual(id) => syntax.type_declarations.iter().find(|declaration| declaration.id == *id).map(|declaration| declaration.name.text.clone()),
    }
}

/// Whether `syntax` declares a member `name` of type `owner` with a `---@field` or `---@function`.
fn tag_declares(syntax: &luanalysis_syntax::File, owner: &str, name: &str) -> bool {
    let named = |key: &luanalysis_syntax::TypeFieldKey| matches!(key, luanalysis_syntax::TypeFieldKey::Named(field) if field.text == name);
    syntax.type_declarations.iter().any(|declaration| declaration.name.text == owner && matches!(&declaration.kind,
        luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } if fields.iter().any(|field| named(&field.key))))
        || syntax.type_extensions.iter().any(|extension| extension.owner.text == owner && extension.fields.iter().any(|field| named(&field.key)))
        || syntax.function_declarations.iter().any(|declaration| declaration.name.text == name && match &declaration.owner {
            Some(luanalysis_syntax::DocFunctionOwner::Explicit(explicit)) => explicit.text == owner,
            Some(luanalysis_syntax::DocFunctionOwner::Contextual(id)) => syntax.type_declarations.iter().any(|declaration| declaration.id == *id && declaration.name.text == owner),
            None => false,
        })
}

/// Why renaming type name `at` of `file` (whose type names are `names`) to `name` would change
/// what a type name refers to: the name is built in or already a type's, a `---@generic` of the
/// name would capture a renamed use, or a renamed `---@generic`'s block already uses the name.
fn type_rename_conflict(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    at: &luanalysis_syntax::TypeName,
    names: &[luanalysis_syntax::TypeName],
    navigation: &Navigation,
    name: &str,
) -> Result<(), String> {
    use luanalysis_syntax::TypeNameKind;
    const BUILT_IN: [&str; 12] = ["any", "unknown", "never", "nil", "boolean", "number", "integer", "string", "table", "function", "void", "self"];
    if !name.split('.').all(crate::symbols::is_name) { return Err(format!("{name:?} is not a type name")); }
    if BUILT_IN.contains(&name) { return Err(format!("{name} is a built-in type")); }
    // A `---@generic` and its uses are its documentation block's alone.
    let generic = names.iter().any(|other| other.kind == TypeNameKind::Generic && other.block == at.block && other.name == at.name);
    for other in project.files.iter().filter(|other| !generic || other.id == file) {
        let Some(syntax) = registry.source(other.id).map(|source| source.syntax()) else { continue };
        let names = luanalysis_syntax::type_names(syntax, &other.source);
        let generic_in = |block: Span| names.iter().any(|type_name| type_name.kind == TypeNameKind::Generic && type_name.block == block && type_name.name == name);
        let renamed = |type_name: &luanalysis_syntax::TypeName| navigation.occurrences.iter().any(|(path, span, _)| *path == other.path && *span == type_name.span);
        let conflict = names.iter().find(|type_name| match () {
            _ if generic => type_name.block == at.block && type_name.name == name,
            _ if renamed(type_name) => generic_in(type_name.block),
            _ => type_name.name == name && !generic_in(type_name.block),
        });
        if let Some(conflict) = conflict {
            let (line, column) = crate::symbols::line_column(&other.source, conflict.span.start);
            let at = format!("{}:{line}:{column}", other.path.display());
            return Err(match conflict.kind {
                TypeNameKind::Declaration if !generic => format!("a type named {name} is already declared at {at}"),
                _ => format!("renaming to {name} would change what the type name at {at} refers to"),
            });
        }
    }
    Ok(())
}


/// What a document symbol is, as the outline shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Interface,
    Alias,
    Module,
    Field,
    Function,
    Method,
    Variable,
    Constant,
}

/// A declaration in a document's outline, with the declarations inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: SymbolKind,
    /// The whole declaration, and its name within it.
    pub range: Span,
    pub selection: Span,
    pub children: Vec<DocumentSymbol>,
}

/// The outline of a Lua source: its classes, shapes and aliases with their fields, its functions
/// and methods with the functions inside them, and its top-level locals and globals, with a
/// table constructor's keys.
pub fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    let file = luanalysis_syntax::parse(text);
    let slice = |span: Span| text.get(span.start..span.end).unwrap_or_default().to_owned();
    let mut symbols = Vec::new();
    for declaration in &file.type_declarations {
        let (kind, fields) = match &declaration.kind {
            luanalysis_syntax::NamedTypeDeclarationKind::Alias { target } => (SymbolKind::Alias, (Vec::new(), Some(slice(target.span)))),
            luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor, fields, .. } => {
                let kind = match flavor {
                    luanalysis_syntax::NamedRecordKind::Class => SymbolKind::Class,
                    luanalysis_syntax::NamedRecordKind::Shape => SymbolKind::Interface,
                    luanalysis_syntax::NamedRecordKind::Module => SymbolKind::Module,
                };
                let children = fields.iter().filter_map(|field| match &field.key {
                    luanalysis_syntax::TypeFieldKey::Named(name) => Some(DocumentSymbol {
                        name: name.text.clone(), detail: Some(slice(field.ty.span)), kind: SymbolKind::Field,
                        range: field.span, selection: name.span, children: Vec::new(),
                    }),
                    _ => None,
                }).collect();
                (kind, (children, None))
            }
        };
        let range = declaration.span.join(declaration.name.span);
        let range = fields.0.iter().fold(range, |range, field| range.join(field.range));
        symbols.push(DocumentSymbol { name: declaration.name.text.clone(), detail: fields.1, kind, range, selection: declaration.name.span, children: fields.0 });
    }
    statement_symbols(&file.statements, text, true, &mut symbols);
    symbols.sort_by_key(|symbol| (symbol.range.start, symbol.range.end));
    symbols
}

fn statement_symbols(statements: &[luanalysis_syntax::Stmt], text: &str, top: bool, symbols: &mut Vec<DocumentSymbol>) {
    use luanalysis_syntax::StmtKind;
    let slice = |span: Span| text.get(span.start..span.end).unwrap_or_default().to_owned();
    for statement in statements {
        match &statement.kind {
            StmtKind::LocalFunction { name, function } => symbols.push(function_symbol(name.text.clone(), SymbolKind::Function, statement.span, name.span, function, text)),
            StmtKind::FunctionDeclaration { target, function, method_span } => {
                let (kind, selection) = match &target.kind {
                    ExprKind::Field { name, .. } => (if method_span.is_some() { SymbolKind::Method } else { SymbolKind::Function }, name.span),
                    _ => (SymbolKind::Function, target.span),
                };
                symbols.push(function_symbol(slice(target.span), kind, statement.span, selection, function, text));
            }
            StmtKind::Local { names, attributes, values, .. } if top => {
                for (index, name) in names.iter().enumerate() {
                    let constant = attributes.get(index).and_then(Option::as_ref).is_some_and(|attribute| attribute.text == "const");
                    symbols.push(value_symbol(name.text.clone(), if constant { SymbolKind::Constant } else { SymbolKind::Variable }, statement.span, name.span, values.get(index), text));
                }
            }
            StmtKind::Local { values, .. } => for value in values { nested_function_symbols(value, text, symbols); },
            StmtKind::Assign { targets, values, .. } => {
                for (index, target) in targets.iter().enumerate() {
                    let selection = match &target.kind {
                        ExprKind::Name(_) if top => target.span,
                        ExprKind::Field { name, .. } if top => name.span,
                        _ => {
                            if let Some(value) = values.get(index) { nested_function_symbols(value, text, symbols); }
                            continue;
                        }
                    };
                    let kind = if matches!(target.kind, ExprKind::Field { .. }) { SymbolKind::Field } else { SymbolKind::Variable };
                    symbols.push(value_symbol(slice(target.span), kind, statement.span, selection, values.get(index), text));
                }
            }
            StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => statement_symbols(body, text, false, symbols),
            StmtKind::If { branches, else_branch } => {
                for branch in branches { statement_symbols(&branch.body, text, false, symbols); }
                if let Some(branch) = else_branch { statement_symbols(&branch.body, text, false, symbols); }
            }
            StmtKind::Call(call) => nested_function_symbols(call, text, symbols),
            StmtKind::Return(values) => for value in values { nested_function_symbols(value, text, symbols); },
            _ => {}
        }
    }
}

fn function_symbol(name: String, kind: SymbolKind, range: Span, selection: Span, function: &luanalysis_syntax::Function, text: &str) -> DocumentSymbol {
    let parameters = text.get(function.span.start..function.body_span.start).and_then(|header| {
        let open = header.find('(')?;
        Some(header[open..].trim_end().to_owned())
    });
    let mut children = Vec::new();
    statement_symbols(&function.body, text, false, &mut children);
    DocumentSymbol { name, detail: parameters, kind, range, selection, children }
}

/// A local's, global's or field's symbol: a function's if it holds one, with a table constructor's
/// keys as its children.
fn value_symbol(name: String, kind: SymbolKind, range: Span, selection: Span, value: Option<&Expr>, text: &str) -> DocumentSymbol {
    match value.map(|value| &value.kind) {
        Some(ExprKind::Function(function)) => function_symbol(name, if kind == SymbolKind::Field { SymbolKind::Method } else { SymbolKind::Function }, range, selection, function, text),
        Some(ExprKind::Table(fields)) => {
            let children = fields.iter().filter_map(|field| match &field.kind {
                luanalysis_syntax::TableFieldKind::Named(key) => Some(value_symbol(key.text.clone(), SymbolKind::Field, field.span, key.span, Some(&field.value), text)),
                _ => None,
            }).collect();
            DocumentSymbol { name, detail: None, kind, range, selection, children }
        }
        _ => DocumentSymbol { name, detail: None, kind, range, selection, children: Vec::new() },
    }
}

/// The named functions inside the anonymous functions an expression holds, such as a callback's.
fn nested_function_symbols(expression: &Expr, text: &str, symbols: &mut Vec<DocumentSymbol>) {
    match &expression.kind {
        ExprKind::Function(function) => statement_symbols(&function.body, text, false, symbols),
        ExprKind::Call { callee, args } => {
            nested_function_symbols(callee, text, symbols);
            for arg in args { nested_function_symbols(arg, text, symbols); }
        }
        ExprKind::MethodCall { receiver, args, .. } => {
            nested_function_symbols(receiver, text, symbols);
            for arg in args { nested_function_symbols(arg, text, symbols); }
        }
        ExprKind::Table(fields) => for field in fields { nested_function_symbols(&field.value, text, symbols); },
        ExprKind::Parenthesized(inner) | ExprKind::Field { base: inner, .. } => nested_function_symbols(inner, text, symbols),
        ExprKind::Binary { left, right, .. } => {
            nested_function_symbols(left, text, symbols);
            nested_function_symbols(right, text, symbols);
        }
        _ => {}
    }
}

/// A range of a document that an editor can fold: from `start` to `end` (byte offsets), with
/// whether it is a comment or a `--region`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub start: usize,
    pub end: usize,
    pub kind: Option<FoldKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Comment,
    Region,
}

/// The foldable ranges of a Lua source: functions, blocks, multi-line tables, runs of
/// documentation comments, long comments, and `--region` to `--endregion` (or `--{{{` to
/// `--}}}`), as v1 folds them.
pub fn folding_ranges(text: &str) -> Vec<Fold> {
    let file = luanalysis_syntax::parse(text);
    let mut folds = Vec::new();
    statement_folds(&file.statements, &mut folds);
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| {
        if matches!(expression.kind, ExprKind::Table(_) | ExprKind::Function(_)) {
            folds.push(Fold { start: expression.span.start, end: expression.span.end, kind: None });
        }
    });
    let mut regions = Vec::new();
    let mut run: Option<Span> = None;
    for comment in &file.comments {
        let body = comment.text.trim_start_matches('-').trim_start_matches('#');
        let line_comment = comment.text.starts_with("--") && !comment.text.starts_with("--[");
        if line_comment && (body.starts_with("region") || comment.text.starts_with("--{{{")) {
            regions.push(comment.span.start);
            continue;
        }
        if line_comment && (body.starts_with("endregion") || comment.text.starts_with("--}}}")) {
            if let Some(start) = regions.pop() { folds.push(Fold { start, end: comment.span.end, kind: Some(FoldKind::Region) }); }
            continue;
        }
        if !line_comment {
            folds.push(Fold { start: comment.span.start, end: comment.span.end, kind: Some(FoldKind::Comment) });
            continue;
        }
        // Consecutive line comments fold together.
        run = Some(match run {
            Some(span) if text.get(span.end..comment.span.start).is_some_and(|gap| gap.trim().is_empty() && gap.matches('\n').count() <= 1) => span.join(comment.span),
            Some(span) => {
                folds.push(Fold { start: span.start, end: span.end, kind: Some(FoldKind::Comment) });
                comment.span
            }
            None => comment.span,
        });
    }
    if let Some(span) = run { folds.push(Fold { start: span.start, end: span.end, kind: Some(FoldKind::Comment) }); }
    // Only what spans lines folds.
    folds.retain(|fold| text.get(fold.start..fold.end).is_some_and(|folded| folded.trim_end().contains('\n')));
    folds.sort_by_key(|fold| (fold.start, fold.end));
    folds.dedup_by_key(|fold| (fold.start, fold.end));
    folds
}

fn statement_folds(statements: &[luanalysis_syntax::Stmt], folds: &mut Vec<Fold>) {
    use luanalysis_syntax::StmtKind;
    for statement in statements {
        match &statement.kind {
            StmtKind::LocalFunction { .. } | StmtKind::FunctionDeclaration { .. } | StmtKind::Block { .. }
                | StmtKind::While { .. } | StmtKind::Repeat { .. } | StmtKind::NumericFor { .. } | StmtKind::GenericFor { .. } => {
                folds.push(Fold { start: statement.span.start, end: statement.span.end, kind: None });
            }
            StmtKind::If { branches, else_branch } => {
                // Each branch folds to the next, so `else` stays visible.
                for branch in branches { folds.push(Fold { start: branch.span.start, end: branch.body_span.end, kind: None }); }
                if let Some(branch) = else_branch { folds.push(Fold { start: branch.span.start, end: branch.body_span.end, kind: None }); }
            }
            _ => {}
        }
        match &statement.kind {
            StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => statement_folds(&function.body, folds),
            StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => statement_folds(body, folds),
            StmtKind::If { branches, else_branch } => {
                for branch in branches { statement_folds(&branch.body, folds); }
                if let Some(branch) = else_branch { statement_folds(&branch.body, folds); }
            }
            _ => {}
        }
    }
}

/// For each offset, the spans that an editor's selection grows through: the name or expression
/// there, each expression and statement around it, and the whole source.
pub fn selection_ranges(text: &str, offsets: &[usize]) -> Vec<Vec<Span>> {
    let file = luanalysis_syntax::parse(text);
    let mut spans = Vec::new();
    statement_spans(&file.statements, &mut spans);
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| {
        spans.push(expression.span);
        if let ExprKind::Field { name, .. } | ExprKind::MethodCall { name, .. } = &expression.kind { spans.push(name.span); }
    });
    spans.push(Span { start: 0, end: text.len() });
    offsets.iter().map(|&offset| {
        let mut around: Vec<Span> = spans.iter().copied().filter(|span| span.start <= offset && offset <= span.end && span.start < span.end).collect();
        around.sort_by_key(|span| (span.end - span.start, std::cmp::Reverse(span.start)));
        around.dedup();
        // Each range must contain the one before it.
        let mut chain: Vec<Span> = Vec::new();
        for span in around {
            if chain.last().is_none_or(|inner| span.start <= inner.start && inner.end <= span.end && span != *inner) { chain.push(span); }
        }
        chain
    }).collect()
}

fn statement_spans(statements: &[luanalysis_syntax::Stmt], spans: &mut Vec<Span>) {
    use luanalysis_syntax::StmtKind;
    for statement in statements {
        spans.push(statement.span);
        match &statement.kind {
            StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => {
                spans.push(function.body_span);
                statement_spans(&function.body, spans);
            }
            StmtKind::Block { body, body_span } | StmtKind::While { body, body_span, .. } | StmtKind::Repeat { body, body_span, .. }
                | StmtKind::NumericFor { body, body_span, .. } | StmtKind::GenericFor { body, body_span, .. } => {
                spans.push(*body_span);
                statement_spans(body, spans);
            }
            StmtKind::If { branches, else_branch } => {
                for branch in branches { spans.push(branch.span); spans.push(branch.body_span); statement_spans(&branch.body, spans); }
                if let Some(branch) = else_branch { spans.push(branch.span); spans.push(branch.body_span); statement_spans(&branch.body, spans); }
            }
            _ => {}
        }
    }
}

/// The names a source declares without a written type whose type is not plain from their
/// initializer: locals without `---@type` holding something other than a literal, a table or a
/// function, and generic `for` variables. An inlay hint shows each one's type.
pub fn untyped_declarations(text: &str) -> Vec<Span> {
    fn collect(statements: &[luanalysis_syntax::Stmt], names: &mut Vec<Span>) {
        use luanalysis_syntax::StmtKind;
        for statement in statements {
            match &statement.kind {
                StmtKind::Local { names: declared, values, annotation: None, .. } => {
                    for (index, name) in declared.iter().enumerate() {
                        // Extra names take the last value's further results.
                        let value = values.get(index).or(if index >= values.len() { values.last() } else { None });
                        let plain = value.is_some_and(|value| matches!(value.kind,
                            ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_) | ExprKind::Table(_) | ExprKind::Function(_)));
                        if value.is_some() && !plain { names.push(name.span); }
                    }
                }
                StmtKind::GenericFor { names: declared, .. } => names.extend(declared.iter().map(|name| name.span)),
                _ => {}
            }
            match &statement.kind {
                StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => collect(&function.body, names),
                StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                    | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => collect(body, names),
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { collect(&branch.body, names); }
                    if let Some(branch) = else_branch { collect(&branch.body, names); }
                }
                _ => {}
            }
        }
    }
    let file = luanalysis_syntax::parse(text);
    let mut names = Vec::new();
    collect(&file.statements, &mut names);
    // Functions written as values hold statements too.
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| {
        if let ExprKind::Function(function) = &expression.kind { collect(&function.body, &mut names); }
    });
    names.sort_by_key(|span| span.start);
    names.dedup();
    names
}

/// The parameter names of the literal arguments of the calls in `range` of `file`: each
/// argument's offset and its parameter's name, as TypeScript's "literals" inlay hints show them.
pub fn parameter_hints(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    range: Range<usize>,
    deliver: impl FnOnce(Vec<(usize, String)>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |_, registry, queries| {
        deliver(literal_parameter_names(registry, file, range, queries));
    });
}

fn literal_parameter_names(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    range: Range<usize>,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<(usize, String)> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    // As TypeScript's "literals" hints: scalars, not tables or functions.
    let literal = |argument: &Expr| matches!(argument.kind, ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_))
        || matches!(&argument.kind, ExprKind::Unary { value, .. } if matches!(value.kind, ExprKind::Number(_)));
    let mut calls = Vec::new();
    crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
        if expression.span.end < range.start || expression.span.start > range.end { return; }
        match &expression.kind {
            ExprKind::Call { callee, args } if args.iter().any(literal) => calls.push((callee.span, None, args.clone())),
            ExprKind::MethodCall { receiver, name, args } if args.iter().any(literal) => calls.push((receiver.span, Some(name.text.clone()), args.clone())),
            _ => {}
        }
    });
    let mut hints = Vec::new();
    for (callee, method, args) in calls {
        let alone = |span: Span, queries: &mut dyn luanalysis_engine::TypeQueries| {
            source.source_check_expression(source.expression_key_for_span(span), None, NativeCheckMode::empty(), queries)
        };
        let value = match &method {
            None => alone(callee, queries),
            Some(name) => {
                let receiver = alone(callee, queries);
                let receiver = queries.exclude_type(&receiver, &Type::Nil);
                let Some((_, member)) = queries.type_members(&receiver).into_iter().find(|(member, _)| member == name) else { continue };
                member
            }
        };
        let hidden = usize::from(method.is_some());
        // The first overload that takes as many arguments.
        let mut names: Option<Vec<String>> = None;
        for head in queries.call_signature_types(&value) {
            let Some((parameters, _)) = crate::describe_signature(&head, queries) else { continue };
            let shown: Vec<String> = parameters.iter().skip(hidden).map(|parameter| parameter.split(':').next().unwrap_or_default().trim_end_matches('?').to_owned()).collect();
            let rest = shown.last().is_some_and(|name| name == "...");
            if rest || shown.len() >= args.len() { names = Some(shown); break; }
        }
        let Some(names) = names else { continue };
        for (argument, name) in args.iter().zip(&names) {
            if literal(argument) && name != "..." && !name.is_empty() && range.contains(&argument.span.start) {
                hints.push((argument.span.start, name.clone()));
            }
        }
    }
    hints
}

/// What a semantic token is, in the order of the language server's legend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Class,
    Interface,
    Type,
    TypeParameter,
    Parameter,
    Variable,
    Property,
    Function,
    Method,
}

/// Semantic token modifiers, as bits in the order of the legend.
pub const MODIFIER_DECLARATION: u32 = 1;
pub const MODIFIER_READONLY: u32 = 2;
pub const MODIFIER_DEFAULT_LIBRARY: u32 = 4;
pub const MODIFIER_GLOBAL: u32 = 8;

const STANDARD_GLOBALS: [&str; 40] = [
    "_G", "_VERSION", "_ENV", "assert", "collectgarbage", "dofile", "error", "getmetatable", "ipairs", "load",
    "loadfile", "next", "pairs", "pcall", "print", "rawequal", "rawget", "rawlen", "rawset", "require",
    "select", "setmetatable", "tonumber", "tostring", "type", "xpcall", "unpack", "loadstring", "setfenv", "getfenv",
    "coroutine", "debug", "io", "math", "os", "package", "string", "table", "utf8", "bit32",
];
const BUILTIN_TYPES: [&str; 14] = ["nil", "boolean", "number", "integer", "string", "table", "function", "userdata", "thread", "any", "unknown", "never", "void", "self"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticToken {
    pub span: Span,
    pub kind: TokenType,
    pub modifiers: u32,
}

/// The semantic tokens of a Lua source: what each name is, from its binding (a parameter, a
/// local, a function, a global, the standard library's), its place (a field, a method, a table
/// key) and in annotations, a type's declaring tag or a generic parameter.
pub fn semantic_tokens(text: &str) -> Vec<SemanticToken> {
    use std::collections::HashSet;
    let file = luanalysis_syntax::parse(text);
    let bound = bind::bind(&file);
    let mut tokens = Vec::new();
    // Names called, and locals holding functions.
    let mut called: HashSet<(usize, usize)> = HashSet::new();
    let mut functions: HashSet<(usize, usize)> = HashSet::new();
    let mut members: Vec<(Span, TokenType, u32)> = Vec::new();
    // Declarations first: a `function M:f()` target is also a field expression.
    declarations(&file.statements, &mut functions, &mut members);
    crate::module_requests::visit_expressions(&file.statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Call { callee, .. } => {
            let span = match &callee.kind { ExprKind::Field { name, .. } => name.span, _ => callee.span };
            called.insert((span.start, span.end));
        }
        ExprKind::Field { name, .. } => members.push((name.span, TokenType::Property, 0)),
        ExprKind::MethodCall { name, .. } => members.push((name.span, TokenType::Method, 0)),
        ExprKind::Table(fields) => for field in fields {
            if let luanalysis_syntax::TableFieldKind::Named(key) = &field.kind {
                let kind = if matches!(field.value.kind, ExprKind::Function(_)) { TokenType::Method } else { TokenType::Property };
                members.push((key.span, kind, MODIFIER_DECLARATION));
            }
        },
        _ => {}
    });
    fn declarations(statements: &[luanalysis_syntax::Stmt], functions: &mut std::collections::HashSet<(usize, usize)>, members: &mut Vec<(Span, TokenType, u32)>) {
        use luanalysis_syntax::StmtKind;
        for statement in statements {
            match &statement.kind {
                StmtKind::Local { names, values, .. } => for (name, value) in names.iter().zip(values) {
                    if matches!(value.kind, ExprKind::Function(_)) { functions.insert((name.span.start, name.span.end)); }
                },
                StmtKind::FunctionDeclaration { target, function, .. } => {
                    if let ExprKind::Field { name, .. } = &target.kind { members.push((name.span, TokenType::Method, MODIFIER_DECLARATION)); }
                    declarations(&function.body, functions, members);
                }
                StmtKind::LocalFunction { function, .. } => declarations(&function.body, functions, members),
                StmtKind::Block { body, .. } | StmtKind::While { body, .. } | StmtKind::Repeat { body, .. }
                    | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => declarations(body, functions, members),
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { declarations(&branch.body, functions, members); }
                    if let Some(branch) = else_branch { declarations(&branch.body, functions, members); }
                }
                _ => {}
            }
        }
    }
    for occurrence in &bound.occurrences {
        let key = (occurrence.span.start, occurrence.span.end);
        let declaration_modifier = if occurrence.kind == bind::OccurrenceKind::Declaration { MODIFIER_DECLARATION } else { 0 };
        let (kind, modifiers) = match occurrence.binding.map(|binding| bound.declaration(binding)) {
            Some(declaration) => {
                let declared = (declaration.span.start, declaration.span.end);
                let kind = match declaration.kind {
                    bind::DeclarationKind::Parameter | bind::DeclarationKind::ImplicitReceiver => TokenType::Parameter,
                    bind::DeclarationKind::LocalFunction => TokenType::Function,
                    _ if functions.contains(&declared) || called.contains(&key) => TokenType::Function,
                    _ => TokenType::Variable,
                };
                (kind, declaration_modifier | if declaration.constant { MODIFIER_READONLY } else { 0 })
            }
            None => {
                let library = if STANDARD_GLOBALS.contains(&occurrence.name.as_str()) { MODIFIER_DEFAULT_LIBRARY } else { 0 };
                let kind = if called.contains(&key) { TokenType::Function } else { TokenType::Variable };
                (kind, MODIFIER_GLOBAL | library | declaration_modifier)
            }
        };
        tokens.push(SemanticToken { span: occurrence.span, kind, modifiers });
    }
    for (span, kind, modifiers) in members {
        let kind = if kind == TokenType::Property && called.contains(&(span.start, span.end)) { TokenType::Method } else { kind };
        tokens.push(SemanticToken { span, kind, modifiers });
    }
    for declaration in &file.type_declarations {
        if let luanalysis_syntax::NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind {
            for field in fields {
                if let luanalysis_syntax::TypeFieldKey::Named(name) = &field.key { tokens.push(SemanticToken { span: name.span, kind: TokenType::Property, modifiers: MODIFIER_DECLARATION }); }
            }
        }
    }
    let names = luanalysis_syntax::type_names(&file, text);
    for name in &names {
        let generic = names.iter().any(|other| other.kind == luanalysis_syntax::TypeNameKind::Generic && other.block == name.block && other.name == name.name);
        let (kind, modifiers) = match name.kind {
            luanalysis_syntax::TypeNameKind::Generic => (TokenType::TypeParameter, MODIFIER_DECLARATION),
            _ if generic => (TokenType::TypeParameter, 0),
            luanalysis_syntax::TypeNameKind::Declaration => {
                let declared = file.type_declarations.iter().find(|declaration| declaration.name.span == name.span).map(|declaration| &declaration.kind);
                let kind = match declared {
                    Some(luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor: luanalysis_syntax::NamedRecordKind::Shape, .. }) => TokenType::Interface,
                    Some(luanalysis_syntax::NamedTypeDeclarationKind::Record { .. }) => TokenType::Class,
                    _ => TokenType::Type,
                };
                (kind, MODIFIER_DECLARATION)
            }
            _ if BUILTIN_TYPES.contains(&name.name.as_str()) => (TokenType::Type, MODIFIER_DEFAULT_LIBRARY),
            _ => {
                let declared = file.type_declarations.iter().find(|declaration| declaration.name.text == name.name).map(|declaration| &declaration.kind);
                match declared {
                    Some(luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor: luanalysis_syntax::NamedRecordKind::Shape, .. }) => (TokenType::Interface, 0),
                    Some(luanalysis_syntax::NamedTypeDeclarationKind::Record { .. }) => (TokenType::Class, 0),
                    _ => (TokenType::Type, 0),
                }
            }
        };
        tokens.push(SemanticToken { span: name.span, kind, modifiers });
    }
    // Only names are tokens; an implicit `self` is declared by its method's `:`.
    tokens.retain(|token| text.get(token.span.start..token.span.end)
        .is_some_and(|name| name.starts_with(|ch: char| ch.is_alphabetic() || ch == '_') && !name.contains('\n')));
    tokens.sort_by_key(|token| (token.span.start, token.span.end));
    // Overlapping tokens keep the first.
    let mut kept: Vec<SemanticToken> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if kept.last().is_none_or(|last| last.span.end <= token.span.start) { kept.push(token); }
    }
    kept
}

/// A quick fix: replace `span` with `replacement` to resolve diagnostic number `diagnostic`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub diagnostic: usize,
    pub span: Span,
    pub replacement: String,
}

/// Spelling fixes for the diagnostics given as (code, span): a missing property (TS2339) is
/// changed to its receiver's nearest member, and an unknown name (TS2304) to the nearest local
/// in scope or global.
pub fn spelling_fixes(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    diagnostics: Vec<(String, Span)>,
    deliver: impl FnOnce(Vec<Fix>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |_, registry, queries| {
        deliver(spellings(registry, file, &diagnostics, queries));
    });
}

fn spellings(
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    diagnostics: &[(String, Span)],
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Vec<Fix> {
    let Some(source) = registry.source(file) else { return Vec::new() };
    let mut fixes = Vec::new();
    for (index, (code, span)) in diagnostics.iter().enumerate() {
        let overlaps = |other: Span| other.start < span.end && span.start < other.end;
        let (word, candidates): (Name, Vec<String>) = match code.as_str() {
            "TS2339" => {
                let mut found = None;
                crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
                    ExprKind::Field { base, name } | ExprKind::MethodCall { receiver: base, name, .. } if overlaps(name.span) => found = Some((base.span, name.clone())),
                    _ => {}
                });
                let Some((receiver, name)) = found else { continue };
                let Some(receiver) = expression_type(registry, file, receiver, queries) else { continue };
                let receiver = queries.exclude_type(&receiver, &Type::Nil);
                (name, queries.type_members(&receiver).into_iter().map(|(member, _)| member).collect())
            }
            "TS2304" => {
                let mut found = None;
                crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| {
                    if let ExprKind::Name(name) = &expression.kind && overlaps(expression.span) { found = Some(name.clone()); }
                });
                let Some(name) = found else { continue };
                let bound = source.bound();
                let mut candidates: Vec<String> = visible_bindings(bound, name.span.start).into_iter().map(|binding| bound.declaration(binding).name.clone()).collect();
                if let Some(environment) = source.global_environment()
                    && let Ok(environment) = queries.global_environment_type(environment)
                {
                    candidates.extend(queries.type_members(&environment).into_iter().map(|(member, _)| member));
                }
                (name, candidates)
            }
            _ => continue,
        };
        if let Some(best) = nearest(&word.text, &candidates) {
            fixes.push(Fix { title: format!("Change to '{best}'"), diagnostic: index, span: word.span, replacement: best });
        }
    }
    fixes
}

/// The candidate TypeScript would suggest for a misspelled `word`.
fn nearest(word: &str, candidates: &[String]) -> Option<String> {
    luanalysis_engine::spelling_suggestion(word, candidates.iter().map(String::as_str).filter(|candidate| is_identifier(candidate)))
        .map(str::to_owned)
}

/// A function in a call hierarchy, or a file's main chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallItem {
    pub name: String,
    pub kind: SymbolKind,
    pub chunk: bool,
    pub path: std::path::PathBuf,
    pub span: Span,
    pub selection: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallDirection {
    Prepare,
    Incoming,
    Outgoing,
}

/// A call hierarchy: the function at a position, and in a direction, the functions that call
/// it or that it calls, each with the spans of the calls in the caller's file.
#[derive(Clone, Debug, Default)]
pub struct CallHierarchy {
    pub item: Option<CallItem>,
    pub calls: Vec<(CallItem, Vec<Span>)>,
    pub texts: std::collections::HashMap<std::path::PathBuf, String>,
}

/// The call hierarchy of the function declared or called at `offset` in `file`. Functions are
/// identified by their value, not their names, so a local, a global and a method alike are
/// followed through their calls.
pub fn call_hierarchy(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    direction: CallDirection,
    deliver: impl FnOnce(Option<CallHierarchy>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, queries| {
        deliver(calls(project, registry, file, offset, direction, queries));
    });
}

/// Each function in a source with its key, and where it is declared: a statement's name (local,
/// global, field or method), a local's or a table key's.
fn source_functions(source: &crate::native_functions::NativeRegistry) -> Vec<(Span, luanalysis_engine::NativeFunctionKey, Option<(String, Span, Span, SymbolKind)>)> {
    fn statements(statements: &[luanalysis_syntax::Stmt], source: &crate::native_functions::NativeRegistry, text: &mut Vec<(Span, luanalysis_engine::NativeFunctionKey, Option<(String, Span, Span, SymbolKind)>)>) {
        use luanalysis_syntax::StmtKind;
        for statement in statements {
            match &statement.kind {
                StmtKind::LocalFunction { name, function } => {
                    text.push((function.span, source.key_for(function), Some((name.text.clone(), statement.span, name.span, SymbolKind::Function))));
                }
                StmtKind::FunctionDeclaration { target, function, method_span } => {
                    let (name, selection, kind) = match &target.kind {
                        ExprKind::Field { name, .. } => (name.text.clone(), name.span, if method_span.is_some() { SymbolKind::Method } else { SymbolKind::Function }),
                        ExprKind::Name(name) => (name.text.clone(), name.span, SymbolKind::Function),
                        _ => continue,
                    };
                    text.push((function.span, source.key_for(function), Some((name, statement.span, selection, kind))));
                }
                StmtKind::Local { names, values, .. } => for (name, value) in names.iter().zip(values) {
                    if let ExprKind::Function(function) = &value.kind {
                        text.push((function.span, source.key_for(function), Some((name.text.clone(), statement.span, name.span, SymbolKind::Function))));
                    }
                },
                _ => {}
            }
        }
    }
    let mut functions = Vec::new();
    let syntax = source.syntax();
    statements(&syntax.statements, source, &mut functions);
    crate::module_requests::visit_expressions(&syntax.statements, &mut |expression: &Expr| match &expression.kind {
        ExprKind::Function(function) => {
            let key = source.key_for(function);
            if !functions.iter().any(|(_, other, _)| *other == key) { functions.push((function.span, key, None)); }
        }
        ExprKind::Table(fields) => for field in fields {
            if let (luanalysis_syntax::TableFieldKind::Named(name), ExprKind::Function(function)) = (&field.kind, &field.value.kind) {
                functions.push((function.span, source.key_for(function), Some((name.text.clone(), field.span, name.span, SymbolKind::Method))));
            }
        },
        _ => {}
    });
    // Visiting statements also visits the functions they declare; each function once.
    let mut seen = std::collections::HashSet::new();
    functions.retain(|(_, key, _)| seen.insert(*key));
    functions
}

fn call_item(project: &Project, registry: &crate::native_functions::project_provider::ProjectRegistry, key: luanalysis_engine::NativeFunctionKey, texts: &mut std::collections::HashMap<std::path::PathBuf, String>) -> Option<CallItem> {
    let (file, span) = registry.function_site(key)?;
    let source = project.file(file)?;
    texts.entry(source.path.clone()).or_insert_with(|| source.source.clone());
    let functions = source_functions(registry.source(file)?);
    let (name, range, selection, kind) = functions.into_iter().find(|(function, _, _)| *function == span)
        .and_then(|(_, _, declared)| declared)
        .unwrap_or_else(|| ("function".to_owned(), span, Span { start: span.start, end: span.start + "function".len().min(span.end - span.start) }, SymbolKind::Function));
    Some(CallItem { name, kind, chunk: false, path: source.path.clone(), span: range, selection })
}

fn calls(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    direction: CallDirection,
    queries: &mut dyn luanalysis_engine::TypeQueries,
) -> Option<CallHierarchy> {
    let source = registry.source(file)?;
    let contains = |span: Span| span.start <= offset && offset <= span.end;
    registry.prepare_documentation_members(queries);
    // The function declared at the position, or failing that, called there.
    let callee_key = |source: &crate::native_functions::NativeRegistry, callee: Span, method: Option<&str>, queries: &mut dyn luanalysis_engine::TypeQueries| {
        let value = source.source_check_expression(source.expression_key_for_span(callee), None, NativeCheckMode::empty(), queries);
        let value = match method {
            None => value,
            Some(name) => {
                let receiver = queries.exclude_type(&value, &Type::Nil);
                queries.type_members(&receiver).into_iter().find(|(member, _)| member == name)?.1
            }
        };
        queries.native_function_key(&value)
    };
    let declared = source_functions(source).into_iter()
        .find(|(_, _, declared)| declared.as_ref().is_some_and(|(_, _, selection, _)| contains(*selection)))
        .map(|(_, key, _)| key);
    let key = match declared {
        Some(key) => key,
        None => {
            let mut called = None;
            crate::module_requests::visit_expressions(&source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
                ExprKind::Call { callee, .. } if contains(callee.span) => called = Some((callee.span, None)),
                ExprKind::MethodCall { receiver, name, .. } if contains(name.span) => called = Some((receiver.span, Some(name.text.clone()))),
                _ => {}
            });
            let (callee, method) = called?;
            callee_key(source, callee, method.as_deref(), queries)?
        }
    };
    let mut hierarchy = CallHierarchy::default();
    let item = call_item(project, registry, key, &mut hierarchy.texts)?;
    let target_name = item.name.clone();
    hierarchy.item = Some(item);
    match direction {
        CallDirection::Prepare => {}
        CallDirection::Incoming => {
            for other in &project.files {
                let Some(other_source) = registry.source(other.id) else { continue };
                let functions = source_functions(other_source);
                let mut candidates = Vec::new();
                crate::module_requests::visit_expressions(&other_source.syntax().statements, &mut |expression: &Expr| match &expression.kind {
                    ExprKind::Call { callee, .. } => {
                        let name = match &callee.kind { ExprKind::Field { name, .. } => name.clone(), ExprKind::Name(name) => name.clone(), _ => return };
                        if name.text == target_name { candidates.push((callee.span, None, name.span)); }
                    }
                    ExprKind::MethodCall { receiver, name, .. } if name.text == target_name => candidates.push((receiver.span, Some(name.text.clone()), name.span)),
                    _ => {}
                });
                for (callee, method, span) in candidates {
                    if callee_key(other_source, callee, method.as_deref(), queries) != Some(key) { continue; }
                    // The innermost function around the call, or the file's main chunk.
                    let caller = functions.iter().filter(|(function, _, _)| function.start <= span.start && span.end <= function.end)
                        .min_by_key(|(function, _, _)| function.end - function.start);
                    let item = match caller {
                        Some((_, caller, _)) => call_item(project, registry, *caller, &mut hierarchy.texts),
                        None => {
                            hierarchy.texts.entry(other.path.clone()).or_insert_with(|| other.source.clone());
                            let name = other.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                            Some(CallItem { name, kind: SymbolKind::Module, chunk: true, path: other.path.clone(), span: Span { start: 0, end: other.source.len() }, selection: Span { start: 0, end: 0 } })
                        }
                    };
                    let Some(item) = item else { continue };
                    match hierarchy.calls.iter_mut().find(|(existing, _)| *existing == item) {
                        Some((_, spans)) => spans.push(span),
                        None => hierarchy.calls.push((item, vec![span])),
                    }
                }
            }
        }
        CallDirection::Outgoing => {
            let (target_file, body) = registry.function_site(key)?;
            let Some(target_source) = registry.source(target_file) else { return Some(hierarchy) };
            let functions = source_functions(target_source);
            // Calls in the function's own body, not in the functions inside it.
            let nested: Vec<Span> = functions.iter().map(|(span, _, _)| *span)
                .filter(|span| body.start < span.start && span.end <= body.end).collect();
            let mut candidates = Vec::new();
            crate::module_requests::visit_expressions(&target_source.syntax().statements, &mut |expression: &Expr| {
                let inside = body.start < expression.span.start && expression.span.end <= body.end
                    && !nested.iter().any(|span| span.start <= expression.span.start && expression.span.end <= span.end);
                if !inside { return; }
                match &expression.kind {
                    ExprKind::Call { callee, .. } => {
                        let span = match &callee.kind { ExprKind::Field { name, .. } => name.span, _ => callee.span };
                        candidates.push((callee.span, None, span));
                    }
                    ExprKind::MethodCall { receiver, name, .. } => candidates.push((receiver.span, Some(name.text.clone()), name.span)),
                    _ => {}
                }
            });
            for (callee, method, span) in candidates {
                let Some(called) = callee_key(target_source, callee, method.as_deref(), queries) else { continue };
                let Some(item) = call_item(project, registry, called, &mut hierarchy.texts) else { continue };
                match hierarchy.calls.iter_mut().find(|(existing, _)| *existing == item) {
                    Some((_, spans)) => spans.push(span),
                    None => hierarchy.calls.push((item, vec![span])),
                }
            }
        }
    }
    Some(hierarchy)
}

/// A named type in a type hierarchy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeItem {
    pub name: String,
    pub kind: SymbolKind,
    pub path: std::path::PathBuf,
    pub span: Span,
    pub selection: Span,
}

/// A type hierarchy: the class or shape named at `offset`, and in a direction, the types it
/// extends (supertypes: `true`) or that extend it.
pub fn type_hierarchy(
    project: Project,
    options: ProjectCheckOptions,
    workspace: &mut ProjectWorkspace,
    file: FileId,
    offset: usize,
    supertypes: Option<bool>,
    deliver: impl FnOnce(Option<(Vec<TypeItem>, std::collections::HashMap<std::path::PathBuf, String>)>) + Send,
) {
    let options = ProjectCheckOptions { jobs: 1, describe_types: true, inspections: false, ..options };
    crate::project_check::with_project_session(project, options, workspace, |project, registry, _| {
        deliver(types_related(project, registry, file, offset, supertypes));
    });
}

fn types_related(
    project: &Project,
    registry: &crate::native_functions::project_provider::ProjectRegistry,
    file: FileId,
    offset: usize,
    supertypes: Option<bool>,
) -> Option<(Vec<TypeItem>, std::collections::HashMap<std::path::PathBuf, String>)> {
    let source = project.file(file)?;
    let names = luanalysis_syntax::type_names(registry.source(file)?.syntax(), &source.source);
    let at = names.iter().find(|name| name.span.start <= offset && offset <= name.span.end)?;
    let mut texts = std::collections::HashMap::new();
    // Every record declaration in the project: its file, name, kind, span and base's names.
    let mut declarations = Vec::new();
    for other in &project.files {
        let Some(syntax) = registry.source(other.id).map(|source| source.syntax()) else { continue };
        for declaration in &syntax.type_declarations {
            let luanalysis_syntax::NamedTypeDeclarationKind::Record { flavor, base, .. } = &declaration.kind else { continue };
            let kind = if *flavor == luanalysis_syntax::NamedRecordKind::Shape { SymbolKind::Interface } else { SymbolKind::Class };
            let mut bases = Vec::new();
            let mut pending: Vec<&luanalysis_syntax::TypeExpr> = base.iter().collect();
            while let Some(ty) = pending.pop() {
                match &ty.kind {
                    luanalysis_syntax::TypeExprKind::Named(name) => bases.push(name.clone()),
                    luanalysis_syntax::TypeExprKind::Generic { base, .. } => bases.push(base.text.clone()),
                    luanalysis_syntax::TypeExprKind::Union(types) => pending.extend(types),
                    luanalysis_syntax::TypeExprKind::Parenthesized(inner) => pending.push(inner),
                    _ => {}
                }
            }
            declarations.push((other, TypeItem { name: declaration.name.text.clone(), kind, path: other.path.clone(), span: declaration.span.join(declaration.name.span), selection: declaration.name.span }, bases));
        }
    }
    let item = |(other, item, _): &(&luanalysis_project::SourceFile, TypeItem, Vec<String>), texts: &mut std::collections::HashMap<std::path::PathBuf, String>| {
        texts.entry(other.path.clone()).or_insert_with(|| other.source.clone());
        item.clone()
    };
    let items = match supertypes {
        None => declarations.iter().filter(|(_, item, _)| item.name == at.name).map(|declaration| item(declaration, &mut texts)).collect(),
        Some(true) => {
            let bases: Vec<String> = declarations.iter().filter(|(_, item, _)| item.name == at.name).flat_map(|(_, _, bases)| bases.clone()).collect();
            declarations.iter().filter(|(_, item, _)| bases.contains(&item.name)).map(|declaration| item(declaration, &mut texts)).collect()
        }
        Some(false) => declarations.iter().filter(|(_, _, bases)| bases.contains(&at.name)).map(|declaration| item(declaration, &mut texts)).collect(),
    };
    Some((items, texts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(text: &str) -> Option<CompletionSite> {
        let offset = text.find('|').expect("a cursor");
        let text = text.replacen('|', "", 1);
        completion_site(&text, offset)
    }

    #[test]
    fn a_member_at_statement_position_is_repaired_into_a_call() {
        let site = site("local player = {}\nplayer.na|\nlocal after = 1\n").unwrap();
        assert_eq!(site.context, CompletionContext::Member { method: false });
        assert_eq!(site.text, format!("local player = {{}}\nplayer.{PLACEHOLDER}()\nlocal after = 1\n"));
        assert_eq!(site.word, 25..27);
    }

    #[test]
    fn an_expression_member_keeps_the_rest_of_the_text() {
        let site = site("local value = player.|\n").unwrap();
        assert_eq!(site.text, format!("local value = player.{PLACEHOLDER}\n"));
    }

    #[test]
    fn open_blocks_and_brackets_are_closed_when_the_rest_does_not_parse() {
        let method = site("local function f(player)\n  if player:|").unwrap();
        assert_eq!(method.context, CompletionContext::Member { method: true });
        assert_eq!(method.text, format!("local function f(player)\n  if player:{PLACEHOLDER}() then end\nend"));
        let index = site("print(items[pla|").unwrap();
        assert_eq!(index.context, CompletionContext::Name);
        assert_eq!(index.text, format!("print(items[{PLACEHOLDER}])"));
    }

    #[test]
    fn comments_strings_numbers_and_declared_names_are_not_completed() {
        for text in ["-- player.|", "local text = 'pla|'", "local n = 1.|5", "local pla|", "local function na|", "for i|"] {
            assert!(site(text).is_none(), "{text}");
        }
    }

    fn call(text: &str) -> Option<(usize, usize, String)> {
        let offset = text.find('|').expect("a cursor");
        let text = text.replacen('|', "", 1);
        signature_site(&text, offset).map(|site| (site.open, site.argument, site.text))
    }

    #[test]
    fn the_argument_is_counted_by_the_commas_of_the_innermost_call() {
        assert_eq!(call("print(|)").map(|(open, argument, _)| (open, argument)), Some((5, 0)));
        assert_eq!(call("print(a, |b)").map(|(open, argument, _)| (open, argument)), Some((5, 1)));
        // Commas inside a table constructor or a nested call belong to them.
        assert_eq!(call("f(a, { x = 1, y = 2 }, |").map(|(open, argument, _)| (open, argument)), Some((1, 2)));
        assert_eq!(call("f(g(1, 2), |").map(|(open, argument, _)| (open, argument)), Some((1, 1)));
        assert_eq!(call("f(1, g(|").map(|(open, argument, _)| (open, argument)), Some((6, 0)));
        assert_eq!(call("object:method(1, |").map(|(open, argument, _)| (open, argument)), Some((13, 1)));
        assert_eq!(call("f({ a = |").map(|(open, argument, _)| (open, argument)), Some((1, 0)));
    }

    #[test]
    fn an_incomplete_argument_list_is_repaired_to_parse() {
        assert_eq!(call("local x = f(1, |").unwrap().2, format!("local x = f(1, {PLACEHOLDER})"));
        assert_eq!(call("f(1, |)\nlocal y = 2\n").unwrap().2, format!("f(1, {PLACEHOLDER})\nlocal y = 2\n"));
        assert_eq!(call("if f(|").unwrap().2, "if f() then end");
    }

    #[test]
    fn parameter_lists_parentheses_strings_and_comments_are_not_calls() {
        for text in ["local function f(|", "function M.f(a, |", "function M:f(|", "local g = function(|", "local x = (|", "f('a, |", "f(1) -- f(|", "f(1) --[[ g(| ]]"] {
            assert!(call(text).is_none(), "{text}");
        }
        assert!(call("f('a', |").is_some());
    }

    #[test]
    fn annotations_complete_tags_and_types_but_not_the_names_they_declare() {
        let context = |text: &str| site(text).map(|site| site.context);
        assert_eq!(context("---@par|"), Some(CompletionContext::Tag));
        assert_eq!(context("---@param point Po|"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@type |"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@return number, str|"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@class Point : Ba|"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@type\n---| str|"), Some(CompletionContext::AnnotationType));
        assert_eq!(context("---@param po|"), None);
        assert_eq!(context("---@class Po|"), None);
        assert_eq!(context("-- not documentation @param x Po|"), None);
    }

    #[test]
    fn the_outline_nests_fields_and_functions_in_their_declarations() {
        let text = "---@class Point\n---@field x number\nlocal Point = {}\n\nfunction Point:length() local function square(v) return v * v end return 0 end\n\nlocal config = { name = 'a', run = function() end }\nVERSION = 1\nlocal function helper() end\nfor i = 1, 2 do local function inner() end end\n";
        let outline = |symbols: &[DocumentSymbol]| symbols.iter().map(|symbol| {
            let children: Vec<_> = symbol.children.iter().map(|child| format!("{}:{:?}", child.name, child.kind)).collect();
            format!("{}:{:?}[{}]", symbol.name, symbol.kind, children.join(","))
        }).collect::<Vec<_>>();
        assert_eq!(outline(&document_symbols(text)), vec![
            "Point:Class[x:Field]",
            "Point:Variable[]",
            "Point:length:Method[square:Function]",
            "config:Variable[name:Field,run:Method]",
            "VERSION:Variable[]",
            "helper:Function[]",
            "inner:Function[]",
        ]);
    }

    #[test]
    fn functions_blocks_tables_comments_and_regions_fold() {
        let text = "--region setup\n--- One.\n--- Two.\nlocal function f()\n    if x then\n        y()\n    else\n        z()\n    end\nend\nlocal t = {\n    a = 1,\n}\n--endregion\nlocal flat = { 1 }\n";
        let folds: Vec<_> = folding_ranges(text).into_iter().map(|fold| (text[fold.start..fold.end].lines().next().unwrap().to_owned(), fold.kind)).collect();
        assert_eq!(folds, vec![
            ("--region setup".to_owned(), Some(FoldKind::Region)),
            ("--- One.".to_owned(), Some(FoldKind::Comment)),
            ("local function f()".to_owned(), None),
            ("if x then".to_owned(), None),
            ("else".to_owned(), None),
            ("{".to_owned(), None),
        ]);
    }

    #[test]
    fn a_selection_grows_from_a_name_through_its_expressions_and_statements() {
        let text = "local function f()\n    return point.x + 1\nend\n";
        let offset = text.find(".x").unwrap() + 1;
        let chain: Vec<_> = selection_ranges(text, &[offset])[0].iter().map(|span| &text[span.start..span.end]).collect();
        assert_eq!(chain[..4], ["x", "point.x", "point.x + 1", "return point.x + 1"]);
        assert_eq!(*chain.last().unwrap(), text);
    }

    #[test]
    fn semantic_tokens_classify_names_members_and_annotation_types() {
        let text = "---@class Point\n---@field x number\nlocal Point = {}\n\n---@generic T\n---@param value T\nfunction Point:move(value) local limit <const> = 1 print(self.x, limit) return { y = value } end\n";
        let tokens: Vec<_> = semantic_tokens(text).into_iter().map(|token| (&text[token.span.start..token.span.end], token.kind, token.modifiers)).collect();
        for expected in [
            ("Point", TokenType::Class, MODIFIER_DECLARATION),
            ("x", TokenType::Property, MODIFIER_DECLARATION),
            ("number", TokenType::Type, MODIFIER_DEFAULT_LIBRARY),
            ("T", TokenType::TypeParameter, MODIFIER_DECLARATION),
            ("value", TokenType::Parameter, 0),
            ("T", TokenType::TypeParameter, 0),
            ("move", TokenType::Method, MODIFIER_DECLARATION),
            ("limit", TokenType::Variable, MODIFIER_DECLARATION | MODIFIER_READONLY),
            ("print", TokenType::Function, MODIFIER_GLOBAL | MODIFIER_DEFAULT_LIBRARY),
            ("self", TokenType::Parameter, 0),
            ("y", TokenType::Property, MODIFIER_DECLARATION),
        ] {
            assert!(tokens.contains(&expected), "{expected:?} in {tokens:?}");
        }
    }

    #[test]
    fn concatenation_is_not_a_member_access() {
        let site = site("local text = prefix ..|").unwrap();
        assert_eq!(site.context, CompletionContext::Name);
    }
}
