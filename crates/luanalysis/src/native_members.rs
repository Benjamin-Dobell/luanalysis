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
#[derive(Clone, Debug)]
pub(super) struct OriginalMemberInput {
    pub owner: NativeNamedKey,
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
    /// Duplicate constructor stores remain separate original candidates. The
    /// native table constructor combines them using its Lua collision policy.
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
            }) {
                // The constructor defines the property contract. A subsequent
                // method declaration is a checked write to that same member.
                if matches!(input.origin, OriginalMemberOrigin::ConstructorField(_))
                    && matches!(previous.candidates[0].origin, OriginalMemberOrigin::ConstructorField(_))
                {
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
    pub(super) fn sources(&self) -> &[OriginalMemberSource] { &self.sources }
    pub(super) fn namespace_end(&self) -> u32 { self.base.checked_add(self.sources.len().try_into().expect("original member count fits u32")).expect("original member namespace fits u32") }
    pub(super) fn inputs(&self) -> &[OriginalMemberInput] { &self.inputs }
}

use crate::native_functions::NativeRegistry;
use luanalysis_engine::{TableFieldOrigin, TableKey, TableType, Type, TypeMemberHeader, TypeMemberKeyRecipe, TypeMemberOwner, TypeMemberVisibility, TypeQueries, NativeTypeParameterKey};
use luanalysis_syntax::{ExprKind, TableFieldKind};
use std::collections::HashSet;

impl NativeRegistry {
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
                    owner: source.key, key,
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
    fn original_member_expression_type(&self, key: NativeExpressionKey, queries: &mut dyn TypeQueries) -> Type {
        let expression = &self.expressions().get(key).expression;
        let (ty, report, incomplete) = {
            let mut checker = self.checker_for_declaration_expression(key, queries);
            let ty = checker.expression(expression);
            (ty, checker.report, checker.incomplete_flow)
        };
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
                let ty = self.original_member_expression_type(*key, queries);
                queries.widen(&ty)
            }
            OriginalMemberValue::Function(key) => {
                if self.source_role() == luanalysis_project::SourceRole::Declaration {
                    return self.ambient_function_contract(*key, queries).unwrap_or_else(|| queries.error_type());
                }
                let value = Type::NativeFunction(*key);
                if let Ok(Some(expected)) = queries.named_member_context(member) {
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
        let source = self.members.get(key)?;
        if source.candidates.len() == 1 { return Some(self.original_member_candidate_type(key, &source.candidates[0], queries)); }
        let OriginalMemberKey::Fixed(member_key) = &source.candidates[0].key else { unreachable!("separate computed source addresses") };
        let mut table = TableType { fresh: true, ..Default::default() };
        for input in &source.candidates {
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
                    self.value_type_dependencies(&self.expressions().get(key).expression, used, &mut HashSet::new(), &mut HashSet::new());
                }
                match &input.value {
                    OriginalMemberValue::Written(ty) => self.annotation_parameter_dependencies(input.lexical_context, ty, used),
                    OriginalMemberValue::Expression(key) => self.value_type_dependencies(&self.expressions().get(*key).expression, used, &mut HashSet::new(), &mut HashSet::new()),
                    OriginalMemberValue::Function(key) => self.function_type_dependencies(*key, used, &mut HashSet::new(), &mut HashSet::new()),
                }
            }
        }
    }
    pub(super) fn original_member_owner_scopes(&self) -> Vec<(NativeTypeContext, NativeNamedKey)> {
        let mut scopes = Vec::new();
        for input in self.members.inputs() {
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
