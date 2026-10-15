//! Immutable source recipes for named declarations. This module indexes Lua
//! origins/owners only; the native core resolves alias heads, cycles, members,
//! inheritance, instantiation and relations. It does not manufacture TS nodes.
use super::bind::{BindingId, BoundFile, ScopeId};
use luanalysis_engine::NativeFunctionKey;
use luanalysis_syntax::{
    Expr, ExprKind, FieldDeclaration, Function, GenericParameter, Name, NamedTypeDeclaration,
    NamedTypeDeclarationId, NamedTypeDeclarationKind, Span, Stmt, StmtKind, TableFieldKind,
    TypeExpr, TypeExprKind, TypeExtension, TypeFieldKey,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct NativeNamedKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct NativeNamedMemberKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct NativeAugmentationKey {
    pub snapshot: u64,
    pub index: u32,
}

/// Anonymous heads occupy the same backend declaration namespace after all
/// named keys, while the source uses a distinct Rust key type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct NativeAnonymousHeadKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct NativeAnonymousMemberKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AnonymousHeadKind {
    Record,
    Array,
    Table,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AnonymousHeadOrigin {
    Named(NativeNamedKey),
    Member(NativeNamedMemberKey),
    Function(NativeFunctionKey),
    RuntimeAnnotation(Span),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AnonymousHeadUse {
    pub origin: AnonymousHeadOrigin,
    pub lexical_context: Option<NativeTypeContext>,
    pub documentation_contexts: Vec<NativeNamedKey>,
}

#[derive(Clone, Debug)]
pub(super) struct AnonymousHeadSource {
    pub key: NativeAnonymousHeadKey,
    pub kind: AnonymousHeadKind,
    /// The exact unexpanded node, including array element or table arguments.
    pub syntax: TypeExpr,
    pub lexical_context: Option<NativeTypeContext>,
    pub documentation_contexts: Vec<NativeNamedKey>,
    pub origins: Vec<AnonymousHeadOrigin>,
    /// A function `@type fun(...)` can be retained on both a local and its
    /// initializer function. Keep both source uses instead of losing a scope.
    pub uses: Vec<AnonymousHeadUse>,
    pub members: Vec<NativeAnonymousMemberKey>,
}

#[derive(Clone, Debug)]
pub(super) enum AnonymousMemberKeyRecipe {
    Record(TypeFieldKey),
    /// A semantic builtin domain recipe, not a fabricated annotation AST.
    ArrayNumber,
    Table(TypeExpr),
}

#[derive(Clone, Debug)]
pub(super) struct AnonymousMemberSource {
    pub key: NativeAnonymousMemberKey,
    pub owner: NativeAnonymousHeadKey,
    pub key_recipe: AnonymousMemberKeyRecipe,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum NativeTypeContext {
    Function(NativeFunctionKey),
    NamedType(NativeNamedKey),
    Signature(luanalysis_engine::NativeSignatureKey),
}

#[derive(Clone, Debug)]
pub(super) struct NamedGenericParameterRecipe {
    pub owner: NativeTypeContext,
    pub order: u32,
    pub name: Name,
    pub constraint: Option<TypeExpr>,
}

#[derive(Clone, Debug)]
pub(super) struct NamedGenericScopeRecipe {
    pub owner: NativeTypeContext,
    pub parent: Option<NativeTypeContext>,
    pub parameters: Vec<NamedGenericParameterRecipe>,
    /// Raw lexical record owners in the same original documentation group.
    /// Distinct from an augmentation's named target; ambiguity is not hidden.
    pub documentation_parents: Vec<NativeNamedKey>,
}

#[derive(Clone, Debug)]
pub(super) struct NamedDeclarationSource {
    pub key: NativeNamedKey,
    pub syntax: NamedTypeDeclaration,
    pub scope: Option<ScopeId>,
    pub generic_scope: NamedGenericScopeRecipe,
    pub members: Vec<NativeNamedMemberKey>,
    pub documentation_contexts: Vec<NativeNamedKey>,
}

#[derive(Clone, Debug)]
pub(super) enum NamedMemberOwner {
    Declared(NativeNamedKey),
    /// This is a reference, not another declaration. An alias/duplicate owner
    /// is resolved/validated by the core rather than selecting a source copy.
    Reference {
        name: Name,
        candidates: Vec<NativeNamedKey>,
        augmentation: NativeAugmentationKey,
    },
}

#[derive(Clone, Debug)]
pub(super) struct NamedMemberSource {
    pub key: NativeNamedMemberKey,
    pub owner: NamedMemberOwner,
    pub field: FieldDeclaration,
    pub lexical_context: Option<NativeTypeContext>,
    pub documentation_contexts: Vec<NativeNamedKey>,
}

#[derive(Clone, Debug)]
pub(super) struct AugmentationSource {
    pub key: NativeAugmentationKey,
    pub syntax: TypeExtension,
    pub scope: Option<ScopeId>,
    pub lexical_context: Option<NativeTypeContext>,
    pub candidates: Vec<NativeNamedKey>,
    pub members: Vec<NativeNamedMemberKey>,
    pub documentation_contexts: Vec<NativeNamedKey>,
}

#[derive(Clone, Debug)]
pub(super) enum RuntimeAttachmentKind {
    Local { bindings: Vec<BindingId> },
    Assignment { targets: Vec<Expr> },
    LocalFunction { binding: BindingId },
    NamedFunction { target: Expr },
    Other,
}

#[derive(Clone, Debug)]
pub(super) struct RuntimeAttachment {
    pub span: Span,
    pub kind: RuntimeAttachmentKind,
    pub declarations: Vec<NativeNamedKey>,
    pub augmentations: Vec<NativeAugmentationKey>,
    pub has_explicit_type_annotation: bool,
    pub local_values: Vec<AttachmentValueOrigin>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct AttachmentValueOrigin {
    pub span: Span,
    pub table_literal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NamedRegistryError {
    ForeignFunction(NativeFunctionKey),
    DuplicateSyntaxId(NamedTypeDeclarationId),
    MissingAttachment(Span),
    MissingAttachmentBinding(Span),
}

pub(super) struct NativeNamed {
    snapshot: u64,
    declarations: Vec<NamedDeclarationSource>,
    names: HashMap<String, Vec<NativeNamedKey>>,
    syntax_keys: HashMap<NamedTypeDeclarationId, NativeNamedKey>,
    members: Vec<NamedMemberSource>,
    augmentations: Vec<AugmentationSource>,
    attachments: HashMap<(usize, usize), RuntimeAttachment>,
    anonymous_heads: Vec<AnonymousHeadSource>,
    anonymous_by_span: HashMap<(usize, usize), NativeAnonymousHeadKey>,
    anonymous_members: Vec<AnonymousMemberSource>,
}

impl NativeNamed {
    pub(super) fn new(
        snapshot: u64,
        file: &luanalysis_syntax::File,
        bound: &BoundFile,
        functions: &HashMap<(usize, usize), NativeFunctionKey>,
    ) -> Result<Self, NamedRegistryError> {
        for &function in functions.values() {
            if function.snapshot != snapshot {
                return Err(NamedRegistryError::ForeignFunction(function));
            }
        }
        let mut result = Self {
            snapshot,
            declarations: Vec::new(),
            names: HashMap::new(),
            syntax_keys: HashMap::new(),
            members: Vec::new(),
            augmentations: Vec::new(),
            attachments: HashMap::new(),
            anonymous_heads: Vec::new(),
            anonymous_by_span: HashMap::new(),
            anonymous_members: Vec::new(),
        };
        // Allocate every declaration/name before target, member or augmentation
        // metadata can ask for a forward reference. Duplicate names survive.
        for declaration in &file.type_declarations {
            let key = NativeNamedKey {
                snapshot,
                index: result
                    .declarations
                    .len()
                    .try_into()
                    .expect("named index fits u32"),
            };
            if result.syntax_keys.insert(declaration.id, key).is_some() {
                return Err(NamedRegistryError::DuplicateSyntaxId(declaration.id));
            }
            result
                .names
                .entry(declaration.name.text.clone())
                .or_default()
                .push(key);
            let parent = enclosing_function(declaration.name.span, functions);
            result.declarations.push(NamedDeclarationSource {
                key,
                syntax: declaration.clone(),
                scope: bound.scope_at(declaration.name.span.start),
                generic_scope: named_scope(key, parent, &declaration.generics),
                members: Vec::new(),
                documentation_contexts: Vec::new(),
            });
        }
        // Keep every original candidate; selecting a lexical owner is a later
        // binding/diagnostic decision, never a name-based augmentation rewrite.
        for index in 0..result.declarations.len() {
            let group = result.declarations[index].syntax.group_span;
            let contexts = result.documentation_contexts(group);
            result.declarations[index]
                .generic_scope
                .documentation_parents = contexts
                .iter()
                .copied()
                .filter(|&key| key != result.declarations[index].key)
                .collect();
            result.declarations[index].documentation_contexts = contexts;
        }
        for index in 0..result.declarations.len() {
            let key = result.declarations[index].key;
            let fields = match &result.declarations[index].syntax.kind {
                NamedTypeDeclarationKind::Alias { .. } => Vec::new(),
                NamedTypeDeclarationKind::Record { fields, .. } => fields.clone(),
            };
            for field in fields {
                let member = result.push_member(
                    NamedMemberOwner::Declared(key),
                    field,
                    Some(NativeTypeContext::NamedType(key)),
                    result.declarations[index].documentation_contexts.clone(),
                );
                result.declarations[index].members.push(member);
            }
        }
        for extension in &file.type_extensions {
            let key = NativeAugmentationKey {
                snapshot,
                index: result
                    .augmentations
                    .len()
                    .try_into()
                    .expect("augmentation index fits u32"),
            };
            let candidates = result.candidates(&extension.owner.text).to_vec();
            let lexical_context = enclosing_function(extension.span, functions);
            let documentation_contexts = result.documentation_contexts(extension.group_span);
            let mut members = Vec::new();
            for field in &extension.fields {
                members.push(result.push_member(
                    NamedMemberOwner::Reference {
                        name: extension.owner.clone(),
                        candidates: candidates.clone(),
                        augmentation: key,
                    },
                    field.clone(),
                    lexical_context,
                    documentation_contexts.clone(),
                ));
            }
            result.augmentations.push(AugmentationSource {
                key,
                syntax: extension.clone(),
                scope: bound.scope_at(extension.span.start),
                lexical_context,
                candidates,
                members,
                documentation_contexts,
            });
        }
        let source = source_index(&file.statements);
        result.index_anonymous(&source, functions);
        let statements = source.statements;
        for declaration in &result.declarations {
            if let Some(span) = declaration.syntax.attachment {
                let statement = statements
                    .get(&(span.start, span.end))
                    .copied()
                    .ok_or(NamedRegistryError::MissingAttachment(span))?;
                result
                    .attachments
                    .entry((span.start, span.end))
                    .or_insert(runtime_attachment(statement, bound)?)
                    .declarations
                    .push(declaration.key);
            }
        }
        for extension in &result.augmentations {
            if let Some(span) = extension.syntax.attachment {
                let statement = statements
                    .get(&(span.start, span.end))
                    .copied()
                    .ok_or(NamedRegistryError::MissingAttachment(span))?;
                result
                    .attachments
                    .entry((span.start, span.end))
                    .or_insert(runtime_attachment(statement, bound)?)
                    .augmentations
                    .push(extension.key);
            }
        }
        Ok(result)
    }

    fn push_member(
        &mut self,
        owner: NamedMemberOwner,
        field: FieldDeclaration,
        lexical_context: Option<NativeTypeContext>,
        documentation_contexts: Vec<NativeNamedKey>,
    ) -> NativeNamedMemberKey {
        let key = NativeNamedMemberKey {
            snapshot: self.snapshot,
            index: self
                .members
                .len()
                .try_into()
                .expect("member index fits u32"),
        };
        self.members.push(NamedMemberSource {
            key,
            owner,
            field,
            lexical_context,
            documentation_contexts,
        });
        key
    }

    fn index_anonymous(
        &mut self,
        source: &SourceIndex<'_>,
        functions: &HashMap<(usize, usize), NativeFunctionKey>,
    ) {
        #[derive(Clone)]
        struct Root {
            ty: TypeExpr,
            context: Option<NativeTypeContext>,
            documentation_contexts: Vec<NativeNamedKey>,
            origin: AnonymousHeadOrigin,
        }
        let mut roots = Vec::new();
        for declaration in &self.declarations {
            let origin = AnonymousHeadOrigin::Named(declaration.key);
            let context = Some(NativeTypeContext::NamedType(declaration.key));
            for parameter in &declaration.syntax.generics {
                if let Some(ty) = &parameter.constraint {
                    roots.push(Root {
                        ty: ty.clone(),
                        context,
                        documentation_contexts: declaration.documentation_contexts.clone(),
                        origin: origin.clone(),
                    });
                }
            }
            let ty = match &declaration.syntax.kind {
                NamedTypeDeclarationKind::Alias { target } => Some(target),
                NamedTypeDeclarationKind::Record { base, .. } => base.as_ref(),
            };
            if let Some(ty) = ty {
                roots.push(Root {
                    ty: ty.clone(),
                    context,
                    documentation_contexts: declaration.documentation_contexts.clone(),
                    origin,
                });
            }
        }
        for member in &self.members {
            let origin = AnonymousHeadOrigin::Member(member.key);
            if let TypeFieldKey::Indexed(ty) = &member.field.key {
                roots.push(Root {
                    ty: *ty.clone(),
                    context: member.lexical_context,
                    documentation_contexts: member.documentation_contexts.clone(),
                    origin: origin.clone(),
                });
            }
            roots.push(Root {
                ty: member.field.ty.clone(),
                context: member.lexical_context,
                documentation_contexts: member.documentation_contexts.clone(),
                origin,
            });
        }
        let mut statements: Vec<_> = source.statements.values().copied().collect();
        statements.sort_by_key(|statement| (statement.span.start, statement.span.end));
        for statement in statements {
            let annotation = match &statement.kind {
                StmtKind::Local { annotation, .. } | StmtKind::Assign { annotation, .. } => {
                    annotation.as_ref()
                }
                _ => None,
            };
            if let Some(annotation) = annotation {
                for ty in &annotation.types {
                    roots.push(Root {
                        ty: ty.clone(),
                        context: enclosing_function(statement.span, functions),
                        documentation_contexts: self.documentation_contexts_at(ty.span),
                        origin: AnonymousHeadOrigin::RuntimeAnnotation(statement.span),
                    });
                }
            }
        }
        let mut source_functions: Vec<_> = source.functions.values().copied().collect();
        source_functions.sort_by_key(|function| (function.span.start, function.span.end));
        for function in source_functions {
            let key = functions
                .get(&(function.span.start, function.span.end))
                .copied();
            let context = key.map(NativeTypeContext::Function);
            let origin = key.map_or(
                AnonymousHeadOrigin::RuntimeAnnotation(function.span),
                AnonymousHeadOrigin::Function,
            );
            let annotations = &function.annotations;
            let types = annotations
                .generics
                .iter()
                .filter_map(|parameter| parameter.constraint.as_ref())
                .chain(annotations.params.iter().map(|parameter| &parameter.ty))
                .chain(annotations.vararg.iter().map(|vararg| &vararg.ty))
                .chain(
                    annotations
                        .returns
                        .iter()
                        .flat_map(|annotation| &annotation.types),
                )
                .chain(
                    annotations
                        .type_annotation
                        .iter()
                        .flat_map(|annotation| &annotation.types),
                );
            for ty in types {
                roots.push(Root {
                    ty: ty.clone(),
                    context,
                    documentation_contexts: self.documentation_contexts_at(ty.span),
                    origin: origin.clone(),
                });
            }
        }
        roots.sort_by_key(|root| (root.ty.span.start, root.ty.span.end));
        for root in roots {
            let mut pending = vec![&root.ty];
            while let Some(ty) = pending.pop() {
                let kind = match &ty.kind {
                    TypeExprKind::Record(_) => Some(AnonymousHeadKind::Record),
                    TypeExprKind::Array { .. } => Some(AnonymousHeadKind::Array),
                    TypeExprKind::Generic { base, .. } if base.text == "table" => {
                        Some(AnonymousHeadKind::Table)
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    if let Some(key) = self.anonymous_key_at(ty.span) {
                        let head = &mut self.anonymous_heads
                            [(key.index as usize) - self.declarations.len()];
                        if !head.origins.contains(&root.origin) {
                            head.origins.push(root.origin.clone());
                        }
                        let usage = AnonymousHeadUse {
                            origin: root.origin.clone(),
                            lexical_context: root.context,
                            documentation_contexts: root.documentation_contexts.clone(),
                        };
                        if !head.uses.contains(&usage) {
                            head.uses.push(usage);
                        }
                    } else {
                        let key = NativeAnonymousHeadKey {
                            snapshot: self.snapshot,
                            index: (self.declarations.len() + self.anonymous_heads.len())
                                .try_into()
                                .expect("head index fits u32"),
                        };
                        self.anonymous_by_span
                            .insert((ty.span.start, ty.span.end), key);
                        // Publish identity before indexing child recipes. These
                        // are annotations, never resolved member/alias types.
                        self.anonymous_heads.push(AnonymousHeadSource {
                            key,
                            kind,
                            syntax: ty.clone(),
                            lexical_context: root.context,
                            documentation_contexts: root.documentation_contexts.clone(),
                            origins: vec![root.origin.clone()],
                            uses: vec![AnonymousHeadUse {
                                origin: root.origin.clone(),
                                lexical_context: root.context,
                                documentation_contexts: root.documentation_contexts.clone(),
                            }],
                            members: Vec::new(),
                        });
                        let mut members = Vec::new();
                        match &ty.kind {
                            TypeExprKind::Record(fields) => {
                                for field in fields {
                                    members.push(self.push_anonymous_member(
                                        key,
                                        AnonymousMemberKeyRecipe::Record(field.key.clone()),
                                        field.ty.clone(),
                                        field.span,
                                    ));
                                }
                            }
                            TypeExprKind::Array { element, .. } => {
                                members.push(self.push_anonymous_member(
                                    key,
                                    AnonymousMemberKeyRecipe::ArrayNumber,
                                    *element.clone(),
                                    ty.span,
                                ))
                            }
                            TypeExprKind::Generic { args, .. } if args.len() == 2 => {
                                members.push(self.push_anonymous_member(
                                    key,
                                    AnonymousMemberKeyRecipe::Table(args[0].clone()),
                                    args[1].clone(),
                                    ty.span,
                                ))
                            }
                            _ => {} // Raw malformed container arguments survive for backend diagnostics.
                        }
                        let index = self.anonymous_heads.len() - 1;
                        self.anonymous_heads[index].members = members;
                    }
                }
                match &ty.kind {
                    TypeExprKind::Union(types) | TypeExprKind::Generic { args: types, .. } => {
                        pending.extend(types.iter().rev())
                    }
                    TypeExprKind::Record(fields) => {
                        for field in fields.iter().rev() {
                            pending.push(&field.ty);
                            if let TypeFieldKey::Indexed(key) = &field.key {
                                pending.push(key);
                            }
                        }
                    }
                    TypeExprKind::Array { element, .. } | TypeExprKind::Parenthesized(element) => {
                        pending.push(element)
                    }
                    TypeExprKind::Function(function) => {
                        pending.extend(function.returns.iter().rev());
                        if let Some(vararg) = &function.vararg {
                            pending.push(&vararg.ty);
                        }
                        pending.extend(
                            function
                                .params
                                .iter()
                                .rev()
                                .filter_map(|parameter| parameter.ty.as_ref()),
                        );
                    }
                    TypeExprKind::Named(_) | TypeExprKind::Literal(_) | TypeExprKind::Error => {}
                }
            }
        }
    }

    fn push_anonymous_member(
        &mut self,
        owner: NativeAnonymousHeadKey,
        key_recipe: AnonymousMemberKeyRecipe,
        ty: TypeExpr,
        span: Span,
    ) -> NativeAnonymousMemberKey {
        let key = NativeAnonymousMemberKey {
            snapshot: self.snapshot,
            index: (self.members.len() + self.anonymous_members.len())
                .try_into()
                .expect("anonymous member index fits u32"),
        };
        self.anonymous_members.push(AnonymousMemberSource {
            key,
            owner,
            key_recipe,
            ty,
            span,
        });
        key
    }

    pub(super) fn candidates(&self, name: &str) -> &[NativeNamedKey] {
        self.names.get(name).map_or(&[], Vec::as_slice)
    }
    pub(super) fn documentation_contexts(&self, group: Span) -> Vec<NativeNamedKey> {
        self.declarations
            .iter()
            .filter(|source| {
                source.syntax.group_span == group
                    && matches!(source.syntax.kind, NamedTypeDeclarationKind::Record { .. })
            })
            .map(|source| source.key)
            .collect()
    }
    pub(super) fn documentation_contexts_at(&self, span: Span) -> Vec<NativeNamedKey> {
        self.declarations
            .iter()
            .filter(|source| {
                source.syntax.group_span.start <= span.start
                    && span.end <= source.syntax.group_span.end
                    && matches!(source.syntax.kind, NamedTypeDeclarationKind::Record { .. })
            })
            .map(|source| source.key)
            .collect()
    }
    pub(super) fn anonymous_heads(&self) -> &[AnonymousHeadSource] {
        &self.anonymous_heads
    }
    pub(super) fn anonymous_head(
        &self,
        key: NativeAnonymousHeadKey,
    ) -> Option<&AnonymousHeadSource> {
        if key.snapshot != self.snapshot {
            return None;
        }
        let index = (key.index as usize).checked_sub(self.declarations.len())?;
        self.anonymous_heads.get(index)
    }
    pub(super) fn anonymous_key_at(&self, span: Span) -> Option<NativeAnonymousHeadKey> {
        self.anonymous_by_span.get(&(span.start, span.end)).copied()
    }
    pub(super) fn anonymous_member(
        &self,
        key: NativeAnonymousMemberKey,
    ) -> Option<&AnonymousMemberSource> {
        if key.snapshot != self.snapshot {
            return None;
        }
        let index = (key.index as usize).checked_sub(self.members.len())?;
        self.anonymous_members.get(index)
    }
    pub(super) fn key_for_syntax(&self, syntax: NamedTypeDeclarationId) -> Option<NativeNamedKey> {
        self.syntax_keys.get(&syntax).copied()
    }
    pub(super) fn declarations(&self) -> &[NamedDeclarationSource] {
        &self.declarations
    }
    pub(super) fn augmentations(&self) -> &[AugmentationSource] {
        &self.augmentations
    }
    pub(super) fn declaration(&self, key: NativeNamedKey) -> Option<&NamedDeclarationSource> {
        (key.snapshot == self.snapshot)
            .then(|| self.declarations.get(key.index as usize))
            .flatten()
    }
    pub(super) fn member(&self, key: NativeNamedMemberKey) -> Option<&NamedMemberSource> {
        (key.snapshot == self.snapshot)
            .then(|| self.members.get(key.index as usize))
            .flatten()
    }
    pub(super) fn augmentation(&self, key: NativeAugmentationKey) -> Option<&AugmentationSource> {
        (key.snapshot == self.snapshot)
            .then(|| self.augmentations.get(key.index as usize))
            .flatten()
    }
    pub(super) fn attachment(&self, span: Span) -> Option<&RuntimeAttachment> {
        self.attachments.get(&(span.start, span.end))
    }
    pub(super) fn generic_scope(
        &self,
        owner: NativeTypeContext,
    ) -> Option<&NamedGenericScopeRecipe> {
        let NativeTypeContext::NamedType(key) = owner else {
            return None;
        };
        self.declaration(key).map(|source| &source.generic_scope)
    }
}

fn named_scope(
    key: NativeNamedKey,
    parent: Option<NativeTypeContext>,
    parameters: &[GenericParameter],
) -> NamedGenericScopeRecipe {
    let owner = NativeTypeContext::NamedType(key);
    NamedGenericScopeRecipe {
        owner,
        parent,
        parameters: parameters
            .iter()
            .enumerate()
            .map(|(order, parameter)| NamedGenericParameterRecipe {
                owner,
                order: order.try_into().expect("parameter order fits u32"),
                name: parameter.name.clone(),
                constraint: parameter.constraint.clone(),
            })
            .collect(),
        documentation_parents: Vec::new(),
    }
}

fn enclosing_function(
    span: Span,
    functions: &HashMap<(usize, usize), NativeFunctionKey>,
) -> Option<NativeTypeContext> {
    functions
        .iter()
        .filter(|((start, end), _)| *start <= span.start && span.end <= *end)
        .min_by_key(|((start, end), _)| end - start)
        .map(|(_, &key)| NativeTypeContext::Function(key))
}

fn runtime_attachment(
    statement: &Stmt,
    bound: &BoundFile,
) -> Result<RuntimeAttachment, NamedRegistryError> {
    let kind = match &statement.kind {
        StmtKind::Local { names, .. } => RuntimeAttachmentKind::Local {
            bindings: names
                .iter()
                .map(|name| {
                    bound
                        .binding_at(name.span.start)
                        .ok_or(NamedRegistryError::MissingAttachmentBinding(name.span))
                })
                .collect::<Result<_, _>>()?,
        },
        StmtKind::Assign { targets, .. } => RuntimeAttachmentKind::Assignment {
            targets: targets.clone(),
        },
        StmtKind::LocalFunction { name, .. } => RuntimeAttachmentKind::LocalFunction {
            binding: bound
                .binding_at(name.span.start)
                .ok_or(NamedRegistryError::MissingAttachmentBinding(name.span))?,
        },
        StmtKind::FunctionDeclaration { target, .. } => RuntimeAttachmentKind::NamedFunction {
            target: *target.clone(),
        },
        _ => RuntimeAttachmentKind::Other,
    };
    Ok(RuntimeAttachment {
        span: statement.span,
        kind,
        declarations: Vec::new(),
        augmentations: Vec::new(),
        has_explicit_type_annotation: match &statement.kind {
            StmtKind::Local { annotation, .. } | StmtKind::Assign { annotation, .. } => {
                annotation.is_some()
            }
            StmtKind::LocalFunction { function, .. }
            | StmtKind::FunctionDeclaration { function, .. } => {
                function.annotations.type_annotation.is_some()
            }
            _ => false,
        },
        local_values: match &statement.kind {
            StmtKind::Local { values, .. } => values.iter().map(|value| {
                let mut literal = value;
                while let ExprKind::Parenthesized(inner) = &literal.kind {
                    literal = inner;
                }
                AttachmentValueOrigin {
                    span: value.span,
                    table_literal: matches!(literal.kind, ExprKind::Table(_)),
                }
            }).collect(),
            _ => Vec::new(),
        },
    })
}

struct SourceIndex<'a> {
    statements: HashMap<(usize, usize), &'a Stmt>,
    functions: HashMap<(usize, usize), &'a Function>,
}

fn source_index(statements: &[Stmt]) -> SourceIndex<'_> {
    enum Node<'a> {
        Statement(&'a Stmt),
        Expression(&'a Expr),
    }
    let mut pending: Vec<_> = statements.iter().map(Node::Statement).collect();
    let mut result = HashMap::new();
    let mut functions = HashMap::new();
    while let Some(node) = pending.pop() {
        match node {
            Node::Statement(statement) => {
                result.insert((statement.span.start, statement.span.end), statement);
                match &statement.kind {
                    StmtKind::Local { values, .. } | StmtKind::Return(values) => {
                        pending.extend(values.iter().map(Node::Expression))
                    }
                    StmtKind::Assign {
                        targets, values, ..
                    } => pending.extend(targets.iter().chain(values).map(Node::Expression)),
                    StmtKind::Block { body, .. } => {
                        pending.extend(body.iter().map(Node::Statement))
                    }
                    StmtKind::While {
                        condition, body, ..
                    }
                    | StmtKind::Repeat {
                        condition, body, ..
                    } => {
                        pending.push(Node::Expression(condition));
                        pending.extend(body.iter().map(Node::Statement));
                    }
                    StmtKind::NumericFor {
                        start,
                        limit,
                        step,
                        body,
                        ..
                    } => {
                        pending.push(Node::Expression(start));
                        pending.push(Node::Expression(limit));
                        if let Some(step) = step {
                            pending.push(Node::Expression(step));
                        }
                        pending.extend(body.iter().map(Node::Statement));
                    }
                    StmtKind::GenericFor { values, body, .. } => {
                        pending.extend(values.iter().map(Node::Expression));
                        pending.extend(body.iter().map(Node::Statement));
                    }
                    StmtKind::LocalFunction { function, .. }
                    | StmtKind::FunctionDeclaration { function, .. } => {
                        functions.insert((function.span.start, function.span.end), &**function);
                        pending.extend(function.body.iter().map(Node::Statement))
                    }
                    StmtKind::If {
                        branches,
                        else_branch,
                    } => {
                        for branch in branches {
                            pending.push(Node::Expression(&branch.condition));
                            pending.extend(branch.body.iter().map(Node::Statement));
                        }
                        if let Some(branch) = else_branch {
                            pending.extend(branch.body.iter().map(Node::Statement));
                        }
                    }
                    StmtKind::Call(expression) => pending.push(Node::Expression(expression)),
                    StmtKind::Break => {}
                }
            }
            Node::Expression(expression) => match &expression.kind {
                ExprKind::Function(function) => {
                    functions.insert((function.span.start, function.span.end), &**function);
                    pending.extend(function.body.iter().map(Node::Statement))
                }
                ExprKind::Unary { value, .. } | ExprKind::Parenthesized(value) => {
                    pending.push(Node::Expression(value))
                }
                ExprKind::Binary { left, right, .. }
                | ExprKind::Index {
                    base: left,
                    index: right,
                } => {
                    pending.push(Node::Expression(left));
                    pending.push(Node::Expression(right));
                }
                ExprKind::Field { base, .. } => pending.push(Node::Expression(base)),
                ExprKind::Call { callee, args } => {
                    pending.push(Node::Expression(callee));
                    pending.extend(args.iter().map(Node::Expression));
                }
                ExprKind::MethodCall { receiver, args, .. } => {
                    pending.push(Node::Expression(receiver));
                    pending.extend(args.iter().map(Node::Expression));
                }
                ExprKind::Table(fields) => {
                    for field in fields {
                        pending.push(Node::Expression(&field.value));
                        if let TableFieldKind::Keyed(key) = &field.kind {
                            pending.push(Node::Expression(key));
                        }
                    }
                }
                ExprKind::Nil
                | ExprKind::Boolean(_)
                | ExprKind::Number(_)
                | ExprKind::String(_)
                | ExprKind::Name(_)
                | ExprKind::Vararg
                | ExprKind::Error => {}
            },
        }
    }
    SourceIndex {
        statements: result,
        functions,
    }
}
