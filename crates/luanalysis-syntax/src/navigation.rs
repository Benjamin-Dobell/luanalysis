//! The type names that documentation comments declare and refer to, for go to definition, find
//! references and rename of classes, shapes, aliases and generic parameters.

use crate::types::{DeclarationItem, ParsedDocumentation};
use crate::{CommentKind, File, FunctionAnnotations, Name, FunctionType, GenericParameter, Span, TypeAnnotation, TypeExpr, TypeExprKind, TypeFieldKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeNameKind {
    /// A use of a type's name: `---@param point Point`.
    Reference,
    /// The name a `---@class`, `---@shape`, `---@alias` or `---@enum` declares.
    Declaration,
    /// A `---@generic` parameter (or a `fun<T>` one), which only its own documentation block sees.
    Generic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeName {
    pub name: String,
    pub span: Span,
    pub kind: TypeNameKind,
    /// The documentation block it is written in: the contiguous documentation comments around it.
    pub block: Span,
}

/// Every type name the documentation comments of `file` declare or refer to, in source order.
pub fn type_names(file: &File, source: &str) -> Vec<TypeName> {
    let mut names = Vec::new();
    let mut block: Option<Span> = None;
    for comment in &file.comments {
        if comment.kind != CommentKind::Documentation { continue; }
        // Consecutive documentation lines form one block.
        block = Some(match block {
            Some(span) if source.get(span.end..comment.span.start).is_some_and(|gap| gap.trim().is_empty() && gap.matches('\n').count() <= 1) => span.join(comment.span),
            _ => comment.span,
        });
        let (parsed, _) = crate::types::annotations(comment);
        let mut found = Vec::new();
        visit_documentation(&parsed, &mut |name, span, kind| if let Visited::Type(kind) = kind { found.push((name.to_owned(), span, kind)) });
        names.extend(found.into_iter().map(|(name, span, kind)| TypeName { name, span, kind, block: comment.span }));
    }
    // Each name's block is the whole run of comments it belongs to.
    let mut blocks: Vec<Span> = Vec::new();
    let mut current: Option<Span> = None;
    for comment in file.comments.iter().filter(|comment| comment.kind == CommentKind::Documentation) {
        current = Some(match current {
            Some(span) if source.get(span.end..comment.span.start).is_some_and(|gap| gap.trim().is_empty() && gap.matches('\n').count() <= 1) => span.join(comment.span),
            Some(span) => { blocks.push(span); comment.span }
            None => comment.span,
        });
    }
    blocks.extend(current);
    for name in &mut names {
        if let Some(block) = blocks.iter().find(|block| block.start <= name.span.start && name.span.end <= block.end) {
            name.block = *block;
        }
    }
    names
}

/// What the documentation visitor finds: a type's name, or the variable a `typeof` names.
#[derive(Clone, Copy)]
enum Visited {
    Type(TypeNameKind),
    Value,
}

/// The variables `typeof` names in the documentation comments of `file`, in source order: the
/// first name of each (`Color` in `typeof Color.Red`). A reference there reads that variable.
pub fn type_query_roots(file: &File) -> Vec<Name> {
    let mut roots = Vec::new();
    for comment in file.comments.iter().filter(|comment| comment.kind == CommentKind::Documentation && comment.text.contains("typeof")) {
        let (parsed, _) = crate::types::annotations(comment);
        visit_documentation(&parsed, &mut |name, span, kind| if let Visited::Value = kind { roots.push(Name { text: name.to_owned(), span }) });
    }
    roots
}

fn visit_documentation(parsed: &ParsedDocumentation, visit: &mut dyn FnMut(&str, Span, Visited)) {
    visit_annotations(&parsed.functions, visit);
    for item in &parsed.declarations {
        match item {
            DeclarationItem::Reference(reference) => visit(&reference.target.text, reference.target.span, Visited::Type(TypeNameKind::Reference)),
            DeclarationItem::Named { name, generics, kind, .. } => {
                visit(&name.text, name.span, Visited::Type(TypeNameKind::Declaration));
                visit_generics(generics, visit);
                match kind {
                    crate::NamedTypeDeclarationKind::Alias { target } => visit_type(target, visit),
                    crate::NamedTypeDeclarationKind::Record { base, fields, .. } => {
                        if let Some(base) = base { visit_type(base, visit); }
                        for field in fields { visit_field(&field.key, &field.ty, visit); }
                    }
                }
            }
            DeclarationItem::Field(field) => {
                if let Some(owner) = &field.explicit_owner { visit(&owner.text, owner.span, Visited::Type(TypeNameKind::Reference)); }
                visit_field(&field.key, &field.ty, visit);
            }
            DeclarationItem::Function { explicit_owner, signature, annotations, .. } => {
                if let Some(owner) = explicit_owner { visit(&owner.text, owner.span, Visited::Type(TypeNameKind::Reference)); }
                visit_function(signature, visit);
                visit_annotations(annotations, visit);
            }
            DeclarationItem::Overload { signature, .. } | DeclarationItem::Operator { signature, .. } => visit_function(signature, visit),
        }
    }
    for annotation in parsed.exclusions.iter().chain(&parsed.as_casts) { visit_annotation(annotation, visit); }
    for cast in &parsed.casts { visit_annotation(&cast.annotation, visit); }
}

fn visit_annotations(annotations: &FunctionAnnotations, visit: &mut dyn FnMut(&str, Span, Visited)) {
    visit_generics(&annotations.generics, visit);
    for param in &annotations.params { visit_type(&param.ty, visit); }
    if let Some(vararg) = &annotations.vararg { visit_type(&vararg.ty, visit); }
    for annotation in annotations.returns.iter().chain(&annotations.type_annotation) { visit_annotation(annotation, visit); }
}

fn visit_annotation(annotation: &TypeAnnotation, visit: &mut dyn FnMut(&str, Span, Visited)) {
    for ty in &annotation.types { visit_type(ty, visit); }
}

fn visit_generics(generics: &[GenericParameter], visit: &mut dyn FnMut(&str, Span, Visited)) {
    for generic in generics {
        visit(&generic.name.text, generic.name.span, Visited::Type(TypeNameKind::Generic));
        if let Some(constraint) = &generic.constraint { visit_type(constraint, visit); }
    }
}

fn visit_field(key: &TypeFieldKey, ty: &TypeExpr, visit: &mut dyn FnMut(&str, Span, Visited)) {
    if let TypeFieldKey::Indexed(key) = key { visit_type(key, visit); }
    visit_type(ty, visit);
}

fn visit_function(function: &FunctionType, visit: &mut dyn FnMut(&str, Span, Visited)) {
    visit_generics(&function.generics, visit);
    for param in &function.params {
        if let Some(ty) = &param.ty { visit_type(ty, visit); }
    }
    if let Some(vararg) = &function.vararg { visit_type(&vararg.ty, visit); }
    for ty in &function.returns { visit_type(ty, visit); }
}

fn visit_type(ty: &TypeExpr, visit: &mut dyn FnMut(&str, Span, Visited)) {
    match &ty.kind {
        TypeExprKind::Named(name) => visit(name, ty.span, Visited::Type(TypeNameKind::Reference)),
        TypeExprKind::Generic { base, args, .. } => {
            visit(&base.text, base.span, Visited::Type(TypeNameKind::Reference));
            for arg in args { visit_type(arg, visit); }
        }
        TypeExprKind::Union(types) | TypeExprKind::Intersection(types) => for ty in types { visit_type(ty, visit); },
        TypeExprKind::Array { element, .. } | TypeExprKind::Parenthesized(element) | TypeExprKind::TypePredicate { ty: Some(element), .. }
        | TypeExprKind::KeyOf(element) => visit_type(element, visit),
        TypeExprKind::IndexedAccess { object, index } => {
            visit_type(object, visit);
            visit_type(index, visit);
        }
        TypeExprKind::Record(fields) => for field in fields { visit_field(&field.key, &field.ty, visit); },
        TypeExprKind::Function(function) => visit_function(function, visit),
        // A type query names a value, not a type.
        TypeExprKind::TypeQuery(path) => visit(&path[0].text, path[0].span, Visited::Value),
        TypeExprKind::Literal(_) | TypeExprKind::CompletionHint(_) | TypeExprKind::TypePredicate { ty: None, .. }
        | TypeExprKind::Error => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_are_declared_referred_to_and_scoped_to_their_block() {
        let source = "---@class Point : Base\n---@field next Point | nil\nlocal Point = {}\n\n---@generic T\n---@param list T[]\n---@return table<string, Point>\nlocal function first(list) end\n";
        let file = crate::parse(source);
        let names = type_names(&file, source);
        let found: Vec<_> = names.iter().map(|name| (name.name.as_str(), &source[name.span.start..name.span.end], name.kind)).collect();
        assert_eq!(found, vec![
            ("Point", "Point", TypeNameKind::Declaration),
            ("Base", "Base", TypeNameKind::Reference),
            ("Point", "Point", TypeNameKind::Reference),
            ("nil", "nil", TypeNameKind::Reference),
            ("T", "T", TypeNameKind::Generic),
            ("T", "T", TypeNameKind::Reference),
            ("table", "table", TypeNameKind::Reference),
            ("string", "string", TypeNameKind::Reference),
            ("Point", "Point", TypeNameKind::Reference),
        ]);
        // The generic and its use share a block; the class's block is another.
        assert_eq!(names[4].block, names[5].block);
        assert_ne!(names[0].block, names[4].block);
    }
}
