//! Reached caller state through a proved original zero-argument invocation.
//! Graph recipes remain immutable; all state and entry bridges are checker-local.
use super::*;
use crate::check::native_aliases::{NativeAliasExecution, NativeCallPreservation, NativeReachedCallee};
use std::collections::HashSet;

#[derive(Default)]
struct CallCaches {
    loops: FxHashMap<LoopKey, TypeId>,
    values: FxHashMap<ValueContext, TypeId>,
    packs: FxHashMap<ValueContext, NativeResultPack>,
    effects: FxHashMap<(ValueContext, u64), NativeCallEffects>,
    reachable: FxHashMap<NativeFlowKey, bool>,
    latest: Option<(NativeFlowKey, bool)>,
    disabled: FxHashMap<NativeGraphKey, ()>,
    owners: FxHashMap<(NativeFunctionId, NativeFlowKey, u64), Option<TypeId>>,
}

pub(super) struct NativeCallStateFrame {
    pub(super) entry: NativeFlowKey,
    caller: NativeFlowKey,
    watched: NativeReferenceKey,
    previous: FlowType,
    contextual: Option<NativeFunctionContext>,
    captured: Option<NativeCapturedFunctionContext>,
    outside: Option<CallCaches>,
    outside_epoch: u64,
    alias_depth: usize,
    unsupported: bool,
}

enum CallState { Unsupported, Pending, Value(FlowType) }

impl<'p, 's> Checker<'p, 's> {
    fn take_call_caches(&mut self) -> CallCaches {
        let state = &mut self.flow_memo.native_state;
        CallCaches {
            loops: std::mem::take(&mut state.loop_types),
            values: std::mem::take(&mut state.value_types),
            packs: std::mem::take(&mut state.pack_types),
            effects: std::mem::take(&mut state.call_effects),
            reachable: std::mem::take(&mut state.reachable),
            latest: state.latest_reachable.take(),
            disabled: std::mem::take(&mut state.disabled),
            owners: std::mem::take(&mut state.contextual_owners),
        }
    }

    fn restore_call_caches(&mut self, cache: CallCaches) {
        let state = &mut self.flow_memo.native_state;
        state.loop_types = cache.loops;
        state.value_types = cache.values;
        state.pack_types = cache.packs;
        state.call_effects = cache.effects;
        state.reachable = cache.reachable;
        state.latest_reachable = cache.latest;
        state.disabled = cache.disabled;
        state.contextual_owners = cache.owners;
    }

    fn with_call_parent<R>(&mut self, index: usize, work: impl FnOnce(&mut Self) -> R) -> R {
        let mut outside = self.flow_memo.native_state.call_executions.split_off(index);
        let parent = outside[0].outside.take().expect("call entry owns its caller caches");
        let epoch = self.flow_memo.native_state.effect_epoch;
        let current = self.take_call_caches();
        self.restore_call_caches(parent);
        self.flow_memo.native_state.effect_epoch = outside[0].outside_epoch;
        let contextual = outside[0].contextual;
        let captured = outside[0].captured;
        let alias_depth = outside[0].alias_depth;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.with_native_alias_parent(alias_depth, |checker| {
                checker.with_native_function_frame(contextual, captured, work)
            })
        }));
        outside[0].outside = Some(self.take_call_caches());
        self.restore_call_caches(current);
        self.flow_memo.native_state.effect_epoch = epoch;
        self.flow_memo.native_state.call_executions.extend(outside);
        result.unwrap_or_else(|error| std::panic::resume_unwind(error))
    }

    pub(super) fn native_call_entry(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, walk: &NativeWalk) -> Option<FlowType> {
        let index = self.flow_memo.native_state.call_executions.iter().rposition(|frame| frame.entry == at)?;
        let frame = &self.flow_memo.native_state.call_executions[index];
        let watched = frame.watched;
        let previous = frame.previous;
        let caller = frame.caller;
        if provider.reference_declaration(walk.reference).is_some_and(|declaration| declaration.owner == at.graph) {
            return None;
        }
        if walk.reference == watched { return Some(previous) }
        // Descendant reads use the actual incoming view, even if that spelling
        // never existed in the caller's original reference table.
        if let Some(path) = provider.reference_member_path(watched, walk.reference, self) {
            let mut ty = previous.ty;
            for key in path {
                match self.native_table_read(ty, crate::types::NativeTableAccessKey::Literal(key)) {
                    Ok(value) => ty = value,
                    Err(_) => {
                        self.flow_memo.native_state.call_executions[index].unsupported = true;
                        return Some(FlowType::new(TypeId::ERROR, false));
                    }
                }
            }
            return Some(FlowType::new(ty, previous.incomplete));
        }
        let Some(reference) = provider.reference_in_graph(walk.reference, caller.graph) else {
            self.flow_memo.native_state.call_executions[index].unsupported = true;
            return Some(FlowType::new(walk.initial, true));
        };
        let declared = walk.declared;
        let initial = walk.initial;
        let result = self.with_call_parent(index, |checker| checker.native_flow_capture_type(provider, caller, reference, declared, initial));
        Some(FlowType::new(result.ty, result.incomplete))
    }

    pub(super) fn native_call_state_transfer(&mut self, provider: &dyn NativeFlowProvider, walk: &NativeWalk, call: NativeValueKey, input: NativeFlowKey, previous: FlowType) -> FlowType {
        if previous.incomplete || !(self.native_has_runtime_table(previous.ty) || self.native_definition_flow_target(previous.ty)) { return previous }
        match self.native_call_state(provider, walk, call, input, previous) {
            CallState::Value(result) => result,
            CallState::Unsupported => previous,
            CallState::Pending => { self.mark_tainted_from(0); FlowType::new(previous.ty, true) },
        }
    }

    fn native_call_state(&mut self, provider: &dyn NativeFlowProvider, walk: &NativeWalk, call: NativeValueKey, input: NativeFlowKey, previous: FlowType) -> CallState {
        let provider = self.native_function_provider.and_then(|provider| provider.project_flow_provider()).unwrap_or(provider);
        let (function, creation, activation, recipe) = match self.native_reached_callee(provider, call, input) {
            NativeReachedCallee::Function { function, creation, activation, call } => (function, creation, activation, call),
            NativeReachedCallee::Pending => return CallState::Pending,
            NativeReachedCallee::Unsupported => return CallState::Unsupported,
        };
        let ty = provider.alias_callee_type(recipe.callee.value, recipe.callee.input, self);
        let Some(signature) = self.native_implementation_signature(ty) else { return CallState::Unsupported };
        if self.sig_native_decl(signature).is_none_or(|actual| actual.0 != function)
            || !self.sig_type_params(signature).is_empty() || !self.sig_params(signature).is_empty()
        { return CallState::Unsupported }
        let epoch = self.flow_memo.native_state.effect_epoch;
        let query = Query::NativeFlowCallState(walk.reference, call, input, previous.ty, signature, epoch, self.native_alias_execution_key());
        if !self.enter(query) { self.mark_tainted_from(0); return CallState::Pending }
        let begun = self.begin_memoizable();
        let contextual = self.current_native_function_context();
        let captured = self.current_native_captured_function();
        let selected = NativeCapturedFunctionContext { signature };
        let callee_contextual = self.sig_native_context(signature);
        let result = self.with_native_function_frame(callee_contextual, Some(selected), |checker| {
            let Some(functions) = checker.native_function_provider else { return CallState::Unsupported };
            let Some(execution) = functions.function_flow_execution(checker, function, creation) else { return CallState::Unsupported };
            if !provider.contains_graph(execution.entry.graph) || !matches!(provider.node(execution.entry), Some(NativeFlowNode::Start))
                || execution.terminals.is_empty() || execution.terminals.iter().any(|terminal| terminal.graph != execution.entry.graph)
            { return CallState::Unsupported }
            let Some(watched) = provider.reference_in_graph(walk.reference, execution.entry.graph) else { return CallState::Unsupported };
            if !provider.contains_reference(watched) { return CallState::Unsupported }
            let outside = checker.take_call_caches();
            let outside_epoch = checker.flow_memo.native_state.effect_epoch;
            checker.flow_memo.native_state.fresh_effect_epoch();
            let alias_depth = checker.native_alias_execution_depth();
            checker.with_native_alias_execution(call, recipe.execution_input, &execution, Some((function, creation, activation)), |checker| {
                checker.flow_memo.native_state.call_executions.push(NativeCallStateFrame {
                    entry: execution.entry, caller: recipe.execution_input, watched,
                    previous, contextual, captured, outside: Some(outside), outside_epoch, alias_depth, unsupported: false,
                });
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    checker.native_call_replay(provider, &execution, watched, walk.declared, previous)
                }));
                let frame = checker.flow_memo.native_state.call_executions.pop().expect("actual call-state entry scope");
                checker.restore_call_caches(frame.outside.expect("actual caller cache scope"));
                checker.flow_memo.native_state.effect_epoch = frame.outside_epoch;
                let result = result.unwrap_or_else(|error| std::panic::resume_unwind(error));
                if frame.unsupported { CallState::Unsupported } else { result }
            })
        });
        let complete = self.end_memoizable(begun);
        let left = self.leave(query);
        if complete.is_none() || left.is_err() { return CallState::Pending }
        result
    }

    fn native_call_replay(&mut self, provider: &dyn NativeFlowProvider, execution: &NativeAliasExecution, watched: NativeReferenceKey, declared: TypeId, previous: FlowType) -> CallState {
        match self.native_call_replay_safe(provider, execution, watched) {
            NativeCallPreservation::Pending => return CallState::Pending,
            NativeCallPreservation::Unknown => return CallState::Unsupported,
            NativeCallPreservation::Preserved => {},
        }
        let mut values = Vec::new();
        for &terminal in &execution.terminals {
            if self.native_flow_definitely_unreachable(provider, terminal) { continue }
            let result = self.native_flow_capture_type(provider, terminal, watched, declared, previous.ty);
            if result.incomplete { return CallState::Pending }
            values.push(result.ty);
        }
        if values.is_empty() { return CallState::Unsupported }
        CallState::Value(FlowType::new(self.union_or_evolving_with(&values, false), false))
    }

    fn native_call_replay_safe(&mut self, provider: &dyn NativeFlowProvider, execution: &NativeAliasExecution, watched: NativeReferenceKey) -> NativeCallPreservation {
        let mut pending: Vec<_> = execution.terminals.iter().rev().map(|&at| (at, false)).collect();
        let mut active = HashSet::new();
        let mut completed = HashSet::new();
        while let Some((at, leaving)) = pending.pop() {
            if leaving { active.remove(&at); completed.insert(at); continue }
            if completed.contains(&at) { continue }
            if at.graph != execution.entry.graph || self.is_stack_low() { return NativeCallPreservation::Unknown }
            if self.native_flow_definitely_unreachable(provider, at) { continue }
            if !active.insert(at) { self.mark_tainted_from(0); return NativeCallPreservation::Pending }
            let Some(node) = provider.node(at) else { return NativeCallPreservation::Unknown };
            let edges = match node {
                NativeFlowNode::Start if at == execution.entry => Vec::new(),
                NativeFlowNode::Write { before, .. } | NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. } => {
                    if !self.native_alias_dispatch_safe(provider, at, before) { return NativeCallPreservation::Unknown }
                    vec![before]
                },
                NativeFlowNode::Call { before, value, input } => {
                    match self.native_flow_call_preservation(provider, watched, value, input) {
                        NativeCallPreservation::Preserved => vec![before],
                        other => return other,
                    }
                },
                NativeFlowNode::Merge { edges } if matches!(provider.alias_node_dispatch(at), super::super::native_aliases::NativeAliasDispatch::NoDispatch) => edges,
                NativeFlowNode::Unreachable => Vec::new(),
                _ => return NativeCallPreservation::Unknown,
            };
            pending.push((at, true));
            pending.extend(edges.into_iter().rev().map(|edge| (edge, false)));
        }
        NativeCallPreservation::Preserved
    }
}
