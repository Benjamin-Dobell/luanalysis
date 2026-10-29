//! Completion snippets are editor metadata, not references to Lua expressions.
use crate::types::{DeclarationItem, ParsedDocumentation};
use crate::{
    Diagnostic, FunctionType, GenericParameter, NamedTypeDeclarationKind, TypeExpr, TypeExprKind,
    TypeField, TypeFieldKey,
};

fn generics<'a>(parameters: &'a [GenericParameter], roots: &mut Vec<&'a TypeExpr>) {
    roots.extend(
        parameters
            .iter()
            .filter_map(|parameter| parameter.constraint.as_ref()),
    );
}
fn fields<'a>(fields: &'a [TypeField], roots: &mut Vec<&'a TypeExpr>) {
    for field in fields {
        if let TypeFieldKey::Indexed(key) = &field.key {
            roots.push(key);
        }
        roots.push(&field.ty);
    }
}
fn function<'a>(signature: &'a FunctionType, roots: &mut Vec<&'a TypeExpr>) {
    generics(&signature.generics, roots);
    roots.extend(
        signature
            .params
            .iter()
            .filter_map(|parameter| parameter.ty.as_ref()),
    );
    if let Some(vararg) = &signature.vararg {
        roots.push(&vararg.ty);
    }
    roots.extend(&signature.returns);
}

pub(crate) fn validate(documentation: &ParsedDocumentation, diagnostics: &mut Vec<Diagnostic>) {
    let mut roots = Vec::new();
    let annotations = &documentation.functions;
    generics(&annotations.generics, &mut roots);
    // A parameter's lone `\`T\`` may capture a type by name, which only the statement's complete
    // tags tell (`types::type_captures`).
    roots.extend(annotations.params.iter().map(|parameter| &parameter.ty)
        .filter(|ty| !matches!(&ty.kind, TypeExprKind::CompletionHint(text) if crate::is_capture_name(text))));
    if let Some(vararg) = &annotations.vararg {
        roots.push(&vararg.ty);
    }
    for annotation in annotations
        .returns
        .iter()
        .chain(annotations.type_annotation.iter())
        .chain(&documentation.exclusions)
    {
        roots.extend(&annotation.types);
    }
    for item in &documentation.declarations {
        match item {
            DeclarationItem::Reference(_) => {}
            DeclarationItem::Named {
                generics: parameters,
                kind,
                ..
            } => {
                generics(parameters, &mut roots);
                match kind {
                    NamedTypeDeclarationKind::Alias { target } => roots.push(target),
                    NamedTypeDeclarationKind::Record {
                        bases,
                        fields: members,
                        ..
                    } => {
                        roots.extend(bases);
                        for member in members {
                            if let TypeFieldKey::Indexed(key) = &member.key {
                                roots.push(key);
                            }
                            roots.push(&member.ty);
                        }
                    }
                }
            }
            DeclarationItem::Field(field) => {
                if let TypeFieldKey::Indexed(key) = &field.key {
                    roots.push(key);
                }
                roots.push(&field.ty);
            }
            DeclarationItem::Overload { signature, .. } | DeclarationItem::Operator { signature, .. } => function(signature, &mut roots),
        }
    }
    let mut pending: Vec<_> = roots.into_iter().map(|ty| (ty, false)).collect();
    while let Some((ty, companion)) = pending.pop() {
        match &ty.kind {
            TypeExprKind::CompletionHint(_) if !companion => diagnostics.push(Diagnostic {
                origin: crate::DiagnosticOrigin::Annotation,
                span: ty.span,
                message: "completion hint requires an accompanying value type in its union".into(),
            }),
            TypeExprKind::Union(parts) => {
                let companion = companion || parts.iter().any(|part| !part.is_completion_only());
                pending.extend(parts.iter().map(|part| (part, companion)));
            }
            TypeExprKind::Parenthesized(inner) => pending.push((inner, companion)),
            _ => {
                let mut children = Vec::new();
                match &ty.kind {
                    TypeExprKind::Record(members) => fields(members, &mut children),
                    TypeExprKind::Function(signature) => function(signature, &mut children),
                    TypeExprKind::Array { element, .. } | TypeExprKind::TypePredicate { ty: Some(element), .. } | TypeExprKind::KeyOf(element) => children.push(element.as_ref()),
                    TypeExprKind::IndexedAccess { object, index } => children.extend([object.as_ref(), index.as_ref()]),
                    TypeExprKind::Generic { args, .. } | TypeExprKind::Intersection(args) => children.extend(args),
                    _ => {}
                }
                pending.extend(children.into_iter().map(|child| (child, false)));
            }
        }
    }
}
