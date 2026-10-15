//! Immutable original named-function/assignment origins. This indexes ownership
//! evidence; receiver typing, substitution and relations remain native queries.
use crate::bind::{BindingId, BoundFile};
use crate::native_named::{NativeNamed, NativeNamedKey, RuntimeAttachmentKind};
use crate::native_members::{OriginalMemberInput, OriginalMemberKey, OriginalMemberOrigin, OriginalMemberValue};
use luanalysis_engine::{LuaKey, NativeFunctionKey, Type, TypeQueries, NativeCheckMode};
use luanalysis_syntax::{
    Expr, ExprKind, File, Function, NamedRecordKind, NamedTypeDeclarationKind, Span, Stmt, StmtKind, TypeExpr,
};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub(super) struct MethodSource {
    pub function: NativeFunctionKey,
    pub target: Expr,
    pub receiver: Expr,
    pub member: LuaKey,
    pub colon: Option<Span>,
    /// Named member declaration syntax, distinct from assigning a function value.
    pub declaration: bool,
    pub implicit_receiver: Option<BindingId>,
    /// Exact original attached class binding, never a matching runtime name.
    pub class_owner: Option<NativeNamedKey>,
    pub contextual_owner: Option<NativeFunctionKey>,
    /// Original global attachment, used only for declaration-library recipes.
    pub declaration_owner: Option<NativeNamedKey>,
}

pub(super) struct NativeMethods {
    snapshot: u64,
    sources: Vec<Option<MethodSource>>,
    fields: Vec<AssignmentFieldSource>,
}
struct AssignmentFieldSource {
    owner: NativeNamedKey,
    contextual_owner: Option<NativeFunctionKey>,
    key: LuaKey,
    target: Span,
    value: Span,
    annotation: Option<TypeExpr>,
}
impl NativeMethods {
    pub(super) fn new(
        snapshot: u64,
        file: &File,
        functions: &[Function],
        spans: &HashMap<(usize, usize), u32>,
        bound: &BoundFile,
        named: &NativeNamed,
        types: &crate::native_types::NativeTypes,
        metatables: &super::metatables::NativeMetatables,
    ) -> Self {
        let mut owners: HashMap<BindingId, Vec<NativeNamedKey>> = HashMap::new();
        let mut contextual_owners = HashMap::new();
        let mut declaration_owners: HashMap<String, Vec<NativeNamedKey>> = HashMap::new();
        for declaration in named.declarations() {
            if !matches!(
                declaration.syntax.kind,
                NamedTypeDeclarationKind::Record {
                    flavor: NamedRecordKind::Class,
                    ..
                }
            ) {
                continue;
            }
            let Some(attachment) = declaration
                .syntax
                .attachment
                .and_then(|span| named.attachment(span))
            else {
                continue;
            };
            if attachment.declarations.len() == 1
                && named.candidates(&declaration.syntax.name.text).len() == 1
            {
                match &attachment.kind {
                    RuntimeAttachmentKind::Local { bindings } => {
                        if let [binding] = bindings.as_slice() {
                            owners.entry(*binding).or_default().push(declaration.key);
                        }
                    }
                    RuntimeAttachmentKind::Assignment { targets } => {
                        if let [Expr { kind: ExprKind::Name(name), .. }] = targets.as_slice()
                            && bound.binding_at(name.span.start).is_none()
                            && !matches!(name.text.as_str(), "_G" | "_ENV")
                        {
                            declaration_owners.entry(name.text.clone()).or_default().push(declaration.key);
                        }
                    }
                    _ => {}
                }
            }
        }
        // Closure-based Lua constructors declare their instance through an
        // original local @type contract, then install functions on that value.
        // These definitions belong to the written class, just as functions on
        // an attached @class local do. Do not infer an owner from a variable's
        // spelling, an arbitrary alias, a parameter, or a declaration without
        // an initializer.
        let assertions: HashMap<_, _> = file.expression_assertions.iter().filter_map(|assertion| {
            if assertion.kind != luanalysis_syntax::ExpressionAssertionKind::Type { return None; }
            let [ty] = assertion.annotation.types.as_slice() else { return None };
            Some(((assertion.expression.start, assertion.expression.end), ty))
        }).collect();
        collect_written_local_owners(&file.statements, None, &assertions, bound, named, types, metatables, &mut owners, &mut contextual_owners);
        for function in functions {
            let context = crate::native_named::NativeTypeContext::Function(NativeFunctionKey {
                snapshot, index: spans[&(function.span.start, function.span.end)],
            });
            collect_written_local_owners(&function.body, Some(context), &assertions, bound, named, types, metatables, &mut owners, &mut contextual_owners);
        }
        let mut methods = Self {
            snapshot,
            sources: vec![None; functions.len()],
            fields: Vec::new(),
        };
        methods.statements(&file.statements, spans, bound, &owners, &declaration_owners, &contextual_owners);
        // Each original function body is indexed once; expression nesting was
        // already traversed by the original function identity collector.
        for function in functions {
            methods.statements(&function.body, spans, bound, &owners, &declaration_owners, &contextual_owners);
        }
        methods
    }
    pub(super) fn get(&self, key: NativeFunctionKey) -> Option<&MethodSource> {
        (key.snapshot == self.snapshot)
            .then(|| self.sources.get(key.index as usize))
            .flatten()
            .and_then(Option::as_ref)
    }
    pub(super) fn sources(&self) -> impl Iterator<Item = &MethodSource> {
        self.sources.iter().filter_map(Option::as_ref)
    }
    pub(super) fn original_members(&self) -> Vec<OriginalMemberInput> {
        self.sources().filter_map(|method| {
            let owner = method.class_owner?;
            Some(OriginalMemberInput {
                owner,
                contextual_owner: method.contextual_owner,
                key: OriginalMemberKey::Fixed(method.member.clone()),
                origin: OriginalMemberOrigin::Method(method.function),
                value: OriginalMemberValue::Function(method.function),
                span: method.target.span,
                lexical_context: Some(crate::native_named::NativeTypeContext::Function(method.function)),
                documentation_contexts: vec![owner],
            })
        }).collect()
    }
    pub(super) fn original_assignment_members(&self, expressions: &super::NativeExpressions) -> Vec<OriginalMemberInput> {
        self.fields.iter().map(|field| {
            let expression = expressions.key_for_span(field.value);
            OriginalMemberInput {
                owner: field.owner, contextual_owner: field.contextual_owner,
                key: OriginalMemberKey::Fixed(field.key.clone()),
                origin: OriginalMemberOrigin::AssignmentField(field.target),
                value: field.annotation.as_ref().map_or(OriginalMemberValue::Expression(expression), |ty| OriginalMemberValue::Written(ty.clone())),
                span: field.target,
                lexical_context: expressions.get(expression).owner.map(crate::native_named::NativeTypeContext::Function),
                documentation_contexts: vec![field.owner],
            }
        }).collect()
    }
    fn statements(
        &mut self,
        statements: &[Stmt],
        spans: &HashMap<(usize, usize), u32>,
        bound: &BoundFile,
        owners: &HashMap<BindingId, Vec<NativeNamedKey>>,
        declaration_owners: &HashMap<String, Vec<NativeNamedKey>>,
        contextual_owners: &HashMap<BindingId, NativeFunctionKey>,
    ) {
        for statement in statements {
            match &statement.kind {
                StmtKind::FunctionDeclaration {
                    target,
                    function,
                    method_span,
                } => self.record(target, function, *method_span, true, spans, bound, owners, declaration_owners, contextual_owners),
                StmtKind::Assign {
                    targets, values, annotation,
                } if targets.len() == values.len() => {
                    for (index, (target, value)) in targets.iter().zip(values).enumerate() {
                        if let Some(function) = function_literal(value) {
                            self.record(target, function, None, false, spans, bound, owners, declaration_owners, contextual_owners);
                        } else if let Some((receiver, key)) = fixed_member_target(target)
                            && let Some(owner) = local_class_owner(receiver, bound, owners)
                        {
                            self.fields.push(AssignmentFieldSource {
                                owner, contextual_owner: receiver_binding(receiver, bound).and_then(|binding| contextual_owners.get(&binding).copied()), key, target: target.span, value: value.span,
                                annotation: annotation.as_ref().and_then(|annotation| annotation.types.get(index)).cloned(),
                            });
                        }
                    }
                }
                StmtKind::Block { body, .. }
                | StmtKind::While { body, .. }
                | StmtKind::Repeat { body, .. }
                | StmtKind::NumericFor { body, .. }
                | StmtKind::GenericFor { body, .. } => self.statements(body, spans, bound, owners, declaration_owners, contextual_owners),
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.statements(&branch.body, spans, bound, owners, declaration_owners, contextual_owners);
                    }
                    if let Some(branch) = else_branch {
                        self.statements(&branch.body, spans, bound, owners, declaration_owners, contextual_owners);
                    }
                }
                _ => {}
            }
        }
    }
    fn record(
        &mut self,
        target: &Expr,
        function: &Function,
        colon: Option<Span>,
        declaration: bool,
        spans: &HashMap<(usize, usize), u32>,
        bound: &BoundFile,
        owners: &HashMap<BindingId, Vec<NativeNamedKey>>,
        declaration_owners: &HashMap<String, Vec<NativeNamedKey>>,
        contextual_owners: &HashMap<BindingId, NativeFunctionKey>,
    ) {
        let Some((receiver, member)) = fixed_member_target(target) else { return };
        let index = *spans
            .get(&(function.span.start, function.span.end))
            .expect("original method function was allocated first");
        let key = NativeFunctionKey {
            snapshot: self.snapshot,
            index,
        };
        let class_owner = local_class_owner(receiver, bound, owners);
        let declaration_owner = match &receiver.kind {
            ExprKind::Name(name) if bound.binding_at(name.span.start).is_none() =>
                declaration_owners.get(&name.text).and_then(|owners| match owners.as_slice() {
                    [owner] => Some(*owner), _ => None,
                }),
            _ => None,
        };
        self.sources[index as usize] = Some(MethodSource {
            function: key,
            target: target.clone(),
            receiver: receiver.clone(),
            member,
            colon,
            declaration,
            implicit_receiver: bound.implicit_receiver(function.span),
            class_owner,
            contextual_owner: receiver_binding(receiver, bound).and_then(|binding| contextual_owners.get(&binding).copied()),
            declaration_owner,
        });
    }
}
fn collect_written_local_owners(
    statements: &[Stmt],
    context: Option<crate::native_named::NativeTypeContext>,
    assertions: &HashMap<(usize, usize), &TypeExpr>,
    bound: &BoundFile,
    named: &NativeNamed,
    types: &crate::native_types::NativeTypes,
    metatables: &super::metatables::NativeMetatables,
    owners: &mut HashMap<BindingId, Vec<NativeNamedKey>>,
    contextual_owners: &mut HashMap<BindingId, NativeFunctionKey>,
) {
    for statement in statements {
        match &statement.kind {
            StmtKind::Local { names, values, annotation } => {
                for (index, (name, value)) in names.iter().zip(values).enumerate() {
                    let written = annotation.as_ref().and_then(|annotation| annotation.types.get(index))
                        .or_else(|| assertions.get(&(value.span.start, value.span.end)).copied());
                    let Some(ty) = written else { continue };
                    let mut ty = ty.value_syntax();
                    while let luanalysis_syntax::TypeExprKind::Parenthesized(inner) = &ty.kind {
                        ty = inner.value_syntax();
                    }
                    let luanalysis_syntax::TypeExprKind::Named(type_name) = &ty.kind else { continue };
                    if type_name == "self" {
                        let Some(crate::native_named::NativeTypeContext::Function(function)) = context else { continue };
                        // Explicit generic parameters called self retain their
                        // lexical meaning; this indexes only contextual self.
                        match types.resolve_name_in_context(context, type_name) {
                            crate::native_types::TypeNameResolution::Parameter(key) if types.parameter(key).semantic_self.is_none() => continue,
                            crate::native_types::TypeNameResolution::Ambiguous(_) => continue,
                            _ => {},
                        }
                        let Some((scope, source)) = metatables.context_scope(function, types) else { continue };
                        let Some(owner) = source.candidate_owner else { continue };
                        let binding = bound.binding_at(name.span.start).expect("original initialized contextual local");
                        owners.entry(binding).or_default().push(owner);
                        contextual_owners.insert(binding, scope);
                        continue;
                    }
                    if types.resolve_name_in_context(context, type_name) != crate::native_types::TypeNameResolution::Missing { continue; }
                    let [owner] = named.candidates(type_name) else { continue };
                    if !named.declaration(*owner).is_some_and(|declaration| matches!(
                        declaration.syntax.kind,
                        NamedTypeDeclarationKind::Record { flavor: NamedRecordKind::Class, .. }
                    )) { continue; }
                    let binding = bound.binding_at(name.span.start).expect("original initialized local");
                    let candidates = owners.entry(binding).or_default();
                    if !candidates.contains(owner) { candidates.push(*owner); }
                }
            }
            StmtKind::Block { body, .. } | StmtKind::While { body, .. }
            | StmtKind::Repeat { body, .. } | StmtKind::NumericFor { body, .. }
            | StmtKind::GenericFor { body, .. } => collect_written_local_owners(body, context, assertions, bound, named, types, metatables, owners, contextual_owners),
            StmtKind::If { branches, else_branch } => {
                for branch in branches { collect_written_local_owners(&branch.body, context, assertions, bound, named, types, metatables, owners, contextual_owners); }
                if let Some(branch) = else_branch { collect_written_local_owners(&branch.body, context, assertions, bound, named, types, metatables, owners, contextual_owners); }
            }
            _ => {}
        }
    }
}
fn fixed_member_target(target: &Expr) -> Option<(&Expr, LuaKey)> {
    match &target.kind {
        ExprKind::Field { base, name } => Some((base, LuaKey::String(name.text.as_bytes().to_vec()))),
        ExprKind::Index { base, index } => Some((base, crate::native_tables::literal_key(index)?)),
        _ => None,
    }
}
fn local_class_owner(receiver: &Expr, bound: &BoundFile, owners: &HashMap<BindingId, Vec<NativeNamedKey>>) -> Option<NativeNamedKey> {
    let [owner] = owners.get(&receiver_binding(receiver, bound)?)?.as_slice() else { return None };
    Some(*owner)
}
fn receiver_binding(receiver: &Expr, bound: &BoundFile) -> Option<BindingId> {
    let ExprKind::Name(name) = &receiver.kind else { return None };
    bound.binding_at(name.span.start)
}
fn function_literal(value: &Expr) -> Option<&Function> {
    match &value.kind {
        ExprKind::Function(function) => Some(function),
        ExprKind::Parenthesized(value) => function_literal(value),
        _ => None,
    }
}

impl super::NativeRegistry {
    pub(crate) fn has_polymorphic_method_owner(&self, key: NativeFunctionKey) -> bool {
        self.method_owner(key).is_some() || self.methods.get(key).is_some_and(|method| method.contextual_owner.is_some())
    }
    pub(crate) fn original_method_target(&self, key: NativeFunctionKey) -> Option<&Expr> {
        Some(&self.methods.get(key)?.target)
    }
    pub(crate) fn method_owner(&self, key: NativeFunctionKey) -> Option<NativeNamedKey> {
        let method = self.methods.get(key)?;
        if method.contextual_owner.is_some() { return None; }
        method.class_owner.or_else(|| (self.source_role() == luanalysis_project::SourceRole::Declaration)
            .then_some(method.declaration_owner).flatten())
    }
    pub(crate) fn implicit_receiver(&self, function: &Function) -> Option<BindingId> {
        self.bound.implicit_receiver(function.span)
    }

    pub(crate) fn default_receiver_type(&self, function: &Function, queries: &mut dyn TypeQueries) -> Option<Type> {
        let key = self.key_for(function);
        let method = self.methods.get(key)?;
        if method.contextual_owner.is_some() {
            match self.contextual_self_type(key, queries) {
                luanalysis_engine::ContextualTypeOwner::Owner(ty) => return Some(ty),
                luanalysis_engine::ContextualTypeOwner::Pending => return Some(queries.error_type()),
                luanalysis_engine::ContextualTypeOwner::Absent => {},
            }
        }
        if let Some(owner) = self.method_owner(key) {
            return self.types.self_parameter(owner).map(Type::NativeTypeParameter);
        }
        if self.source_role() == luanalysis_project::SourceRole::Declaration {
            return None;
        }
        // A plain Lua receiver is read at its original pre-write creation
        // point. The native flow walker remains the only value-flow solver.
        let graph = self.flow.graph_for_function(key, None)?;
        let creation = self.flow.graph_creation(graph)?;
        let mut checker = self.checker_at_flow(creation.input, queries);
        checker.check_mode |= NativeCheckMode::INFERENTIAL;
        let receiver = checker.expression(&method.receiver);
        Some(receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_functions::NativeRegistry;
    fn registry(source: &str) -> NativeRegistry {
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        NativeRegistry::new(file, bound)
    }
    #[test]
    fn method_owners_follow_original_attached_binding_not_spelling() {
        let native = registry(
            "---@class DifferentName\nlocal object={}\nfunction object:method(value) return self end",
        );
        let method = native.methods.sources().next().unwrap();
        assert!(method.colon.is_some());
        assert!(method.implicit_receiver.is_some());
        assert!(method.class_owner.is_some());
        assert_eq!(native.function(method.function).params.len(), 1);
    }
    #[test]
    fn dot_lambda_and_colon_origins_keep_independent_actual_function_keys() {
        let native = registry(
            "---@class C\nlocal object={}\nfunction object.dot() end\nobject.lambda=function() end\nfunction object:colon() end",
        );
        let methods: Vec<_> = native.methods.sources().collect();
        assert_eq!(methods.len(), 3);
        assert!(
            methods
                .iter()
                .all(|method| method.class_owner == methods[0].class_owner)
        );
        assert_eq!(
            methods
                .iter()
                .filter(|method| method.implicit_receiver.is_some())
                .count(),
            1
        );
        assert_ne!(methods[0].function, methods[1].function);
    }
    #[test]
    fn lexical_shadows_aliases_and_instance_extensions_do_not_invent_class_ownership() {
        let native = registry(
            "---@class C\nlocal object={}\nlocal alias=object\nfunction alias:method() end\ndo local object={} function object:method() end end\n---@type C\nlocal instance\nfunction instance:method() end",
        );
        assert_eq!(native.methods.sources().count(), 3);
        assert!(
            native
                .methods
                .sources()
                .all(|method| method.class_owner.is_none())
        );
    }
    #[test]
    fn semantic_self_keeps_written_arity_and_generic_shadowing_separate() {
        use crate::native_named::NativeTypeContext;
        use crate::native_types::TypeNameResolution;
        let native = registry("---@generic T\nlocal function outer()\n---@class C<U>\nlocal object={}\n---@generic T\n---@param value T\n---@return self\nfunction object:method(value) return self end\nreturn object\nend");
        let method = native.methods.sources().next().unwrap();
        let owner = method.class_owner.unwrap();
        let own = native.types.own_parameters(method.function);
        let class = native.types.own_parameters_in_context(NativeTypeContext::NamedType(owner));
        let self_key = native.types.self_parameter(owner).unwrap();
        assert_eq!(own.len(), 1);
        assert_eq!(class.len(), 1);
        assert!(!class.contains(&self_key));
        assert_eq!(native.function(method.function).params.len(), 1);
        assert_eq!(native.types.resolve_name_in_context(Some(NativeTypeContext::Function(method.function)), "T"), TypeNameResolution::Parameter(own[0]));
        assert_eq!(native.types.resolve_name_in_context(Some(NativeTypeContext::Function(method.function)), "U"), TypeNameResolution::Parameter(class[0]));
        assert_eq!(native.types.resolve_name_in_context(Some(NativeTypeContext::Function(method.function)), "self"), TypeNameResolution::Parameter(self_key));
        assert_ne!(own[0], class[0]);
        assert_eq!(native.types.parameter(self_key).semantic_self, Some(owner));
        assert!(native.types.parameter(self_key).constraint.is_none());
    }
    #[test]
    fn member_recipes_retain_original_function_and_target_identities() {
        use crate::native_members::{OriginalMemberOrigin, OriginalMemberValue};
        let native = registry("---@class C\nlocal object={}\nfunction object:method() end\nobject.lambda=function() end\nlocal alias=object\nfunction alias:ignored() end");
        let members = native.methods.original_members();
        assert_eq!(members.len(), 2);
        for member in members {
            let OriginalMemberOrigin::Method(function) = member.origin else { panic!("method origin") };
            assert!(matches!(member.value, OriginalMemberValue::Function(key) if key==function));
            let source = native.methods.get(function).unwrap();
            assert_eq!(member.span, source.target.span);
            assert_eq!(member.documentation_contexts, vec![member.owner]);
            assert!(source.class_owner.is_some());
        }
    }
}
