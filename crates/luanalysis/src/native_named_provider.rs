//! Immutable source recipes for native lazy named queries. Type resolution,
//! recursive publication, substitution and compatibility stay in the core.
use super::NativeRegistry;
use crate::native_named::*;
use crate::native_types::TypeNameResolution;
use crate::{Checker, Diagnostic};
use luanalysis_engine::{
    LuaKey, NamedTypeError, NamedTypeQuery, NativeTypeDeclarationKey, NativeTypeMemberKey,
    NativeTypeParameterKey, ObjectDeclarationForm, Type, TypeDeclarationHeader,
    TypeDeclarationKind, TypeDeclarationOwner, TypeMemberHeader, TypeMemberKeyRecipe,
    TypeMemberOwner, TypeMemberVisibility, TypeQueries,
};
use luanalysis_syntax::{
    FieldVisibility, NamedRecordKind, NamedTypeDeclarationKind, Span, TypeExpr, TypeExprKind,
    TypeFieldKey,
};
use std::collections::HashSet;

fn declaration(key: NativeNamedKey) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn anonymous(key: NativeAnonymousHeadKey) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn member(key: NativeNamedMemberKey) -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn anonymous_member(key: NativeAnonymousMemberKey) -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn named_key(key: NativeTypeDeclarationKey) -> NativeNamedKey {
    NativeNamedKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn anonymous_key(key: NativeTypeDeclarationKey) -> NativeAnonymousHeadKey {
    NativeAnonymousHeadKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn member_key(key: NativeTypeMemberKey) -> NativeNamedMemberKey {
    NativeNamedMemberKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn anonymous_member_key(key: NativeTypeMemberKey) -> NativeAnonymousMemberKey {
    NativeAnonymousMemberKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}

fn lexical_context(
    context: Option<NativeTypeContext>,
    documentation: &[NativeNamedKey],
) -> Option<NativeTypeContext> {
    context.or_else(|| match documentation {
        [key] => Some(NativeTypeContext::NamedType(*key)),
        _ => None,
    })
}
fn head_context(head: &AnonymousHeadSource) -> Option<NativeTypeContext> {
    // A copied function @type retains both uses. The function's own formal
    // scope is the declaration scope of that signature annotation.
    let function = head.uses.iter().find_map(|usage| match usage.origin {
        AnonymousHeadOrigin::Function(key) => Some(NativeTypeContext::Function(key)),
        _ => None,
    });
    lexical_context(
        function.or(head.lexical_context),
        &head.documentation_contexts,
    )
}
fn key_recipe(key: &TypeFieldKey) -> TypeMemberKeyRecipe {
    match key {
        TypeFieldKey::Named(name) => {
            TypeMemberKeyRecipe::Fixed(LuaKey::String(name.text.as_bytes().to_vec()))
        }
        TypeFieldKey::Sequence { index } => TypeMemberKeyRecipe::Fixed(LuaKey::Integer(
            (*index).try_into().expect("source sequence index fits i64"),
        )),
        TypeFieldKey::Indexed(ty) => crate::native_tables::literal_type_key(ty).map_or(
            TypeMemberKeyRecipe::Indexer { sparse: false },
            TypeMemberKeyRecipe::Fixed,
        ),
    }
}

impl NativeRegistry {
    fn signature_aware_head_context(&self,head:&AnonymousHeadSource)->Option<NativeTypeContext> {
        self.signature_context_for_annotation(head.syntax.span)
            .map(NativeTypeContext::Signature).or_else(||head_context(head))
    }
    /// Follow finite source annotation edges solely to collect parameter
    /// occurrences. This does not produce types, expand aliases, or diagnose
    /// semantic cycles; those demands remain in the native checker.
    pub(crate) fn annotation_parameter_dependencies(
        &self,
        context: Option<NativeTypeContext>,
        ty: &TypeExpr,
        used: &mut HashSet<NativeTypeParameterKey>,
    ) {
        let mut pending = vec![(context, ty)];
        let mut seen = HashSet::new();
        while let Some((context, ty)) = pending.pop() {
            let mut reference = None;
            match &ty.kind {
                TypeExprKind::Named(name) => reference = Some(name.as_str()),
                TypeExprKind::Generic { base, args, .. } => {
                    reference = Some(base.text.as_str());
                    pending.extend(args.iter().map(|ty| (context, ty)));
                }
                TypeExprKind::Union(types) => pending.extend(types.iter().map(|ty| (context, ty))),
                TypeExprKind::Array { element, .. } | TypeExprKind::Parenthesized(element) => {
                    pending.push((context, element))
                }
                TypeExprKind::Record(fields) => {
                    for field in fields {
                        pending.push((context, &field.ty));
                        if let TypeFieldKey::Indexed(key) = &field.key {
                            pending.push((context, key));
                        }
                    }
                }
                TypeExprKind::Function(function) => {
                    let context=self.signature_key_for(function.span)
                        .map(NativeTypeContext::Signature).or(context);
                    pending.extend(function.generics.iter().filter_map(|parameter| parameter.constraint.as_ref()).map(|ty|(context,ty)));
                    pending.extend(
                        function
                            .params
                            .iter()
                            .filter_map(|parameter| parameter.ty.as_ref())
                            .map(|ty| (context, ty)),
                    );
                    pending.extend(function.returns.iter().map(|ty| (context, ty)));
                    if let Some(vararg) = &function.vararg {
                        pending.push((context, &vararg.ty));
                    }
                }
                TypeExprKind::Literal(_) | TypeExprKind::CompletionHint(_) | TypeExprKind::Error => {}
            }
            let Some(name) = reference else { continue };
            // Match annotation name-binding precedence for primitive keywords.
            if matches!(
                name,
                "any" | "unknown" | "never" | "nil" | "boolean" | "number" | "string" | "void" | "table" | "function"
            ) {
                continue;
            }
            match self.types.resolve_name_in_context(context, name) {
                TypeNameResolution::Parameter(key) => {
                    used.insert(key);
                    continue;
                }
                TypeNameResolution::Ambiguous(keys) => {
                    used.extend(keys);
                    continue;
                }
                TypeNameResolution::Missing => {}
            }
            for &key in self.named.candidates(name) {
                if !seen.insert(key) {
                    continue;
                }
                let source = self
                    .named
                    .declaration(key)
                    .expect("original annotation declaration");
                let context = Some(NativeTypeContext::NamedType(key));
                for parameter in &source.syntax.generics {
                    if let Some(constraint) = &parameter.constraint {
                        pending.push((context, constraint));
                    }
                }
                match &source.syntax.kind {
                    NamedTypeDeclarationKind::Alias { target } => pending.push((context, target)),
                    NamedTypeDeclarationKind::Record { base, fields, .. } => {
                        if let Some(base) = base {
                            pending.push((context, base));
                        }
                        for field in fields {
                            pending.push((context, &field.ty));
                            if let TypeFieldKey::Indexed(key) = &field.key {
                                pending.push((context, key));
                            }
                        }
                    }
                }
                // Raw augmentation candidates are source binding facts.
                for extension in self
                    .named
                    .augmentations()
                    .iter()
                    .filter(|extension| extension.candidates.contains(&key))
                {
                    let context = lexical_context(
                        extension.lexical_context,
                        &extension.documentation_contexts,
                    );
                    for field in &extension.syntax.fields {
                        pending.push((context, &field.ty));
                        if let TypeFieldKey::Indexed(key) = &field.key {
                            pending.push((context, key));
                        }
                    }
                }
            }
        }
    }

    pub(super) fn function_annotation_parameter_dependencies(
        &self,
        context: Option<luanalysis_engine::NativeFunctionKey>,
        ty: &TypeExpr,
        used: &mut HashSet<NativeTypeParameterKey>,
    ) {
        self.annotation_parameter_dependencies(context.map(NativeTypeContext::Function), ty, used)
    }
    pub(crate) fn annotation_parameter_resolution(
        &self,
        context: Option<NativeTypeContext>,
        name: &str,
    ) -> TypeNameResolution {
        self.types.resolve_name_in_context(context, name)
    }
    pub(super) fn named_contains_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        self.named.declaration(named_key(key)).is_some()
            || self.named.anonymous_head(anonymous_key(key)).is_some()
    }
    pub(super) fn named_contains_member(&self, key: NativeTypeMemberKey) -> bool {
        self.named.member(member_key(key)).is_some()
            || self
                .named
                .anonymous_member(anonymous_member_key(key))
                .is_some()
    }
    fn named_outer_parameters(
        &self,
        context: Option<NativeTypeContext>,
        ty: &TypeExpr,
    ) -> Vec<NativeTypeParameterKey> {
        let Some(context) = context else {
            return Vec::new();
        };
        let mut used = HashSet::new();
        self.annotation_parameter_dependencies(Some(context), ty, &mut used);
        self.types
            .outer_parameters_in_context(context, &used)
            .into_iter()
            .chain(self.types.own_parameters_in_context(context))
            .filter(|key| used.contains(key))
            .collect()
    }
    pub(super) fn named_declaration_header(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> TypeDeclarationHeader {
        if let Some(source) = self.named.declaration(named_key(key)) {
            let context = NativeTypeContext::NamedType(source.key);
            let mut used = HashSet::new();
            for parameter in &source.syntax.generics {
                if let Some(constraint) = &parameter.constraint {
                    self.annotation_parameter_dependencies(Some(context), constraint, &mut used);
                }
            }
            let (kind, object_form) = match &source.syntax.kind {
                NamedTypeDeclarationKind::Alias { target } => {
                    self.annotation_parameter_dependencies(Some(context), target, &mut used);
                    (TypeDeclarationKind::Alias, None)
                }
                NamedTypeDeclarationKind::Record {
                    flavor,
                    base,
                    fields,
                } => {
                    if let Some(base) = base {
                        self.annotation_parameter_dependencies(Some(context), base, &mut used);
                    }
                    for field in fields {
                        self.annotation_parameter_dependencies(Some(context), &field.ty, &mut used);
                        if let TypeFieldKey::Indexed(key) = &field.key {
                            self.annotation_parameter_dependencies(Some(context), key, &mut used);
                        }
                    }
                    (
                        match flavor {
                            NamedRecordKind::Shape => TypeDeclarationKind::Shape,
                            NamedRecordKind::Class => TypeDeclarationKind::Class,
                            NamedRecordKind::Module => TypeDeclarationKind::Module,
                        },
                        Some(ObjectDeclarationForm::Record),
                    )
                }
            };
            return TypeDeclarationHeader {
                kind,
                name: source.syntax.name.text.clone(),
                object_form,
                parameters: self.types.own_parameters_in_context(context),
                outer_parameters: self.types.outer_parameters_in_context(context, &used),
            };
        }
        let source = self
            .named
            .anonymous_head(anonymous_key(key))
            .expect("original anonymous head");
        let name = source
            .origins
            .iter()
            .find_map(|origin| match origin {
                AnonymousHeadOrigin::Named(key) => self
                    .named
                    .declaration(*key)
                    .map(|named| named.syntax.name.text.clone()),
                _ => None,
            })
            .unwrap_or_else(|| match source.kind {
                AnonymousHeadKind::Record => "record".into(),
                AnonymousHeadKind::Array => "array".into(),
                AnonymousHeadKind::Table => "table".into(),
            });
        let object_form = match &source.syntax.kind {
            TypeExprKind::Record(_) => ObjectDeclarationForm::Record,
            TypeExprKind::Array { .. } => ObjectDeclarationForm::Array,
            TypeExprKind::Generic { args, .. } => ObjectDeclarationForm::Table {
                type_argument_count: args.len(),
            },
            _ => unreachable!("registered object head syntax"),
        };
        TypeDeclarationHeader {
            kind: TypeDeclarationKind::Shape,
            name,
            object_form: Some(object_form),
            parameters: Vec::new(),
            outer_parameters: self.named_outer_parameters(self.signature_aware_head_context(source), &source.syntax),
        }
    }
    pub(super) fn named_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        if let Some(source) = self.named.member(member_key(key)) {
            return TypeMemberHeader {
                owner: match &source.owner {
                    NamedMemberOwner::Declared(key) => TypeMemberOwner::Declared(declaration(*key)),
                    NamedMemberOwner::Reference {
                        name, candidates, ..
                    } => TypeMemberOwner::Reference {
                        name: name.text.clone(),
                        candidates: candidates.iter().copied().map(declaration).collect(),
                    },
                },
                key: key_recipe(&source.field.key),
                visibility: source.field.visibility.map_or(
                    TypeMemberVisibility::Public,
                    |visibility| match visibility.kind {
                        FieldVisibility::Public => TypeMemberVisibility::Public,
                        FieldVisibility::Protected => TypeMemberVisibility::Protected,
                        FieldVisibility::Private => TypeMemberVisibility::Private,
                    },
                ),
                optional: false,
                readonly: false,
            };
        }
        let source = self
            .named
            .anonymous_member(anonymous_member_key(key))
            .expect("original anonymous member");
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(anonymous(source.owner)),
            key: match &source.key_recipe {
                AnonymousMemberKeyRecipe::Record(key) => key_recipe(key),
                AnonymousMemberKeyRecipe::ArrayNumber | AnonymousMemberKeyRecipe::Table(_) => {
                    TypeMemberKeyRecipe::Indexer { sparse: false }
                }
            },
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        }
    }
    pub(super) fn named_declaration_members(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> Vec<NativeTypeMemberKey> {
        let mut result = if let Some(source) = self.named.declaration(named_key(key)) {
            source
                .members
                .iter()
                .copied()
                .map(member)
                .collect::<Vec<_>>()
        } else {
            self.named
                .anonymous_head(anonymous_key(key))
                .expect("original anonymous head")
                .members
                .iter()
                .copied()
                .map(anonymous_member)
                .collect()
        };
        // Canonical alias ownership and applicability belong to the core.
        result.extend(
            self.named
                .augmentations()
                .iter()
                .flat_map(|source| source.members.iter().copied())
                .map(member),
        );
        result
    }
    fn named_annotation(
        &self,
        context: Option<NativeTypeContext>,
        documentation: &[NativeNamedKey],
        ty: &TypeExpr,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let mut checker = self.checker(queries);
        match lexical_context(context, documentation) {
            Some(NativeTypeContext::Function(key)) => checker.type_context = Some(key),
            Some(NativeTypeContext::NamedType(key)) => checker.named_type_context = Some(key),
            Some(NativeTypeContext::Signature(key)) => checker.signature_type_context = Some(key),
            None => {}
        }
        let result = checker.annotation(ty);
        self.merge_report(checker.report);
        result
    }
    pub(super) fn named_alias_target(
        &self,
        key: NativeTypeDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let source = self
            .named
            .declaration(named_key(key))
            .expect("original alias declaration");
        let NamedTypeDeclarationKind::Alias { target } = &source.syntax.kind else {
            unreachable!("alias target demand")
        };
        self.named_annotation(
            Some(NativeTypeContext::NamedType(source.key)),
            &source.documentation_contexts,
            target,
            queries,
        )
    }
    pub(super) fn named_member_type(
        &self,
        key: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        if let Some(source) = self.named.member(member_key(key)) {
            return self.named_annotation(
                source.lexical_context,
                &source.documentation_contexts,
                &source.field.ty,
                queries,
            );
        }
        let source = self
            .named
            .anonymous_member(anonymous_member_key(key))
            .expect("original anonymous member");
        let head = self
            .named
            .anonymous_head(source.owner)
            .expect("original anonymous head");
        self.named_annotation(
            self.signature_aware_head_context(head),
            &head.documentation_contexts,
            &source.ty,
            queries,
        )
    }
    pub(super) fn named_indexer_type(
        &self,
        key: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        if let Some(source) = self.named.member(member_key(key)) {
            let TypeFieldKey::Indexed(ty) = &source.field.key else {
                unreachable!("original declared indexer")
            };
            return self.named_annotation(
                source.lexical_context,
                &source.documentation_contexts,
                ty,
                queries,
            );
        }
        let source = self
            .named
            .anonymous_member(anonymous_member_key(key))
            .expect("original anonymous member");
        let head = self
            .named
            .anonymous_head(source.owner)
            .expect("original anonymous head");
        let ty = match &source.key_recipe {
            AnonymousMemberKeyRecipe::ArrayNumber => return Type::Number,
            AnonymousMemberKeyRecipe::Table(ty) => ty,
            AnonymousMemberKeyRecipe::Record(TypeFieldKey::Indexed(ty)) => ty,
            _ => unreachable!("original anonymous indexer"),
        };
        self.named_annotation(
            self.signature_aware_head_context(head),
            &head.documentation_contexts,
            ty,
            queries,
        )
    }
    pub(super) fn named_bases(
        &self,
        key: NativeTypeDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        let Some(source) = self.named.declaration(named_key(key)) else {
            return Vec::new();
        };
        let NamedTypeDeclarationKind::Record {
            base: Some(base), ..
        } = &source.syntax.kind
        else {
            return Vec::new();
        };
        vec![self.named_annotation(
            Some(NativeTypeContext::NamedType(source.key)),
            &source.documentation_contexts,
            base,
            queries,
        )]
    }
    pub(super) fn named_parameter_owner(
        &self,
        key: NativeTypeParameterKey,
    ) -> Option<TypeDeclarationOwner> {
        self.types
            .contains(key)
            .then(|| match self.types.parameter(key).owner {
                NativeTypeContext::Function(key) => TypeDeclarationOwner::Function(key),
                NativeTypeContext::Signature(key) => TypeDeclarationOwner::Signature(key),
                NativeTypeContext::NamedType(key) => {
                    TypeDeclarationOwner::NamedType(declaration(key))
                }
            })
    }
    pub(super) fn named_key_for_annotation(
        &self,
        ty: &TypeExpr,
    ) -> Option<NativeTypeDeclarationKey> {
        self.named.anonymous_key_at(ty.span).map(anonymous)
    }
    pub(super) fn named_candidates(&self, name: &str) -> Vec<NativeTypeDeclarationKey> {
        if let Some(project) = &self.project_names {
            return project.candidates(name).to_vec();
        }
        self.named
            .candidates(name)
            .iter()
            .copied()
            .map(declaration)
            .collect()
    }
    fn named_origin(&self, key: NativeTypeDeclarationKey) -> Span {
        self.named
            .declaration(named_key(key))
            .map(|source| source.syntax.name.span)
            .or_else(|| {
                self.named
                    .anonymous_head(anonymous_key(key))
                    .map(|source| source.syntax.span)
            })
            .unwrap_or(self.file.span)
    }
    fn named_member_origin(&self, key: NativeTypeMemberKey) -> Span {
        self.named
            .member(member_key(key))
            .map(|source| {
                source
                    .field
                    .explicit_owner
                    .as_ref()
                    .map_or(source.field.span, |owner| owner.span)
            })
            .or_else(|| {
                self.named
                    .anonymous_member(anonymous_member_key(key))
                    .map(|source| source.span)
            })
            .unwrap_or(self.file.span)
    }
    pub(super) fn named_declaration_error(&self, error: NamedTypeError) {
        let (code, span, message) = match error {
            // Actual application spans are supplied by the annotation caller.
            NamedTypeError::TypeArgumentArity { .. }
            | NamedTypeError::TypeArgumentConstraint { .. } => return,
            NamedTypeError::CircularAlias(key) => (
                "TS2456",
                self.named_origin(key),
                "Type alias circularly references itself.".into(),
            ),
            NamedTypeError::UnsupportedKind(key, kind) => (
                "LUA2031",
                self.named_origin(key),
                format!("{kind:?} checking requires native nominal/module semantics."),
            ),
            NamedTypeError::UnsupportedBases(key) => (
                "LUA2031",
                self.named_origin(key),
                "Named base checking is not implemented in this slice.".into(),
            ),
            NamedTypeError::UnsupportedVisibility(key) => (
                "LUA2031",
                self.named_member_origin(key),
                "Private/protected member semantics are not implemented in this slice.".into(),
            ),
            NamedTypeError::UnsupportedOwnerInstantiation(key) => (
                "LUA2031",
                self.named_member_origin(key),
                "Augmentation of a specialized alias requires native owner-mapper transport."
                    .into(),
            ),
            NamedTypeError::UnknownOwner { member, name } => (
                "TS2304",
                self.named_member_origin(member),
                format!("Cannot find an object owner for '{name}'."),
            ),
            NamedTypeError::AmbiguousOwner { member, name } => (
                "TS2300",
                self.named_member_origin(member),
                format!("Duplicate type owner '{name}'."),
            ),
            NamedTypeError::Table { declaration, error } => (
                "LUA2032",
                self.named_origin(declaration),
                format!("Invalid declared table: {error:?}."),
            ),
            NamedTypeError::TooDeep(query) => (
                "TS2589",
                match query {
                    NamedTypeQuery::Head(key)
                    | NamedTypeQuery::Bases(key)
                    | NamedTypeQuery::ObjectMembers(key) => self.named_origin(key),
                    NamedTypeQuery::Member(key) | NamedTypeQuery::Indexer(key) => {
                        self.named_member_origin(key)
                    }
                },
                "Type instantiation is excessively deep.".into(),
            ),
            other => (
                "LUA2032",
                self.file.span,
                format!("Invalid named declaration metadata: {other:?}."),
            ),
        };
        self.report.borrow_mut().diagnostics.push(Diagnostic {
            code,
            span,
            message,
        });
    }
    pub(crate) fn check_named_declarations(&self, queries: &mut dyn TypeQueries) {
        let mut seen = HashSet::new();
        for source in self.named.declarations() {
            if matches!(source.syntax.kind, NamedTypeDeclarationKind::Record {
                flavor: NamedRecordKind::Class, ..
            }) && source.syntax.attachment.is_some() {
                self.report.borrow_mut().diagnostics.push(Diagnostic {
                    code: "LUA2031", span: source.syntax.name.span,
                    message: format!("Runtime class declaration value '{}' requires explicit declaration-value ownership checking; standalone class annotations are supported in this slice.", source.syntax.name.text),
                });
            }
            if let Err(reason) = self.shape_attachment_directive(source) {
                self.report.borrow_mut().diagnostics.push(Diagnostic {
                    code:"LUA2031",span:source.syntax.name.span,
                    message:format!("Shape '{}' attachment is unsupported: {reason}.",source.syntax.name.text),
                });
            }
            if !seen.insert(&source.syntax.name.text) {
                self.report.borrow_mut().diagnostics.push(Diagnostic {
                    code: "TS2300",
                    span: source.syntax.name.span,
                    message: format!("Duplicate identifier '{}'.", source.syntax.name.text),
                });
            }
            if let Err(error) = queries.validate_named_declaration(declaration(source.key)) {
                self.record_named_application_error(
                    error,
                    self.named_origin(declaration(source.key)),
                    queries,
                );
            }
        }
        // These are finite source annotation nodes, not recursive type expansion.
        for source in self.named.anonymous_heads() {
            if let Err(error) = queries.validate_named_declaration(anonymous(source.key)) {
                self.record_named_application_error(error, source.syntax.span, queries);
            }
        }
        for source in self.named.augmentations() {
            for &key in &source.members {
                let _ = queries.validate_named_member(member(key));
            }
        }
    }
    fn record_named_application_error(
        &self,
        error: NamedTypeError,
        span: Span,
        queries: &mut dyn TypeQueries,
    ) {
        let diagnostic = match error {
            NamedTypeError::TypeArgumentArity {
                actual,
                minimum,
                maximum,
                ..
            } => Some(Diagnostic {
                code: "TS2314",
                span,
                message: format!("Expected {minimum}..{maximum} type arguments, got {actual}."),
            }),
            NamedTypeError::TypeArgumentConstraint {
                actual, expected, ..
            } => Some(Diagnostic {
                code: "TS2344",
                span,
                message: format!(
                    "Type '{}' does not satisfy constraint '{}'.",
                    queries.format(&actual),
                    queries.format(&expected)
                ),
            }),
            _ => None,
        };
        if let Some(diagnostic) = diagnostic {
            self.report.borrow_mut().diagnostics.push(diagnostic);
        }
    }
}

impl Checker<'_> {
    pub(crate) fn native_named_reference(
        &mut self,
        name: &str,
        arguments: &[Type],
        syntax: &TypeExpr,
    ) -> Option<Type> {
        let span = syntax.span;
        let native = self.native?;
        let candidates = native.named_candidates(name);
        let declaration = match candidates.as_slice() {
            [] => return None,
            [key] => *key,
            _ => {
                self.error("TS2300", span, format!("Duplicate named type '{name}'."));
                return Some(Type::Any);
            }
        };
        match self.engine.named_type_reference(declaration, arguments) {
            Ok(ty) => Some(ty),
            Err(error) => {
                let span = match (&error, &syntax.kind) {
                    (
                        NamedTypeError::TypeArgumentConstraint { index, .. },
                        TypeExprKind::Generic { args, .. },
                    ) => args.get(*index).map_or(span, |argument| argument.span),
                    _ => span,
                };
                native.record_named_application_error(error, span, self.engine);
                Some(Type::Any)
            }
        }
    }
    pub(crate) fn native_anonymous_annotation(&mut self, ty: &TypeExpr) -> Option<Type> {
        if self.named_type_context.is_none() {
            return None;
        }
        let key = self.native?.named_key_for_annotation(ty)?;
        match self.engine.named_type_reference(key, &[]) {
            Ok(ty) => Some(ty),
            Err(error) => {
                self.native
                    .unwrap()
                    .record_named_application_error(error, ty.span, self.engine);
                Some(Type::Any)
            }
        }
    }
}

#[cfg(test)]
mod dependency_tests {
    use super::*;
    use luanalysis_engine::{NativeFunctionKey, NativeTypeProvider};

    fn registry(source: &str) -> NativeRegistry {
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        NativeRegistry::new(file, bound)
    }
    fn function(registry: &NativeRegistry, name: &str) -> NativeFunctionKey {
        let index = registry
            .function_names
            .iter()
            .position(|candidate| {
                candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.text == name)
            })
            .unwrap();
        NativeFunctionKey {
            snapshot: registry.snapshot,
            index: index as u32,
        }
    }
    #[test]
    fn alias_owned_generic_names_shadow_lexical_names_without_outer_capture() {
        let registry = registry(
            "---@generic T\nlocal function outer()\n---@alias Box<T> {value:T}\n---@return Box<number>\nlocal function inner() return {} end\nreturn inner\nend",
        );
        let inner = function(&registry, "inner");
        assert!(registry.function_outer_type_parameters(inner).is_empty());
    }
    #[test]
    fn aliases_collect_only_referenced_lexical_parameters_and_stop_metadata_cycles() {
        let registry = registry(
            "---@generic T,U\nlocal function outer()\n---@alias First Second\n---@alias Second {next:First|nil,value:T}\n---@alias Unused U\n---@return First\nlocal function inner() return {} end\nreturn inner\nend",
        );
        let outer = function(&registry, "outer");
        let inner = function(&registry, "inner");
        assert_eq!(
            registry.function_outer_type_parameters(inner),
            vec![registry.types.own_parameters(outer)[0]]
        );
        assert!(registry.report.borrow().diagnostics.is_empty()); // Metadata collection emits no alias cycle/type diagnostics.
    }
}
