//! Immutable source recipes for native lazy named queries. Type resolution,
//! recursive publication, substitution and compatibility stay in the core.
use super::NativeRegistry;
use crate::native_named::*;
use crate::native_types::TypeNameResolution;
use crate::{CheckReport, Checker, Diagnostic};
use luanalysis_engine::{
    LuaKey, NamedTypeError, NamedTypeQuery, NativeTypeDeclarationKey, NativeTypeMemberKey,
    NativeTypeParameterKey, ObjectDeclarationForm, Type, TypeDeclarationHeader,
    TypeDeclarationKind, TypeDeclarationOwner, TypeMemberHeader, TypeMemberKeyRecipe,
    TypeMemberOwner, TypeMemberVisibility, TypeQueries,
};
use luanalysis_syntax::{
    NamedRecordKind, NamedTypeDeclarationKind, Span, TypeExpr, TypeExprKind,
    TypeFieldKey,
};
use crate::collections::{HashMap, HashSet};

/// Completed original-source headers only: kinds, names and parameter keys.
/// No checker type, inferred result, mapper or query permission is retained.
#[derive(Default)]
pub(super) struct SourceHeaderMetadata {
    generation: u64,
    scope_revisions: Vec<u64>,
    headers: HashMap<NativeTypeDeclarationKey, TypeDeclarationHeader>,
}
impl SourceHeaderMetadata {
    pub(super) fn invalidate(&mut self) {
        self.headers.clear();
        self.generation = self.generation.checked_add(1).expect("original header metadata generation fits u64");
        crate::native_types::advance_metadata_clock();
    }
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 { self.generation }
    pub(super) fn get(&mut self, revisions: &[u64], key: NativeTypeDeclarationKey) -> Option<TypeDeclarationHeader> {
        if self.scope_revisions != revisions {
            self.headers.clear();
            self.scope_revisions = revisions.to_vec();
        }
        self.headers.get(&key).cloned()
    }
    pub(super) fn insert(&mut self, key: NativeTypeDeclarationKey, header: TypeDeclarationHeader) {
        self.headers.insert(key, header);
    }
}

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
    pub(crate) fn named_owner_scope_recipe(&self, key: NativeTypeDeclarationKey) -> luanalysis_engine::OwnerScopeRecipe {
        use luanalysis_engine::OwnerScopeRecipe;
        let Some(source) = self.named.declaration(named_key(key)) else {
            return OwnerScopeRecipe::Original(TypeDeclarationKind::Shape);
        };
        match &source.syntax.kind {
            NamedTypeDeclarationKind::Record { flavor, .. } => OwnerScopeRecipe::Original(match flavor {
                NamedRecordKind::Class => TypeDeclarationKind::Class,
                NamedRecordKind::Shape => TypeDeclarationKind::Shape,
                NamedRecordKind::Module => TypeDeclarationKind::Module,
            }),
            NamedTypeDeclarationKind::Alias { target } => {
                let mut target = target;
                while let TypeExprKind::Parenthesized(inner) = &target.kind { target = inner; }
                let name = match &target.kind {
                    TypeExprKind::Named(name) => Some(name.as_str()),
                    TypeExprKind::Generic { base, .. } => Some(base.text.as_str()),
                    _ => None,
                };
                if let Some(key) = self.named.anonymous_key_at(target.span) {
                    OwnerScopeRecipe::AliasReference { candidates: vec![anonymous(key)] }
                } else if let Some(name) = name {
                    OwnerScopeRecipe::AliasReference { candidates: self.named_candidates(name) }
                } else {
                    OwnerScopeRecipe::UnsupportedAlias
                }
            }
        }
    }
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
        let mut seen = HashSet::default();
        while let Some((context, ty)) = pending.pop() {
            let mut reference = None;
            match &ty.kind {
                TypeExprKind::Named(name) => reference = Some(name.as_str()),
                TypeExprKind::Generic { base, args, .. } => {
                    reference = Some(base.text.as_str());
                    pending.extend(args.iter().map(|ty| (context, ty)));
                }
                TypeExprKind::Union(types) | TypeExprKind::Intersection(types) | TypeExprKind::TemplateLiteral { types, .. } => pending.extend(types.iter().map(|ty| (context, ty))),
                TypeExprKind::Conditional { check, extends, yes, no } => pending.extend([check, extends, yes, no].map(|ty| (context, ty.as_ref()))),
                TypeExprKind::Mapped(mapped) => {
                    pending.extend([&mapped.constraint].into_iter().chain(&mapped.name).chain([&mapped.ty]).map(|ty| (context, ty)));
                }
                TypeExprKind::Array { element, .. } | TypeExprKind::Parenthesized(element)
                | TypeExprKind::TypePredicate { ty: Some(element), .. } | TypeExprKind::KeyOf(element) => {
                    pending.push((context, element))
                }
                TypeExprKind::IndexedAccess { object, index } => pending.extend([(context, object.as_ref()), (context, index.as_ref())]),
                TypeExprKind::Record(fields) => {
                    for field in fields {
                        pending.push((context, &field.ty));
                        if let TypeFieldKey::Indexed(key) = &field.key {
                            pending.push((context, key));
                        }
                    }
                }
                TypeExprKind::Function(function) => {
                    let context = self.signature_context_for_annotation(ty.span)
                        .map(NativeTypeContext::Signature).or(context);
                    pending.extend(function.generics.iter().filter_map(|parameter| parameter.constraint.as_ref()).map(|ty| (context, ty)));
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
                // `infer R` in a function type's result is that type's use of `R`.
                TypeExprKind::Infer { name, constraint } => {
                    used.extend(self.types.declared_parameter(name.span));
                    pending.extend(constraint.as_deref().map(|ty| (context, ty)));
                }
                TypeExprKind::Literal(_) | TypeExprKind::CompletionHint(_) | TypeExprKind::TypePredicate { ty: None, .. }
                | TypeExprKind::TypeQuery(_) | TypeExprKind::Error => {}
            }
            let Some(name) = reference else { continue };
            // Match annotation name-binding precedence for primitive keywords.
            if matches!(
                name,
                "any" | "unknown" | "never" | "nil" | "boolean" | "number" | "integer" | "string" | "void" | "table" | "function"
            ) {
                continue;
            }
            if let Some(key) = self.types.visible_annotation_parameter(name, ty.span) {
                used.insert(key);
                continue;
            }
            if name == "self"
                && let Some(function) = context.and_then(|context| self.metatable_annotation_function(context))
                && self.method_owner(function).is_none()
                && let Some(candidate) = self.contextual_self_dependency(function)
            {
                let explicit = match self.types.resolve_name_in_context(context, name) {
                    TypeNameResolution::Parameter(key) => !self.types.is_semantic_self_parameter(key),
                    TypeNameResolution::Ambiguous(_) => true,
                    TypeNameResolution::Missing => false,
                };
                if !explicit { used.insert(candidate); }
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
                // Written callable heads are original annotation recipes too.
                // Keep their own signature scopes while following named edges
                // through this same visited worklist (including recursive heads).
                for signature in self.signatures.named_heads(key) {
                    let context = Some(NativeTypeContext::Signature(signature));
                    let signature = self.signatures.get(signature).signature.as_ref()
                        .expect("named callable head retains its original function-type annotation");
                    pending.extend(signature.generics.iter().filter_map(|parameter| parameter.constraint.as_ref()).map(|ty| (context, ty)));
                    pending.extend(signature.params.iter().filter_map(|parameter| parameter.ty.as_ref()).map(|ty| (context, ty)));
                    pending.extend(signature.returns.iter().map(|ty| (context, ty)));
                    if let Some(vararg) = &signature.vararg {
                        pending.push((context, &vararg.ty));
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
        self.runtime_schema_contains(key) || self.named.declaration(named_key(key)).is_some()
            || self.named.anonymous_head(anonymous_key(key)).is_some()
    }
    pub(super) fn named_contains_member(&self, key: NativeTypeMemberKey) -> bool {
        self.runtime_tables.member(key).is_some() || self.doc_members.get(key).is_some() || self.members.get(key).is_some() || self.namespace_contains_member(key)
            || self.named.member(member_key(key)).is_some()
            || self
                .named
                .anonymous_member(anonymous_member_key(key))
                .is_some()
    }
    /// The type parameters that a lazily lowered piece `ty` of a conditional or mapped type
    /// (a branch, a template, an `as` clause), lowered in `context`, uses from outside it: those
    /// in scope there, and those of enclosing conditional and mapped types that it sees.
    pub(crate) fn annotation_piece_outer_parameters(
        &self,
        context: Option<NativeTypeContext>,
        ty: &TypeExpr,
    ) -> Vec<NativeTypeParameterKey> {
        if context.is_some() {
            return self.named_outer_parameters(context, ty);
        }
        let mut used = HashSet::default();
        self.annotation_parameter_dependencies(None, ty, &mut used);
        let mut outer: Vec<_> = self.types.visible_annotation_parameters(ty.span).filter(|key| used.contains(key)).collect();
        outer.dedup();
        outer
    }
    pub(crate) fn named_outer_parameters(
        &self,
        context: Option<NativeTypeContext>,
        ty: &TypeExpr,
    ) -> Vec<NativeTypeParameterKey> {
        let Some(context) = context else {
            return Vec::new();
        };
        let mut used = HashSet::default();
        self.annotation_parameter_dependencies(Some(context), ty, &mut used);
        let mut outer: Vec<_> = self.types
            .outer_parameters_in_context(context, &used)
            .into_iter()
            .chain(self.types.own_parameters_in_context(context))
            .chain(self.types.annotation_parameters_in_context(context))
            .filter(|key| used.contains(key))
            .collect();
        outer.dedup();
        // What is written in a conditional type sees its `infer` type parameters.
        outer.extend(self.types.visible_annotation_parameters(ty.span).filter(|key| used.contains(key) && !outer.contains(key)).collect::<Vec<_>>());
        outer
    }
    pub(crate) fn named_declaration_header(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> TypeDeclarationHeader {
        // Every original scope revision advances the thread's metadata clock.
        let revisions = [crate::native_types::metadata_clock()];
        if let Some(header) = self.header_metadata.borrow_mut().get(&revisions, key) { return header; }
        // Each original owner is traversed with the existing fresh visited
        // sets. A recursive sub-traversal never publishes a partial closure.
        let header = self.named_declaration_header_uncached(key);
        assert_eq!([crate::native_types::metadata_clock()], revisions,
            "header dependency discovery only reads original scope metadata");
        self.header_metadata.borrow_mut().headers.insert(key, header.clone());
        header
    }
    fn named_declaration_header_uncached(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> TypeDeclarationHeader {
        if let Some(header)=self.runtime_schema_header(key) {return header;}
        if let Some(source) = self.named.declaration(named_key(key)) {
            let context = NativeTypeContext::NamedType(source.key);
            let mut used = HashSet::default();
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
            for signature in self.signatures.named_heads(source.key) {
                used.extend(self.signature_outer_keys(signature));
            }
            self.original_member_dependencies(source.key, &mut used);
            self.doc_members.dependencies(source.key,&mut used,|id|self.doc_signature_key(id),|key|self.signature_outer_keys(key));
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
        // An alias names the head it is, `---@alias Names string[]`, not the heads nested in it
        // or in a base's type arguments.
        let name = source
            .origins
            .iter()
            .find_map(|origin| match origin {
                AnonymousHeadOrigin::Named(key) => self
                    .named
                    .declaration(*key)
                    .filter(|named| match &named.syntax.kind {
                        NamedTypeDeclarationKind::Alias { target } => {
                            let mut target = target;
                            while let TypeExprKind::Parenthesized(inner) = &target.kind { target = inner; }
                            target.span == source.syntax.span
                        }
                        NamedTypeDeclarationKind::Record { .. } => false,
                    })
                    .map(|named| named.syntax.name.text.clone()),
                _ => None,
            })
            // Anonymous containers have no declaration name to print. Their
            // original keys and form metadata still own the same schemas.
            .unwrap_or_default();
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
    /// Where a member is declared, for the language service: its span, and whether that is a
    /// documentation tag (`---@field`, `---@function`) rather than a statement.
    /// Where the constructor of runtime table `key` declares its field `name`: the key of
    /// `name = value` or `["name"] = value`.
    pub(crate) fn runtime_table_field_site(&self, key: NativeTypeDeclarationKey, name: &str) -> Option<Span> {
        self.runtime_tables.schema(key)?.fields.iter().find_map(|field| match &field.kind {
            luanalysis_syntax::TableFieldKind::Named(field) => (field.text == name).then_some(field.span),
            luanalysis_syntax::TableFieldKind::Keyed(key) => {
                matches!(&key.kind, luanalysis_syntax::ExprKind::String(bytes) if bytes == name.as_bytes()).then_some(key.span)
            }
            luanalysis_syntax::TableFieldKind::Sequence { .. } => None,
        })
    }
    pub(crate) fn member_site(&self, key: NativeTypeMemberKey) -> Option<(Span, bool)> {
        if let Some(store) = self.runtime_tables.member(key) { return Some((store.span, false)); }
        if let Some(source) = self.doc_members.get(key) { return Some((source.signature_span, true)); }
        if let Some(source) = self.members.get(key) { return Some((source.candidates.first()?.span, false)); }
        if key.snapshot != self.snapshot { return None; }
        self.named.member(member_key(key)).map(|source| (source.field.span, true))
    }
    pub(super) fn named_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        if let Some(header)=self.runtime_member_header(key) {return header;}
        if let Some(header)=self.doc_members.header(key) {return header;}
        if let Some(header) = self.original_member_header(key) { return header; }
        if let Some(header) = self.namespace_member_header(key) { return header; }
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
                // Whatever its visibility, the field is a member of its class; the visibility
                // restricts where it is accessed.
                visibility: crate::native_members::member_visibility(source.field.visibility.map(|visibility| visibility.kind)),
                // `name? T` may be absent whatever T is: `any | nil` is `any`.
                optional: source.field.optional,
                readonly: source.field.readonly.is_some(),
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
            optional: source.optional,
            readonly: source.readonly,
        }
    }
    pub(super) fn named_declaration_members(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> Vec<NativeTypeMemberKey> {
        if let Some(constructor) = self.runtime_schema_constructor(key) {
            // Only the stores this schema owns, as a binder sees them.
            return if self.source_role() == luanalysis_project::SourceRole::Implementation {
                self.runtime_tables.members_with_stores()
                    .filter(|&(_, store)| self.runtime_store_owner(store) == Some(constructor))
                    .map(|(member, _)| member)
                    .collect()
            } else {Vec::new()};
        }
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
        result.extend(self.members.member_candidates(named_key(key)));
        result.extend(self.doc_members.keys());
        result.extend(
            self.named
                .augmentations()
                .iter()
                .flat_map(|source| source.members.iter().copied())
                .map(member),
        );
        result.extend(self.namespace_members_for(key));
        result
    }
    pub(crate) fn named_annotation(
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
            Some(NativeTypeContext::Annotation(_)) | None => {}
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
        if let Some(ty)=self.runtime_member_type(key,queries) {return ty;}
        if let Some(ty)=self.doc_members.ty(key,queries,|id|self.doc_signature_key(id)) {return ty;}
        if let Some(ty) = self.original_member_type(key, queries) { return ty; }
        if let Some(ty) = self.namespace_member_type(key, queries) { return ty; }
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
        if let Some(ty)=self.runtime_member_index_key(key,queries) {return ty;}
        if let Some(ty) = self.original_member_indexer_key(key, queries) { return ty; }
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
        if !self.types.contains(key) {
            return None;
        }
        match self.types.parameter(key).owner {
            NativeTypeContext::Function(key) => Some(TypeDeclarationOwner::Function(key)),
            NativeTypeContext::Signature(key) => Some(TypeDeclarationOwner::Signature(key)),
            NativeTypeContext::NamedType(key) => Some(TypeDeclarationOwner::NamedType(declaration(key))),
            // A conditional type is not a declaration.
            NativeTypeContext::Annotation(_) => None,
        }
    }
    pub(super) fn named_key_for_annotation(
        &self,
        ty: &TypeExpr,
    ) -> Option<NativeTypeDeclarationKey> {
        self.named.anonymous_key_at(ty.span).map(anonymous)
    }
    pub(crate) fn named_candidates(&self, name: &str) -> Vec<NativeTypeDeclarationKey> {
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
        if let Some(span)=self.runtime_declaration_origin(key) {return span;}
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
        if let Some(span)=self.runtime_member_origin(key) {return span;}
        if let Some(source)=self.doc_members.get(key) {return source.owner_span;}
        if let Some(span) = self.namespace_member_origin(key) { return span; }
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
            .or_else(|| self.members.get(key).map(|source| source.candidates[0].span))
            .unwrap_or(self.file.span)
    }
    fn named_base_origin(&self, key: NativeTypeDeclarationKey, index: usize) -> Span {
        self.named
            .declaration(named_key(key))
            .and_then(|source| match &source.syntax.kind {
                NamedTypeDeclarationKind::Record { base: Some(base), .. } if index == 0 => {
                    Some(base.span)
                }
                _ => None,
            })
            .unwrap_or_else(|| self.named_origin(key))
    }
    fn named_member_type_origin(&self, key: NativeTypeMemberKey) -> Span {
        if let Some(source)=self.doc_members.get(key) {return source.signature_span;}
        self.named
            .member(member_key(key))
            .map(|source| source.field.ty.span)
            .or_else(|| {
                self.named
                    .anonymous_member(anonymous_member_key(key))
                    .map(|source| source.span)
            })
            .or_else(|| self.members.get(key).map(|source| {
                use crate::native_members::OriginalMemberValue;
                match &source.candidates[0].value {
                    OriginalMemberValue::Written(ty) => ty.span,
                    OriginalMemberValue::Expression(expression) => self.expressions().get(*expression).expression.span,
                    OriginalMemberValue::Function(function) => self.function(*function).span,
                }
            }))
            .unwrap_or_else(|| self.named_member_origin(key))
    }
    pub(super) fn named_declaration_error(&self, error: NamedTypeError) {
        let typed_origin = match &error {
            NamedTypeError::InvalidBase { declaration, index, .. } => Some(("TS2312", self.named_base_origin(*declaration, *index))),
            NamedTypeError::IncompatibleOverride { member, .. } => Some(("TS2416", self.named_member_type_origin(*member))),
            NamedTypeError::IncompatibleBase { declaration, index, .. } => Some(("TS2415", self.named_base_origin(*declaration, *index))),
            NamedTypeError::Table { declaration, .. } => Some(("LUA2032", self.named_origin(*declaration))),
            NamedTypeError::InvalidReceiver { .. } | NamedTypeError::UnsupportedReceiverType { .. }
            | NamedTypeError::DefinitionOwner { .. } | NamedTypeError::DefinitionInitializer { .. }
            | NamedTypeError::DefinitionField { .. } => Some(("LUA2032", self.file.span)),
            _ => None,
        };
        if let Some((code, span)) = typed_origin {
            let (template, types) = crate::diagnostic_types::named_error_description(&error);
            self.report.borrow_mut().push_type_diagnostic(code, span, template, &types.iter().collect::<Vec<_>>());
            return;
        }
        let (code, span, message) = match error {
            NamedTypeError::PendingContext { .. } => return,
            NamedTypeError::InvalidMemberSignatures {member,..} => ("LUA2034",self.named_member_type_origin(member),"Bodyless member has invalid original signature identities.".into()),
            // Actual application spans are supplied by the annotation caller.
            NamedTypeError::TypeArgumentArity { .. }
            | NamedTypeError::TypeArgumentConstraint { .. } => return,
            NamedTypeError::CircularAlias(key) => (
                "TS2456",
                self.named_origin(key),
                "Type alias circularly references itself.".into(),
            ),
            NamedTypeError::CircularBase(key) => (
                "TS2310",
                self.named_base_origin(key, 0),
                "Type recursively references itself as a base type.".into(),
            ),
            NamedTypeError::InvalidBase { declaration, index, .. } => (
                "TS2312",
                self.named_base_origin(declaration, index),
                "A base type must resolve to a class or shape record schema.".into(),
            ),
            NamedTypeError::IncompatibleOverride { member, .. } => (
                "TS2416",
                self.named_member_type_origin(member),
                "Declared member is not assignable to the inherited member contract.".into(),
            ),
            NamedTypeError::IncompatibleBase { declaration, index, .. } => (
                "TS2415",
                self.named_base_origin(declaration, index),
                "Inherited member or call signatures conflict.".into(),
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
            NamedTypeError::Table { .. } => unreachable!("typed declaration error recorded above"),
            NamedTypeError::TooDeep(query) => (
                "TS2589",
                match query {
                    NamedTypeQuery::OwnerScope(key) | NamedTypeQuery::Head(key)
                    | NamedTypeQuery::Bases(key)
                    | NamedTypeQuery::ObjectMembers(key)
                    | NamedTypeQuery::ObjectValidation(key) => self.named_origin(key),
                    NamedTypeQuery::DefinitionContext(key) => self.named_origin(key),
                    NamedTypeQuery::Member(key) | NamedTypeQuery::MemberContext(key) | NamedTypeQuery::MemberOwnerApplication(key) | NamedTypeQuery::MemberOwnerMatch(key) | NamedTypeQuery::Indexer(key) => {
                        self.named_member_origin(key)
                    }
                },
                "Type instantiation is excessively deep.".into(),
            ),
            other => {
                let (message, types) = crate::diagnostic_types::named_error_description(&other);
                debug_assert!(types.is_empty(), "typed declaration error recorded above");
                ("LUA2032", self.file.span, format!("Invalid named declaration metadata: {message}"))
            },
        };
        self.report.borrow_mut().diagnostics.push(Diagnostic {
            code,
            span,
            message,
        });
    }
    /// TypeScript's checks of type parameter defaults: an optional parameter (one with a default)
    /// precedes every required one, a default names no later parameter of its own list, and it
    /// satisfies its parameter's constraint.
    fn check_type_parameter_defaults(&self, queries: &mut dyn TypeQueries) {
        let sources = self.types.parameter_sources();
        let mut diagnostics = Vec::new();
        for source in sources.iter().filter(|source| source.semantic_self.is_none()) {
            let siblings = || sources.iter().filter(move |sibling| sibling.owner == source.owner && sibling.semantic_self.is_none());
            let Some(default) = &source.default else {
                if siblings().any(|sibling| sibling.order < source.order && sibling.default.is_some()) {
                    diagnostics.push(Diagnostic {
                        code: "TS2706",
                        span: source.name.span,
                        message: "Required type parameters may not follow optional type parameters.".into(),
                    });
                }
                continue;
            };
            let later = luanalysis_syntax::referenced_type_names(default).into_iter()
                .find(|name| siblings().any(|sibling| sibling.order > source.order && sibling.name.text == name.text));
            if let Some(name) = later {
                diagnostics.push(Diagnostic {
                    code: "TS2744",
                    span: name.span,
                    message: "Type parameter defaults can only reference previously declared type parameters.".into(),
                });
                continue;
            }
            if source.constraint.is_some()
                && let (Some(default_type), Some(constraint)) = (queries.type_parameter_default(source.key), queries.type_parameter_constraint(source.key))
                && !queries.is_assignable(&default_type, &constraint)
            {
                let mut report = CheckReport::default();
                report.push_type_diagnostic("TS2344", default.span, "Type '{}' does not satisfy the constraint '{}'.", &[&default_type, &constraint]);
                self.merge_report(report);
            }
        }
        self.report.borrow_mut().diagnostics.extend(diagnostics);
    }

    /// The named types this source declares, in order.
    pub(crate) fn named_declaration_keys(&self) -> Vec<NativeTypeDeclarationKey> {
        self.named.declarations().iter().map(|source| declaration(source.key)).collect()
    }

    pub(crate) fn check_named_declarations(&self, queries: &mut dyn TypeQueries) {
        self.check_type_parameter_defaults(queries);
        self.prepare_documentation_scopes(queries);
        self.doc_members.validate(queries);
        self.validate_return_annotations(queries);
        self.validate_original_assignment_annotations(queries);
        let mut seen = HashSet::default();
        for source in self.named.declarations() {
            if let Err(reason) = self.class_definition_directive(source)
                && !source.syntax.attachment.is_some_and(|span| self.is_ambient_global_attachment(span)) {
                self.report.borrow_mut().diagnostics.push(Diagnostic {
                    code: "LUA2031", span: source.syntax.name.span,
                    message: format!("Class '{}' definition is unsupported: {reason}.", source.syntax.name.text),
                });
            }
            if let Err(reason) = self.shape_attachment_directive(source)
                && !source.syntax.attachment.is_some_and(|span| self.is_ambient_global_attachment(span)) {
                self.report.borrow_mut().diagnostics.push(Diagnostic {
                    code:"LUA2031",span:source.syntax.name.span,
                    message:format!("Shape '{}' attachment is unsupported: {reason}.",source.syntax.name.text),
                });
            }
            if !seen.insert(&source.syntax.name.text) && !self.is_merged_declaration(declaration(source.key)) {
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
    pub(crate) fn record_named_application_error(
        &self,
        error: NamedTypeError,
        span: Span,
        _queries: &mut dyn TypeQueries,
    ) {
        if let NamedTypeError::TypeArgumentConstraint { actual, expected, .. } = &error {
            let mut report = CheckReport::default();
            report.push_type_diagnostic("TS2344", span,
                "Type '{}' does not satisfy constraint '{}'.", &[actual, expected]);
            // An annotation can be demanded while evaluating a prospective
            // runtime-store receiver. Its diagnostics belong to that existing
            // report scope, just like the surrounding expression diagnostics.
            self.merge_report(report);
            return;
        }
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
            NamedTypeError::TypeArgumentConstraint { .. } => unreachable!("typed constraint recorded above"),
            _ => None,
        };
        if let Some(diagnostic) = diagnostic {
            self.merge_report(CheckReport { diagnostics: vec![diagnostic], ..CheckReport::default() });
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
                return Some(self.engine.error_type());
            }
        };
        match self.engine.named_type_reference(declaration, arguments) {
            Ok(ty) => {
                // The reference keeps its instantiated type. Its constraints are checked later,
                // at top level (`check_deferred_constraints`), as TypeScript defers
                // `checkTypeArgumentConstraints`: checking here, inside the resolution that
                // lowered it (a base list, a member type), re-entered that resolution.
                if let TypeExprKind::Generic { args, .. } = &syntax.kind
                    && !arguments.is_empty()
                {
                    let spans = args.iter().map(|argument| argument.span).collect();
                    native.defer_constraint_check(syntax.span, spans, declaration, arguments);
                }
                Some(ty)
            }
            Err(error) => {
                let span = match (&error, &syntax.kind) {
                    (
                        NamedTypeError::TypeArgumentConstraint { index, .. },
                        TypeExprKind::Generic { args, .. },
                    ) => args.get(*index).map_or(span, |argument| argument.span),
                    _ => span,
                };
                native.record_named_application_error(error, span, self.engine);
                Some(self.engine.error_type())
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
                Some(self.engine.error_type())
            }
        }
    }
}

#[cfg(test)]
mod dependency_tests {
    use super::*;
    use luanalysis_engine::{Engine, NativeFunctionKey, NativeTypeProvider};

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
    fn prospective_named_application_errors_belong_to_the_owning_check() {
        for (application, code) in [("Box<number>", "TS2344"), ("Box<number, string>", "TS2314")] {
            let source = format!(
                "---@class Required\n---@field id number\n\n---@class Box<T : Required>\n\n---@type {application}\nlocal value\n"
            );
            Engine::new().with_native_provider(
                move || registry(&source),
                |registry, queries| {
                    let syntax = registry.file.statements.iter().find_map(|statement| {
                        let luanalysis_syntax::StmtKind::Local { annotation: Some(annotation), .. } = &statement.kind else { return None };
                        annotation.types.first().cloned()
                    }).expect("original named application annotation");
                    registry.with_body_report(|| {
                        registry.checker(queries).annotation(&syntax);
                        ((), false)
                    });
                    let provisional = registry.take_report();
                    assert!(provisional.diagnostics.is_empty(), "{application}: {:#?}", provisional.diagnostics);
                    assert!(provisional.pending_diagnostics.is_empty(), "a discarded query must retain no diagnostic type capabilities");

                    // The same original annotation is still invalid. Its own
                    // completed check must report it after the candidate ends.
                    registry.checker(queries).annotation(&syntax);
                    registry.check_deferred_constraints(queries);
                    let mut completed = registry.take_report();
                    completed.render(queries);
                    assert_eq!(completed.diagnostics.len(), 1, "{application}: {:#?}", completed.diagnostics);
                    assert_eq!(completed.diagnostics[0].code, code);
                    assert!(completed.pending_diagnostics.is_empty());
                },
            );
        }
    }

    #[test]
    fn completed_body_named_application_errors_survive_staging_and_corrections() {
        for (argument, expected) in [("number", 1), ("Required", 0)] {
            let source = format!(
                "---@class Required\n---@field id number\n\n---@class Box<T : Required>\n\n---@param value Box<{argument}>\nlocal function inspect(value) return value end\n"
            );
            Engine::new().with_native_provider(
                move || registry(&source),
                |registry, queries| {
                    registry.check_body(function(registry, "inspect"), queries);
                    registry.check_deferred_constraints(queries);
                    let mut report = registry.take_report();
                    report.render(queries);
                    assert_eq!(report.diagnostics.len(), expected, "{argument}: {:#?}", report.diagnostics);
                    assert!(report.diagnostics.iter().all(|diagnostic| diagnostic.code == "TS2344"));
                    assert!(report.pending_diagnostics.is_empty());
                },
            );
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

    #[test]
    fn completed_owner_headers_preserve_cyclic_original_parameter_dependencies() {
        let registry = registry(
            "---@generic T,U\nlocal function outer()\n---@alias First Second\n---@alias Second {next:First|nil,value:T}\n---@alias Unused U\nend",
        );
        let key = registry.named_candidates("First")[0];
        let expected = registry.types.own_parameters(function(&registry, "outer"))[0];
        let header = registry.named_declaration_header(key);
        assert_eq!(header.outer_parameters, vec![expected]);
        assert_eq!(registry.named_declaration_header(key), header);
        assert_eq!(registry.named_declaration_header_uncached(key), header);
        assert_eq!(registry.header_metadata.borrow().headers.len(), 1);
        assert!(registry.report.borrow().diagnostics.is_empty());
    }

    #[test]
    fn documentation_owner_preparation_invalidates_completed_anonymous_headers() {
        // A local qualified owner is also an immutable signature parent. Use
        // the valid cross-file preparation path so only the mutable attached
        // owner supplies T to this consumer's original signature.
        let owner_source = registry("---@class Owner<T>\n");
        let mut registry = registry("---@function Owner.read():{value:T}\n");
        registry.types.import_original_scopes(&owner_source.types.original_scopes());
        let owner = owner_source.named_candidates("Owner")[0];
        let owner = NativeNamedKey { snapshot: owner.snapshot, index: owner.index };
        let parameter = owner_source.types.own_parameters_in_context(NativeTypeContext::NamedType(owner))[0];
        let source = registry.named.anonymous_heads().iter().find(|head| {
            matches!(head.syntax.kind, TypeExprKind::Record(_))
        }).expect("original written return record");
        let key = anonymous(source.key);
        let signature = registry.signature_context_for_annotation(source.syntax.span)
            .expect("return record retains its original signature context");
        let context = Some(NativeTypeContext::Signature(signature));
        assert_eq!(registry.types.resolve_name_in_context(context, "T"), TypeNameResolution::Missing);
        assert!(registry.named_declaration_header(key).outer_parameters.is_empty());
        registry.types.attach_documentation_owner(signature, owner);
        assert_eq!(registry.types.resolve_name_in_context(context, "T"), TypeNameResolution::Parameter(parameter));
        let header = registry.named_declaration_header(key);
        assert_eq!(header.outer_parameters, vec![parameter]);
        registry.types.clear_documentation_owner(signature);
        assert_eq!(registry.types.resolve_name_in_context(context, "T"), TypeNameResolution::Missing);
        let uncached = registry.named_declaration_header_uncached(key);
        assert!(uncached.outer_parameters.is_empty());
        assert_eq!(registry.named_declaration_header(key), uncached);
        registry.types.attach_documentation_owner(signature, owner);
        assert_eq!(registry.types.resolve_name_in_context(context, "T"), TypeNameResolution::Parameter(parameter));
        assert_eq!(registry.named_declaration_header(key), header);
    }

    #[test]
    fn imported_original_scope_preparation_invalidates_observers_and_role_rebuilds() {
        let mut source = registry("---@class Owner<U>\n---@generic T\nlocal function outer()\n---@class Nested\nend\n");
        let mut observer = registry("");
        observer.types.import_original_scopes(&source.types.original_scopes());
        let outer = function(&source, "outer");
        let context = NativeTypeContext::Function(outer);
        assert!(matches!(observer.types.resolve_name_in_context(Some(context), "U"), TypeNameResolution::Missing));
        let revisions = observer.types.metadata_revisions();
        let owner = source.named_candidates("Owner")[0];
        let owner = NativeNamedKey { snapshot: owner.snapshot, index: owner.index };
        source.types.attach_owner_scope(context, owner);
        assert_ne!(observer.types.metadata_revisions(), revisions);
        assert!(matches!(observer.types.resolve_name_in_context(Some(context), "U"), TypeNameResolution::Parameter(_)));
        let key = source.named_candidates("Nested")[0];
        let header = source.named_declaration_header(key);
        assert!(!source.header_metadata.borrow().headers.is_empty());
        source.set_source_role(luanalysis_project::SourceRole::Declaration);
        assert!(source.header_metadata.borrow().headers.is_empty());
        assert_eq!(source.named_declaration_header(key), header);
    }
}
