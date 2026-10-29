//! Original table-field closure contexts. These are candidates only: the
//! checker must prove the actual standard callee at the frozen call input.
use super::NativeRegistry;
use crate::bind::BoundFile;
use crate::native_expressions::NativeExpressions;
use crate::native_named::{NativeNamed, NativeNamedKey, NativeTypeContext};
use luanalysis_engine::{
    ContextualTypeOwner, LuaKey, NativeExpressionKey, NativeFlowKey, NativeFunctionKey,
    NativeFunctionCheckState, NativeIntrinsicKind, NativeTypeMemberKey, TypeMemberActivation, TypeQueries,
};
use luanalysis_syntax::{
    Expr, ExprKind, NamedRecordKind, NamedTypeDeclarationKind, TableFieldKind, TypeExpr,
    TypeExprKind,
};
use crate::collections::HashMap;

#[derive(Clone)]
pub(super) struct MetatableContextSource {
    pub call: NativeExpressionKey,
    pub callee: NativeExpressionKey,
    pub target: NativeExpressionKey,
    /// Only an unambiguous original literal function can supply a written
    /// result contract without executing or inferring the constructor body.
    pub call_function: Option<NativeFunctionKey>,
    pub candidate_owner: Option<NativeNamedKey>,
}

pub(super) struct NativeMetatables {
    calls: Vec<MetatableContextSource>,
    scopes: HashMap<NativeFunctionKey, usize>,
}

impl NativeMetatables {
    pub(super) fn new(
        expressions: &NativeExpressions,
        functions: &[luanalysis_syntax::SharedFunction],
        bound: &BoundFile,
        named: &NativeNamed,
    ) -> Self {
        let mut result = Self {
            calls: Vec::new(),
            scopes: HashMap::default(),
        };
        for source in expressions.original_sources() {
            let ExprKind::Call { callee, args } = &source.expression.kind else {
                continue;
            };
            let [target, meta] = args.as_slice() else {
                continue;
            };
            let table = unparenthesized(meta);
            let ExprKind::Table(fields) = &table.kind else {
                continue;
            };
            let mut call_function = None;
            let mut uncertain = false;
            let mut scopes = Vec::new();
            for field in fields {
                let function = expressions.function_for(expressions.key_for_span(field.value.span));
                if let Some(function) = function {
                    scopes.push(function);
                }
                let key = match &field.kind {
                    TableFieldKind::Named(name) => {
                        Some(LuaKey::String(name.text.as_bytes().to_vec()))
                    }
                    TableFieldKind::Keyed(key) => crate::native_tables::literal_key(key),
                    TableFieldKind::Sequence { index } => Some(LuaKey::Integer(
                        (*index).try_into().expect("original table index"),
                    )),
                };
                match key {
                    Some(LuaKey::String(name)) if name == b"__call" => call_function = function,
                    None => uncertain = true,
                    _ => {}
                }
            }
            if scopes.is_empty() {
                continue;
            }
            if uncertain {
                call_function = None;
            }
            let candidate_owner = call_function.and_then(|key| {
                let annotation = functions.get(key.index as usize)?.annotations.returns.as_ref()?;
                let first = annotation.types.first()?;
                candidate_class(named, first)
            }).or_else(|| {
                let ExprKind::Name(name) = &unparenthesized(target).kind else { return None };
                let binding = bound.binding_at(name.span.start)?;
                let mut owners = named.declarations().iter().filter(|declaration| {
                    matches!(declaration.syntax.kind, NamedTypeDeclarationKind::Record { flavor: NamedRecordKind::Class, .. })
                        && declaration.syntax.attachment.and_then(|span| named.attachment(span)).is_some_and(|attachment| {
                            matches!(&attachment.kind, crate::native_named::RuntimeAttachmentKind::Local { bindings } if bindings.as_slice() == [binding])
                        })
                });
                let owner = owners.next()?.key;
                owners.next().is_none().then_some(owner)
            });
            let index = result.calls.len();
            result.calls.push(MetatableContextSource {
                call: expressions.key_for_span(source.expression.span),
                callee: expressions.key_for_span(callee.span),
                target: expressions.key_for_span(target.span),
                call_function,
                candidate_owner,
            });
            for function in scopes {
                assert!(
                    result.scopes.insert(function, index).is_none(),
                    "an original closure has one direct literal parent"
                );
            }
        }
        result
    }

    pub(super) fn direct_scope(
        &self,
        function: NativeFunctionKey,
    ) -> Option<&MetatableContextSource> {
        self.calls.get(*self.scopes.get(&function)?)
    }

    pub(super) fn context_scope(
        &self,
        mut function: NativeFunctionKey,
        types: &crate::native_types::NativeTypes,
    ) -> Option<(NativeFunctionKey, &MetatableContextSource)> {
        loop {
            if let Some(source) = self.direct_scope(function) {
                return Some((function, source));
            }
            function = types.lexical_function_parent(function)?;
        }
    }
}

fn unparenthesized(mut expression: &Expr) -> &Expr {
    while let ExprKind::Parenthesized(inner) = &expression.kind {
        expression = inner;
    }
    expression
}

fn candidate_class(named: &NativeNamed, ty: &TypeExpr) -> Option<NativeNamedKey> {
    let mut ty = ty.value_syntax();
    while let TypeExprKind::Parenthesized(inner) = &ty.kind {
        ty = inner.value_syntax();
    }
    let TypeExprKind::Named(name) = &ty.kind else {
        return None;
    };
    let owner = named.sole_owner(name)?;
    matches!(
        named.declaration(owner)?.syntax.kind,
        NamedTypeDeclarationKind::Record {
            flavor: NamedRecordKind::Class,
            ..
        }
    )
    .then_some(owner)
}

impl NativeRegistry {
    pub(super) fn metatable_context_scope(
        &self,
        function: NativeFunctionKey,
    ) -> Option<(NativeFunctionKey, &MetatableContextSource)> {
        self.metatables.context_scope(function, &self.types)
    }

    pub(crate) fn contextual_owner_at_creation(
        &self,
        scope: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> ContextualTypeOwner {
        let Some(source) = self.metatables.direct_scope(scope) else {
            return ContextualTypeOwner::Absent;
        };
        let call = self.expressions.get(source.call);
        // A foreign caller's active facet is not this declaration's creation
        // frame. Select only the exact original lexical parent capability.
        let mut captured = None;
        if let Some(parent) = call.owner {
            let mut current = queries.current_captured_function();
            let mut visited = crate::collections::HashSet::default();
            while let Some(frame) = current {
                if !visited.insert(frame) {
                    break;
                }
                let value = queries.captured_function_value(&frame);
                if queries.native_function_key(&value) == Some(parent) {
                    captured = Some(frame);
                    break;
                }
                current = queries.captured_function_parent(&frame);
            }
        }
        let contextual = captured.and_then(|frame| queries.captured_function_context(&frame));
        if let Some(captured) = captured {
            self.retain_capture_ancestry(&captured, queries);
        }
        let region = call.owner.map_or(0, |owner| {
            self.flow
                .graph_for_function(owner, None)
                .expect("original call graph")
                .region
        });
        let graph = self
            .flow
            .graph_with_contexts(region, contextual.as_ref(), captured.as_ref());
        let evaluation = self
            .flow
            .evaluation_for_expression(&call.expression, graph)
            .expect("original call evaluation");
        let input = self.flow.flow_key(evaluation.input, graph.context);
        self.contextual_owner_query(scope, input, queries)
    }

    fn contextual_owner_query(
        &self,
        scope: NativeFunctionKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> ContextualTypeOwner {
        let result = queries.contextual_type_owner(scope, input);
        match result {
            ContextualTypeOwner::Pending => {
                self.pending_contextual_owners
                    .borrow_mut()
                    .insert((scope, input));
                self.pending_owner_observations.set(
                    self.pending_owner_observations
                        .get()
                        .checked_add(1)
                        .expect("source owner observation count"),
                );
            }
            _ => {
                self.pending_contextual_owners
                    .borrow_mut()
                    .remove(&(scope, input));
            }
        }
        result
    }

    pub(super) fn source_contextual_type_owner(
        &self,
        scope: NativeFunctionKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> ContextualTypeOwner {
        let Some(source) = self.metatables.direct_scope(scope) else {
            return ContextualTypeOwner::Absent;
        };
        if !self.flow.contains_graph(input.graph) {
            return ContextualTypeOwner::Absent;
        }
        let Some(evaluation) = self
            .flow
            .evaluation_for_expression(&self.expressions.get(source.call).expression, input.graph)
        else {
            return ContextualTypeOwner::Absent;
        };
        if self.flow.flow_key(evaluation.input, input.graph.context) != input {
            return ContextualTypeOwner::Absent;
        }
        let mut checker = self.checker_at_flow(input, queries);
        checker.runtime_value = true;
        let callee = &self.expressions.get(source.callee).expression;
        let callee_evaluation = self
            .flow
            .evaluation_for_expression(callee, input.graph)
            .expect("the original callee has its frozen evaluation");
        let adapter = crate::native_flow::FlowAdapter {
            sources: &self.flow,
            values: self,
        };
        // This classifies contextual constructor metadata only. Raw heap,
        // callable-table and metatable-absence workers use strict provenance.
        match checker.engine.flow_standard_operation(
            &adapter,
            self.flow.evaluation_value(callee_evaluation.id),
            self.flow
                .flow_key(callee_evaluation.input, input.graph.context),
        ) {
            luanalysis_engine::NativeStandardOperation::Standard(site)
                if site.kind == NativeIntrinsicKind::SetMetatable => {}
            luanalysis_engine::NativeStandardOperation::Standard(_) => {
                return ContextualTypeOwner::Absent;
            }
            luanalysis_engine::NativeStandardOperation::Pending => {
                return ContextualTypeOwner::Pending;
            }
            luanalysis_engine::NativeStandardOperation::Ordinary => {
                // Ordinary functions also have no standard intrinsic. Missing
                // provenance denies inferred ownership; it is not itself a
                // program error. Real self assertions and member uses are
                // checked with the resulting ordinary function context.
                return ContextualTypeOwner::Absent;
            }
        }
        if self.expressions.get(source.call).owner.is_some() {
            // The named schema cache has no captured-parent component. Until
            // that original owner application is part of its identity, no
            // caller-specific activation may become a global class fact.
            checker.error("LUA2042", self.expressions.get(source.call).expression.span,
                "Contextual metatable ownership inside a function requires an owner application that retains its original capture facet.");
            self.merge_flow_report(input, checker.check_mode, checker.report, checker.engine);
            return ContextualTypeOwner::Absent;
        }
        let mut owner = None;
        if let Some(function) = source.call_function {
            if let Some(annotation) = &self.function(function).annotations.returns {
                if let Some(first) = annotation.types.first() {
                    let mut first = first.value_syntax();
                    while let TypeExprKind::Parenthesized(inner) = &first.kind {
                        first = inner.value_syntax();
                    }
                    // A written self result cannot define its own contextual
                    // owner. The original target supplies the fallback.
                    if !matches!(&first.kind, TypeExprKind::Named(name) if name == "self") {
                        checker.type_context = Some(function);
                        let result = checker.annotation(first);
                        if checker.engine.contextual_owner_self_type(&result).is_some() {
                            owner = Some(result);
                        }
                    }
                }
            } else if !self.inferred_call_result_requires_owner(function)
                && let Some(pack) = checker
                .engine
                .function_result_pack(&luanalysis_engine::Type::NativeFunction(function))
            {
                // Luanalysis v1 also obtains this owner from an inferred
                // __call result. Keep the existing native return query in
                // charge of inference, recursion, and provisional state.
                let result = checker.engine.pack_slot(&pack, 0);
                if checker.engine.contextual_owner_self_type(&result).is_some() {
                    owner = Some(result);
                }
            }
        }
        if owner.is_none() {
            // The exact attached class binding already has a nominal owner.
            // Reading its runtime initializer here demands every prospective
            // member, including the contextual-self members whose activation
            // this query is establishing. Reuse its original declaration
            // contract; the initializer and subsequent writes are checked
            // independently against that same class.
            let value = self.own_definition_value(source.target, checker.engine).unwrap_or_else(|| {
                let target = &self.expressions.get(source.target).expression;
                checker.expression(target)
            });
            if checker.engine.contextual_owner_self_type(&value).is_some() {
                owner = Some(value);
            }
        }
        if checker.incomplete_flow {
            return ContextualTypeOwner::Pending;
        }
        self.merge_flow_report(input, checker.check_mode, checker.report, checker.engine);
        owner.map_or(ContextualTypeOwner::Absent, ContextualTypeOwner::Owner)
    }

    fn inferred_call_result_requires_owner(&self, function: NativeFunctionKey) -> bool {
        let Some(parameter) = self.contextual_self_dependency(function) else {
            return false;
        };
        // A return that uses this contextual self cannot establish the same
        // owner. Inspect original dependencies before entering QueryReturn so
        // its legitimate target fallback does not become a return cycle.
        // Explicit generic parameters named self have different source keys.
        let mut used = crate::collections::HashSet::default();
        self.function_type_dependencies(
            function,
            &mut used,
            &mut crate::collections::HashSet::default(),
            &mut crate::collections::HashSet::default(),
        );
        used.contains(&parameter)
    }

    pub(crate) fn contextual_self_type(
        &self,
        function: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> ContextualTypeOwner {
        let Some((scope, _)) = self.metatable_context_scope(function) else {
            return ContextualTypeOwner::Absent;
        };
        match self.contextual_owner_at_creation(scope, queries) {
            ContextualTypeOwner::Owner(owner) => queries
                .contextual_owner_self_type(&owner)
                .map_or(ContextualTypeOwner::Absent, ContextualTypeOwner::Owner),
            other => other,
        }
    }

    pub(crate) fn contextual_self_dependency(
        &self,
        function: NativeFunctionKey,
    ) -> Option<luanalysis_engine::NativeTypeParameterKey> {
        let (_, source) = self.metatable_context_scope(function)?;
        self.types.self_parameter(source.candidate_owner?)
    }

    pub(crate) fn metatable_annotation_function(
        &self,
        mut context: NativeTypeContext,
    ) -> Option<NativeFunctionKey> {
        loop {
            match context {
                NativeTypeContext::Function(function) => return Some(function),
                NativeTypeContext::Signature(signature) => {
                    context = self.signatures.get(signature).context?
                }
                NativeTypeContext::NamedType(_) | NativeTypeContext::Annotation(_) => return None,
            }
        }
    }

    pub(crate) fn source_member_activation(
        &self,
        member: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> TypeMemberActivation {
        self.selected_original_member_candidates(member, queries)
            .map_or(TypeMemberActivation::Active, |(activation, _)| activation)
    }

    pub(crate) fn extend_contextual_self_dependencies(
        &self,
        context: NativeTypeContext,
        used: &crate::collections::HashSet<luanalysis_engine::NativeTypeParameterKey>,
        outer: &mut Vec<luanalysis_engine::NativeTypeParameterKey>,
    ) {
        if let Some(function) = self.metatable_annotation_function(context)
            && self.method_owner(function).is_none()
            && let Some(key) = self.contextual_self_dependency(function)
            && used.contains(&key)
            && !outer.contains(&key)
        {
            outer.push(key);
        }
    }

    /// Retry original validation demands after other declarations have settled.
    /// This queue contains source identities and native capabilities, not types.
    pub(crate) fn retry_contextual_owner_validations(&self, queries: &mut dyn TypeQueries) -> bool {
        let owners: Vec<_> = self
            .pending_contextual_owners
            .borrow()
            .iter()
            .copied()
            .collect();
        let bodies: Vec<_> = self.pending_owner_bodies.borrow().iter().copied().collect();
        for &(scope, input) in &owners {
            let contextual = self.flow.contextual(input.graph);
            let captured = self.flow.captured(input.graph);
            match self.body_frame_check_state(contextual.as_ref(), captured.as_ref(), queries) {
                NativeFunctionCheckState::Expired => { self.pending_contextual_owners.borrow_mut().remove(&(scope, input)); continue; }
                NativeFunctionCheckState::Provisional => continue,
                NativeFunctionCheckState::Final => {}
            }
            self.contextual_owner_query(scope, input, queries);
        }
        for &(key, contextual, captured) in &bodies {
            // Another frame of this function has already been checked completely.
            if self.body_checked_once(key) {
                self.pending_owner_bodies.borrow_mut().remove(&(key, contextual, captured));
                continue;
            }
            match self.body_frame_check_state(contextual.as_ref(), captured.as_ref(), queries) {
                NativeFunctionCheckState::Expired => { self.pending_owner_bodies.borrow_mut().remove(&(key, contextual, captured)); continue; }
                NativeFunctionCheckState::Provisional => continue,
                NativeFunctionCheckState::Final => {}
            }
            match (contextual, captured) {
                (Some(context), Some(capture)) => {
                    queries.with_captured_function(&capture, &mut |queries| {
                        self.check_body_in_context(key, &context, queries);
                    })
                }
                (Some(context), None) => {
                    self.check_body_in_context(key, &context, queries);
                }
                (None, Some(capture)) => {
                    self.check_body_in_capture(key, &capture, queries);
                }
                (None, None) => {
                    self.check_body(key, queries);
                }
            }
        }
        owners
            .iter()
            .any(|key| !self.pending_contextual_owners.borrow().contains(key))
            || bodies
                .iter()
                .any(|key| !self.pending_owner_bodies.borrow().contains(key))
    }

    pub(crate) fn report_unresolved_contextual_owners(&self) {
        let owners: Vec<_> = self
            .pending_contextual_owners
            .borrow()
            .iter()
            .copied()
            .collect();
        let mut report = crate::CheckReport::default();
        for (scope, _) in owners {
            let source = self
                .metatables
                .direct_scope(scope)
                .expect("original pending metatable scope");
            report.diagnostics.push(crate::Diagnostic {
                code: "LUA2040",
                span: self.expressions.get(source.call).expression.span,
                message: "Could not determine the class context for this call."
                    .into(),
            });
        }
        for &(function, _, _) in self.pending_owner_bodies.borrow().iter() {
            report.diagnostics.push(crate::Diagnostic {
                code: "LUA2041",
                span: self.function(function).span,
                message:
                    "Type checking could not finish for this function body."
                        .into(),
            });
        }
        self.merge_report(report);
    }
}
