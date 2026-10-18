//! Finite original class member declarations. These are immutable source
//! recipes; native named queries own type evaluation, cycles and substitution.
use crate::native_named::{NativeNamed, NativeNamedKey, NativeTypeContext};
use luanalysis_engine::{LuaKey, NativeExpressionKey, NativeFunctionKey, NativeTypeMemberKey};
use luanalysis_syntax::{Span, TypeExpr};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum OriginalMemberKey {
    Fixed(LuaKey),
    Computed(NativeExpressionKey),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OriginalMemberOrigin {
    ConstructorField(Span),
    AssignmentField(Span),
    Method(NativeFunctionKey),
}
#[derive(Clone, Debug)]
pub(super) enum OriginalMemberValue {
    Written(TypeExpr),
    Expression(NativeExpressionKey),
    Function(NativeFunctionKey),
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OriginalOwnerApplication {
    pub annotation: TypeExpr,
    pub context: Option<NativeTypeContext>,
    pub binding: crate::bind::BindingId,
}
#[derive(Clone, Debug)]
pub(super) struct OriginalMemberInput {
    pub owner: NativeNamedKey,
    pub owner_application: Option<OriginalOwnerApplication>,
    /// This is only a candidate owner until the original metatable call is
    /// proved to invoke the selected standard function at its creation point.
    pub contextual_owner: Option<NativeFunctionKey>,
    pub key: OriginalMemberKey,
    pub origin: OriginalMemberOrigin,
    pub value: OriginalMemberValue,
    pub span: Span,
    pub lexical_context: Option<NativeTypeContext>,
    pub documentation_contexts: Vec<NativeNamedKey>,
}
#[derive(Clone, Debug)]
pub(super) struct OriginalMemberSource {
    pub key: NativeTypeMemberKey,
    /// Original declaration groups occur in source order, identified by their
    /// contextual-owner guard. Duplicate constructor stores within a group
    /// retain Lua collision handling; later writes do not redefine its contract.
    pub candidates: Vec<OriginalMemberInput>,
}
pub(super) struct NativeMembers {
    snapshot: u64,
    base: u32,
    sources: Vec<OriginalMemberSource>,
    inputs: Vec<OriginalMemberInput>,
}
impl NativeMembers {
    pub(super) fn new(snapshot: u64, named: &NativeNamed, mut inputs: Vec<OriginalMemberInput>, base: u32) -> Self {
        assert!(base >= named.member_namespace_end(), "original member namespaces are disjoint");
        inputs.sort_by_key(|input| (input.span.start, input.span.end));
        let mut result = Self { snapshot, base, sources: Vec::new(), inputs: inputs.clone() };
        for input in inputs {
            assert_eq!(input.owner.snapshot, snapshot);
            let doc_priority = named.declaration(input.owner).is_some_and(|owner| {
                owner.members.iter().any(|&key| {
                    let field = &named.member(key).expect("original written member").field;
                    let fixed = match &field.key {
                        luanalysis_syntax::TypeFieldKey::Named(name) => Some(LuaKey::String(name.text.as_bytes().to_vec())),
                        luanalysis_syntax::TypeFieldKey::Indexed(ty) => crate::native_tables::literal_type_key(ty),
                        luanalysis_syntax::TypeFieldKey::Sequence { index } => Some(LuaKey::Integer((*index).try_into().expect("source field index fits i64"))),
                    };
                    fixed.is_some_and(|key| input.key == OriginalMemberKey::Fixed(key))
                })
            });
            if doc_priority { continue; }
            if let Some(previous) = result.sources.iter_mut().find(|source| {
                source.candidates[0].owner == input.owner && source.candidates[0].key == input.key
                    && source.candidates[0].owner_application == input.owner_application
            }) {
                // An inactive earlier constructor scope must not erase a
                // later declaration. Within the same scope, the first store
                // still defines the contract and later methods are checked
                // writes rather than additional overloads or union members.
                let group = previous.candidates.iter().find(|candidate| {
                    candidate.contextual_owner == input.contextual_owner
                });
                if group.is_none_or(|first| {
                    matches!(input.origin, OriginalMemberOrigin::ConstructorField(_))
                        && matches!(first.origin, OriginalMemberOrigin::ConstructorField(_))
                }) {
                    previous.candidates.push(input);
                }
                continue;
            }
            let index = base.checked_add(result.sources.len().try_into().expect("source members fit u32")).expect("source member namespace fits u32");
            result.sources.push(OriginalMemberSource { key: NativeTypeMemberKey { snapshot, index }, candidates: vec![input] });
        }
        result
    }
    pub(super) fn get(&self, key: NativeTypeMemberKey) -> Option<&OriginalMemberSource> {
        if key.snapshot != self.snapshot { return None; }
        self.sources.get(key.index.checked_sub(self.base)? as usize)
    }
    pub(super) fn members(&self, owner: NativeNamedKey) -> impl Iterator<Item = NativeTypeMemberKey> + '_ {
        self.sources.iter().filter(move |source| source.candidates[0].owner == owner).map(|source| source.key)
    }
    pub(super) fn member_candidates(&self, owner: NativeNamedKey) -> impl Iterator<Item = NativeTypeMemberKey> + '_ {
        // An original written application may name an alias. Offer these finite
        // candidates to the native canonical-owner/applicability worker.
        self.sources.iter().filter(move |source| source.candidates[0].owner == owner || source.candidates[0].owner_application.is_some()).map(|source| source.key)
    }
    pub(super) fn applied_members(&self) -> impl Iterator<Item = NativeTypeMemberKey> + '_ {
        self.sources.iter().filter(|source|source.candidates[0].owner_application.is_some()).map(|source|source.key)
    }
    #[cfg(test)]
    pub(super) fn sources(&self) -> &[OriginalMemberSource] { &self.sources }
    pub(super) fn namespace_end(&self) -> u32 { self.base.checked_add(self.sources.len().try_into().expect("original member count fits u32")).expect("original member namespace fits u32") }
    pub(super) fn inputs(&self) -> &[OriginalMemberInput] { &self.inputs }
}

use crate::native_functions::NativeRegistry;
use luanalysis_engine::{TableFieldOrigin, TableKey, TableType, Type, TypeMemberHeader, TypeMemberKeyRecipe, TypeMemberOwner, TypeMemberVisibility, TypeQueries, NativeTypeParameterKey};
use luanalysis_syntax::{ExprKind, TableFieldKind};
use crate::collections::HashSet;

impl NativeRegistry {
    /// Choose the first completed active original declaration group. A pending
    /// earlier group cannot be bypassed by publishing a later group's type.
    pub(crate) fn selected_original_member_candidates<'a>(
        &'a self,
        key: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<(luanalysis_engine::TypeMemberActivation, Vec<&'a OriginalMemberInput>)> {
        use luanalysis_engine::{ContextualTypeOwner, TypeMemberActivation};
        let source = self.members.get(key)?;
        let mut visited = HashSet::default();
        for input in &source.candidates {
            if !visited.insert(input.contextual_owner) { continue; }
            if let Some(scope) = input.contextual_owner {
                match self.contextual_owner_at_creation(scope, queries) {
                    ContextualTypeOwner::Pending => return Some((TypeMemberActivation::Pending, Vec::new())),
                    ContextualTypeOwner::Absent => continue,
                    ContextualTypeOwner::Owner(owner) => {
                        if queries.named_object_declaration(&owner) != Some(luanalysis_engine::NativeTypeDeclarationKey {
                            snapshot: input.owner.snapshot, index: input.owner.index,
                        }) { continue; }
                    }
                }
            }
            let selected = source.candidates.iter().filter(|candidate| {
                candidate.contextual_owner == input.contextual_owner
            }).collect();
            return Some((TypeMemberActivation::Active, selected));
        }
        Some((TypeMemberActivation::Inactive, Vec::new()))
    }
    pub(crate) fn is_original_assignment_field(&self, target: Span) -> bool {
        self.members.inputs().iter().any(|input| input.origin == OriginalMemberOrigin::AssignmentField(target))
    }
    pub(crate) fn original_assignment_annotation(&self, target: Span, queries: &mut dyn TypeQueries) -> Option<Type> {
        let input = self.members.inputs().iter().find(|input| input.origin == OriginalMemberOrigin::AssignmentField(target))?;
        let OriginalMemberValue::Written(ty) = &input.value else { return None };
        Some(self.named_annotation(input.lexical_context, &input.documentation_contexts, ty, queries))
    }
    pub(crate) fn validate_original_assignment_annotations(&self, queries: &mut dyn TypeQueries) {
        for input in self.members.inputs() {
            if matches!(input.origin, OriginalMemberOrigin::AssignmentField(_))
                && let OriginalMemberValue::Written(ty) = &input.value
            { self.named_annotation(input.lexical_context, &input.documentation_contexts, ty, queries); }
        }
    }
    pub(super) fn original_constructor_member_inputs(&self) -> Vec<OriginalMemberInput> {
        let mut inputs = Vec::new();
        for source in self.named_metadata().declarations() {
            let Some(directive) = self.class_definition_directive(source).ok().flatten() else { continue };
            let root = self.expressions().get(self.expressions().key_for_span(directive.initializer));
            let mut constructor = &root.expression;
            while let ExprKind::Parenthesized(inner) = &constructor.kind { constructor = inner; }
            let ExprKind::Table(fields) = &constructor.kind else { continue };
            for field in fields {
                let key = match &field.kind {
                    TableFieldKind::Named(name) => OriginalMemberKey::Fixed(LuaKey::String(name.text.as_bytes().to_vec())),
                    TableFieldKind::Sequence { index } => OriginalMemberKey::Fixed(LuaKey::Integer((*index).try_into().expect("source sequence index fits i64"))),
                    TableFieldKind::Keyed(key) => crate::native_tables::literal_key(key)
                        .map_or_else(|| OriginalMemberKey::Computed(self.expression_key(key)), OriginalMemberKey::Fixed),
                };
                let expression = self.expression_key(&field.value);
                let value = if let Some(ty) = field.annotation.as_ref().and_then(|annotation| annotation.types.first()) {
                    OriginalMemberValue::Written(ty.clone())
                } else if let Some(function) = self.expressions().function_for(expression) {
                    OriginalMemberValue::Function(function)
                } else { OriginalMemberValue::Expression(expression) };
                inputs.push(OriginalMemberInput {
                    owner: source.key, owner_application: None, contextual_owner: None, key,
                    origin: OriginalMemberOrigin::ConstructorField(field.span),
                    value, span: field.span,
                    lexical_context: root.owner.map(NativeTypeContext::Function),
                    documentation_contexts: vec![source.key],
                });
            }
        }
        inputs
    }
    pub(super) fn original_member_header(&self, key: NativeTypeMemberKey) -> Option<TypeMemberHeader> {
        let source = self.members.get(key)?;
        let input = &source.candidates[0];
        Some(TypeMemberHeader {
            owner: TypeMemberOwner::Declared(luanalysis_engine::NativeTypeDeclarationKey { snapshot: input.owner.snapshot, index: input.owner.index }),
            key: match &input.key {
                OriginalMemberKey::Fixed(key) => TypeMemberKeyRecipe::Fixed(key.clone()),
                OriginalMemberKey::Computed(_) => TypeMemberKeyRecipe::Indexer { sparse: true },
            },
            visibility: TypeMemberVisibility::Public, optional: false, readonly: false,
        })
    }
    pub(super) fn original_member_origin(&self, key: NativeTypeMemberKey) -> Option<TableFieldOrigin> {
        Some(if self.members.get(key)?.candidates.iter().any(|input| matches!(input.value, OriginalMemberValue::Written(_))) {
            TableFieldOrigin::Declared
        } else { TableFieldOrigin::Inferred })
    }
    pub(super) fn original_member_owner_application(&self, key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Option<luanalysis_engine::MemberOwnerApplication> {
        let application = self.members.get(key)?.candidates[0].owner_application.as_ref()?;
        Some(luanalysis_engine::MemberOwnerApplication {
            reference: self.named_annotation(application.context, &[], &application.annotation, queries),
            parameters: self.named_outer_parameters(application.context, &application.annotation),
        })
    }
    pub(crate) fn original_member_effective_metadata(&self, key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Option<luanalysis_engine::TypeMemberEffectiveMetadata> {
        let (_, candidates) = self.selected_original_member_candidates(key, queries)?;
        Some(luanalysis_engine::TypeMemberEffectiveMetadata {
            origin: if candidates.iter().any(|input| matches!(input.value, OriginalMemberValue::Written(_))) {
                TableFieldOrigin::Declared
            } else { TableFieldOrigin::Inferred },
            is_assignment: candidates.first().is_some_and(|input| matches!(input.origin, OriginalMemberOrigin::AssignmentField(_))),
        })
    }
    fn original_member_expression_type(&self, key: NativeExpressionKey, queries: &mut dyn TypeQueries) -> Type {
        let expression = &self.expressions().get(key).expression;
        // A member's type belongs to its declaration, not to whichever
        // function's contextual or captured frame demanded it (a union
        // reduction inside another body, for example). Evaluate it in
        // declaration space; function values inside it otherwise look for
        // their lexical parent in an unrelated frame chain.
        let mut result = None;
        queries.with_native_function_frame(luanalysis_engine::NativeCallerFrame::default(), &mut |queries| {
            let mut checker = self.checker_for_declaration_expression(key, queries);
            let ty = checker.expression(expression);
            result = Some((ty, checker.report, checker.incomplete_flow));
        });
        let (ty, report, incomplete) = result.expect("declaration-space member evaluation runs");
        if !incomplete { self.merge_report(report); }
        ty
    }
    fn original_member_candidate_type(&self, member: NativeTypeMemberKey, input: &OriginalMemberInput, queries: &mut dyn TypeQueries) -> Type {
        match &input.value {
            OriginalMemberValue::Written(ty) => {
                if let OriginalMemberOrigin::ConstructorField(span) = input.origin {
                    if let Some(source) = self.named_metadata().field_annotations().iter().find(|source| source.field_span == span) {
                        let pack = crate::native_packs::AnnotationPack::annotation(&source.annotation).lower(&mut |ty| {
                            self.named_annotation(input.lexical_context, &input.documentation_contexts, ty, queries)
                        });
                        return queries.pack_slot(&pack, 0);
                    }
                }
                self.named_annotation(input.lexical_context, &input.documentation_contexts, ty, queries)
            }
            OriginalMemberValue::Expression(key) => {
                // An ordinary assignment to a declared property uses that
                // property's existing contract. Reinferring it from its RHS
                // can demand this same class through a read such as self.x.
                // The original write still checks its actual RHS separately.
                if matches!(input.origin, OriginalMemberOrigin::AssignmentField(_)) {
                    match queries.named_member_context(member) {
                        Ok(Some(expected)) => return expected,
                        Err(luanalysis_engine::NamedTypeError::PendingContext { .. }) => return queries.error_type(),
                        _ => {},
                    }
                }
                // A class's table stored in its own field (`mt.__index = mt`) is the
                // class, as LuaLS reads it. Typing the table there would resolve the
                // class from inside its own definition.
                if let Some(class) = self.own_definition_value(*key, queries) { return class }
                let ty = self.original_member_expression_type(*key, queries);
                // Original inferred Lua fields retain their literal contract.
                // This leaves functions, generics and written contexts alone;
                // it does not impose const/readonly semantics on the member.
                queries.regular_type(&ty)
            }
            OriginalMemberValue::Function(key) => {
                if self.source_role() == luanalysis_project::SourceRole::Declaration {
                    return self.ambient_function_contract(*key, queries).unwrap_or_else(|| queries.error_type());
                }
                let value = Type::NativeFunction(*key);
                let expected = match queries.named_member_context(member) {
                    Ok(expected) => expected,
                    Err(luanalysis_engine::NamedTypeError::PendingContext { .. }) => return queries.error_type(),
                    Err(_) => None,
                };
                if let Some(expected) = expected {
                    if let OriginalMemberOrigin::Method(_) = input.origin {
                        let target = self.original_method_target(*key).expect("original named member definition");
                        // A schema demand describes this declaration, not a
                        // runtime closure created in the caller's active facet.
                        return queries.contextual_function_value(&value, self.expression_key(target), &expected, luanalysis_engine::NativeCheckMode::empty());
                    }
                    let expression = self.expressions().key_for_span(self.function(*key).span);
                    self.source_check_expression(expression, Some(&expected), luanalysis_engine::NativeCheckMode::empty(), queries)
                } else { value }
            },
        }
    }
    pub(super) fn original_member_type(&self, key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Option<Type> {
        let (activation, candidates) = self.selected_original_member_candidates(key, queries)?;
        if activation != luanalysis_engine::TypeMemberActivation::Active {
            // The native owner query taints pending demands. Neither an
            // unresolved nor an inactive group may contribute a source type.
            return Some(queries.error_type());
        }
        if candidates.len() == 1 { return Some(self.original_member_candidate_type(key, candidates[0], queries)); }
        let OriginalMemberKey::Fixed(member_key) = &candidates[0].key else { unreachable!("separate computed source addresses") };
        let mut table = TableType { fresh: true, ..Default::default() };
        for input in candidates {
            let value = self.original_member_candidate_type(key, input, queries);
            table.fields.push(if matches!(input.value, OriginalMemberValue::Written(_)) {
                luanalysis_engine::TableField::declared(member_key.clone(), value)
            } else { luanalysis_engine::TableField::new(member_key.clone(), value) });
        }
        let table = queries.table_type(&table).expect("original fixed constructor candidates have valid Lua keys");
        Some(queries.table_lookup(&table, &TableKey::Literal(member_key.clone())).expect("original candidate key exists"))
    }
    pub(super) fn original_member_indexer_key(&self, key: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Option<Type> {
        let OriginalMemberKey::Computed(expression) = self.members.get(key)?.candidates[0].key else { return None };
        Some(self.original_member_expression_type(expression, queries))
    }
    pub(super) fn original_member_dependencies(&self, owner: NativeNamedKey, used: &mut HashSet<NativeTypeParameterKey>) {
        for key in self.members.members(owner) {
            for input in &self.members.get(key).unwrap().candidates {
                if let OriginalMemberKey::Computed(key) = input.key {
                    self.value_type_dependencies(&self.expressions().get(key).expression, used, &mut HashSet::default(), &mut HashSet::default());
                }
                match &input.value {
                    OriginalMemberValue::Written(ty) => self.annotation_parameter_dependencies(input.lexical_context, ty, used),
                    OriginalMemberValue::Expression(key) => self.value_type_dependencies(&self.expressions().get(*key).expression, used, &mut HashSet::default(), &mut HashSet::default()),
                    OriginalMemberValue::Function(key) => self.function_type_dependencies(*key, used, &mut HashSet::default(), &mut HashSet::default()),
                }
            }
        }
    }
    pub(super) fn original_member_owner_scopes(&self) -> Vec<(NativeTypeContext, NativeNamedKey)> {
        let mut scopes = Vec::new();
        for input in self.members.inputs() {
            if input.contextual_owner.is_some() || input.owner_application.is_some() { continue; }
            if let OriginalMemberValue::Function(function) = input.value { scopes.push((NativeTypeContext::Function(function), input.owner)); }
            if matches!(input.origin, OriginalMemberOrigin::AssignmentField(_))
                && let OriginalMemberValue::Written(ty) = &input.value
            {
                let mut ty = ty.value_syntax();
                while let luanalysis_syntax::TypeExprKind::Parenthesized(inner) = &ty.kind { ty = inner.value_syntax(); }
                if let luanalysis_syntax::TypeExprKind::Function(function) = &ty.kind
                    && let Some(key) = self.signature_key_for(function.span)
                { scopes.push((NativeTypeContext::Signature(key), input.owner)); }
            }
            if let OriginalMemberOrigin::ConstructorField(span) = input.origin {
                if let Some(source) = self.named_metadata().field_annotations().iter().find(|source| source.field_span == span) {
                    if let Some(function) = self.expressions().function_for(self.expressions().key_for_span(source.value_span)) {
                        scopes.push((NativeTypeContext::Function(function), input.owner));
                    }
                    for ty in &source.annotation.types {
                        let mut ty = ty.value_syntax();
                        while let luanalysis_syntax::TypeExprKind::Parenthesized(inner) = &ty.kind { ty = inner.value_syntax(); }
                        if let luanalysis_syntax::TypeExprKind::Function(function) = &ty.kind {
                            if let Some(key) = self.signature_key_for(function.span) { scopes.push((NativeTypeContext::Signature(key), input.owner)); }
                        }
                    }
                }
            }
        }
        scopes.sort_by_key(|(context, owner)| (format!("{context:?}"), owner.index));
        scopes.dedup(); scopes
    }
}
