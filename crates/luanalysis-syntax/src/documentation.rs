//! What documentation comments say beside their types: a declaration's description, and its
//! parameters', results' and fields' descriptions, for hover, completion and signature help.

use crate::{CommentKind, File};

/// The prose of a declaration's documentation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Documentation {
    /// The block's prose, and the description of the tag that declares it (`@type`, `@class`,
    /// `@alias`, `@overload` and the like).
    pub description: String,
    /// Each `@param`'s description, by parameter name (`...` for the variadic parameters).
    pub params: Vec<(String, String)>,
    /// Each `@return`'s description.
    pub returns: Vec<String>,
    /// Each `@field`'s description, by field name.
    pub fields: Vec<(String, String)>,
    /// What a `---@deprecated` says, when the declaration is deprecated (empty when nothing).
    pub deprecated: Option<String>,
}

impl Documentation {
    pub fn is_empty(&self) -> bool {
        self.description.is_empty() && self.params.is_empty() && self.returns.is_empty() && self.fields.is_empty() && self.deprecated.is_none()
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(param, _)| param == name).map(|(_, text)| text.as_str())
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|(field, _)| field == name).map(|(_, text)| text.as_str())
    }
}

/// The documentation of the declaration whose first line contains `offset`: the documentation
/// comments directly above that line, as v1 and LuaLS attach a block to the statement below it.
pub fn documentation_above(file: &File, source: &str, offset: usize) -> Documentation {
    let line_start = source.get(..offset).and_then(|head| head.rfind('\n')).map_or(0, |newline| newline + 1);
    let Some(last) = file.comments.iter().rposition(|comment| {
        comment.span.end <= line_start && comment.kind == CommentKind::Documentation
            && source.get(comment.span.end..line_start).is_some_and(|gap| gap.trim().is_empty() && gap.matches('\n').count() <= 1)
    }) else {
        return Documentation::default();
    };
    collect(file, source, block_start(file, source, last), last)
}

/// The documentation of the block of documentation comments that contains `offset`, such as a
/// `---@field` or `---@overload` tag's.
pub fn documentation_at(file: &File, source: &str, offset: usize) -> Documentation {
    let Some(index) = file.comments.iter().position(|comment| {
        comment.kind == CommentKind::Documentation && comment.span.start <= offset && offset < comment.span.end
    }) else {
        return Documentation::default();
    };
    let mut last = index;
    while last + 1 < file.comments.len() && contiguous(file, source, last, last + 1) { last += 1; }
    collect(file, source, block_start(file, source, index), last)
}

/// The first comment of the block that ends with comment `last`.
fn block_start(file: &File, source: &str, last: usize) -> usize {
    let mut first = last;
    while first > 0 && contiguous(file, source, first - 1, first) { first -= 1; }
    first
}

/// Whether documentation comments `before` and `after` are on consecutive lines, each on a line
/// of its own.
fn contiguous(file: &File, source: &str, before: usize, after: usize) -> bool {
    let (before, after) = (&file.comments[before], &file.comments[after]);
    before.kind == CommentKind::Documentation && after.kind == CommentKind::Documentation
        && standalone(source, before.span.start)
        && source.get(before.span.end..after.span.start).is_some_and(|gap| gap.trim().is_empty() && gap.matches('\n').count() <= 1)
}

/// Whether only indentation precedes `offset` on its line.
fn standalone(source: &str, offset: usize) -> bool {
    let line_start = source.get(..offset).and_then(|head| head.rfind('\n')).map_or(0, |newline| newline + 1);
    source.get(line_start..offset).is_some_and(|indent| indent.trim().is_empty())
}

fn collect(file: &File, source: &str, first: usize, last: usize) -> Documentation {
    let mut documentation = Documentation::default();
    let mut description: Vec<String> = Vec::new();
    for comment in &file.comments[first..=last] {
        if !standalone(source, comment.span.start) { continue; }
        let (parsed, _) = crate::types::annotations(comment, file.options);
        if parsed.deprecated.is_some() { documentation.deprecated = parsed.deprecated.clone(); }
        let prose = parsed.prose.join("\n");
        if !prose.trim().is_empty() { description.push(prose.trim().to_owned()); }
        for tag in parsed.descriptions {
            match (tag.tag.as_str(), tag.subject) {
                ("param", Some(name)) => documentation.params.push((name, tag.text)),
                ("return", _) => documentation.returns.push(tag.text),
                ("field", Some(name)) => documentation.fields.push((name, tag.text)),
                ("type" | "class" | "shape" | "alias" | "module" | "enum" | "overload", _) => description.push(tag.text),
                _ => {}
            }
        }
    }
    documentation.description = description.join("\n\n");
    documentation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn above(source: &str, before: &str) -> Documentation {
        let file = crate::parse(source);
        documentation_above(&file, source, source.find(before).expect("the declaration"))
    }

    #[test]
    fn a_function_is_described_by_its_prose_and_its_tags() {
        let source = "--- Moves an object.\n--- It glides.\n---@param object Object the object to move\n---@param distance number # how far,\n--- in metres\n---@return boolean moved\nlocal function move(object, distance) end\n";
        let documentation = above(source, "local function move");
        assert_eq!(documentation.description, "Moves an object.\nIt glides.");
        assert_eq!(documentation.param("object"), Some("the object to move"));
        assert_eq!(documentation.param("distance"), Some("how far,\nin metres"));
        assert_eq!(documentation.returns, vec!["moved".to_owned()]);
    }

    #[test]
    fn a_block_after_a_blank_line_or_code_describes_nothing() {
        assert!(above("--- Unrelated.\n\nlocal x = 1\n", "local x").is_empty());
        assert!(above("local y = 1 --- trailing\nlocal x = 1\n", "local x").is_empty());
    }

    #[test]
    fn a_class_block_describes_the_class_and_its_fields() {
        let source = "--- A point.\n---@class Point\n---@field x number the distance across\n---@field y number\n--- the distance up\nlocal Point = {}\n";
        let file = crate::parse(source);
        let documentation = documentation_at(&file, source, source.find("@field y").unwrap());
        assert_eq!(documentation.description, "A point.");
        assert_eq!(documentation.field("x"), Some("the distance across"));
        assert_eq!(documentation.field("y"), Some("the distance up"));
        let local = above(source, "local Point");
        assert_eq!(local.field("x"), Some("the distance across"));
    }

    #[test]
    fn a_local_is_described_by_its_type_tag() {
        let documentation = above("---@type number The count of things.\nlocal count = 0\n", "local count");
        assert_eq!(documentation.description, "The count of things.");
    }
}
