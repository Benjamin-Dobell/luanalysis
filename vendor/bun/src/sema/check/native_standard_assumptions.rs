//! Ordinary host assumptions for selected original standard bindings.
//! These results classify operations; they never certify heap identity,
//! metatable absence, or purity of an unknown call.
use super::*;

struct StandardLoop { reference: NativeReferenceKey, head: NativeFlowKey, entry: NativeFlowKey, seed: Origins }
#[derive(Clone, Copy)]
struct StandardScope { function: bool, module: bool, loader: bool }
impl StandardScope { const ROOT: Self = Self { function: true, module: true, loader: false }; }

impl<'p, 's> Checker<'p, 's> {
    pub fn native_flow_standard_operation(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> NativeStandardOperation {
        let provider = self.native_function_provider.and_then(|provider| provider.project_flow_provider()).unwrap_or(provider);
        if self.native_assumed_environment().is_none() {
            return match self.native_flow_alias_intrinsic(provider, value, input) {
                NativeAliasIntrinsic::Standard(site) => NativeStandardOperation::Standard(site),
                NativeAliasIntrinsic::Unknown => NativeStandardOperation::Ordinary,
                NativeAliasIntrinsic::Pending => NativeStandardOperation::Pending,
            };
        }
        // Reuse completed physical evidence without starting an expensive raw
        // effect proof merely to classify an ordinary standard operation.
        let raw_key = (value, input, 0, self.flow_memo.native_state.effect_epoch, self.native_alias_cache.frame());
        let cached = self.native_alias_cache.values.get(&raw_key).cloned();
        let origins = cached.filter(|value| !value.unknown && !value.uncertain && !value.non_table && value.tokens.len() == 1
            && value.tokens.iter().all(|origin| matches!(origin, Origin::Standard(_))))
            .unwrap_or_else(|| self.native_standard_value(provider, value, input));
        if origins.uncertain { self.mark_tainted_from(0); return NativeStandardOperation::Pending }
        if origins.unknown || origins.non_table || origins.tokens.len() != 1 { return NativeStandardOperation::Ordinary }
        match origins.tokens.into_iter().next().unwrap() {
            Origin::Standard(site) => NativeStandardOperation::Standard(site),
            _ => NativeStandardOperation::Ordinary,
        }
    }

    /// Ordinary module typing has its own state walk. Never insert this host
    /// assumption into the shared flow walker used by physical loader proofs.
    pub fn native_flow_standard_loader_state(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, reference: NativeReferenceKey) -> NativeFlowResult {
        let provider = self.native_function_provider.and_then(|provider| provider.project_flow_provider()).unwrap_or(provider);
        if self.native_assumed_environment().is_none() { return self.native_flow_type(provider, at, reference, crate::types::TypeId::FALSE, crate::types::TypeId::TRUE) }
        self.native_standard_loader_state(provider, at, reference)
    }

    fn native_standard_loader_state(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, reference: NativeReferenceKey) -> NativeFlowResult {
        use crate::types::TypeId;
        let invalid = NativeFlowResult { ty: TypeId::FALSE, incomplete: false, unresolved_auto: false };
        if reference.graph != at.graph || !provider.contains_reference(reference) || provider.node(at).is_none() { return invalid }
        if self.is_stack_low() { self.mark_tainted_from(0); return NativeFlowResult { incomplete: true, ..invalid } }
        let key = (reference, at, self.flow_memo.native_state.effect_epoch, self.native_alias_cache.frame());
        if let Some(result) = self.native_alias_cache.standard_loaders.get(&key) { return *result }
        // Guard the original dependency too: recursive module execution can
        // otherwise create indefinitely many frames for the same source node.
        let query = Query::NativeStandardLoader(reference, at, key.2);
        if !self.enter(query) { self.mark_tainted_from(0); return NativeFlowResult { incomplete: true, ..invalid } }
        let (mut result, completed) = self.run_memoizable(|checker| match provider.node(at) {
            Some(NativeFlowNode::Start) => {
                if let Some(index) = checker.native_alias_cache.executions.iter().rposition(|frame| frame.entry == at) {
                    let caller = checker.native_alias_cache.executions[index].caller;
                    if let Some(parent) = provider.reference_in_graph(reference, caller.graph) {
                        return checker.with_native_alias_parent(index, |checker| checker.native_standard_loader_state(provider, caller, parent));
                    }
                    return invalid;
                }
                if provider.reference_follows_creation(reference)
                    && let Some(creation) = provider.graph_creation(at.graph)
                    && let Some(parent) = provider.reference_in_graph(reference, creation.input.graph)
                { return checker.native_standard_loader_state(provider, creation.input, parent) }
                NativeFlowResult { ty: TypeId::TRUE, ..invalid }
            },
            Some(NativeFlowNode::Write { before, target, .. }) => {
                if matches!(provider.write_affects_with_types(reference, target, at, checker), NativeWriteRelation::Exact | NativeWriteRelation::PossibleExact | NativeWriteRelation::Ancestor | NativeWriteRelation::PossibleAncestor) { invalid }
                else { checker.native_standard_loader_state(provider, before, reference) }
            },
            Some(NativeFlowNode::Call { before, value, input }) => {
                if provider.known_standard_loader_write(at) { return invalid }
                match checker.native_standard_call_continuation(provider, reference, value, input, StandardScope { loader: true, ..StandardScope::ROOT }, None) {
                    NativeCallPreservation::Preserved => checker.native_standard_loader_state(provider, before, reference),
                    NativeCallPreservation::Unknown => invalid,
                    NativeCallPreservation::Pending => NativeFlowResult { incomplete: true, ..invalid },
                }
            },
            Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }
                | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. }) => checker.native_standard_loader_state(provider, before, reference),
            Some(NativeFlowNode::Merge { edges }) => {
                let mut result = NativeFlowResult { ty: TypeId::TRUE, ..invalid };
                for edge in edges {
                    let incoming = checker.native_standard_loader_state(provider, edge, reference);
                    if incoming.ty != TypeId::TRUE { result.ty = TypeId::FALSE; }
                    result.incomplete |= incoming.incomplete;
                }
                result
            },
            Some(NativeFlowNode::Loop { edges }) => {
                let Some((&entry, backedges)) = edges.split_first() else { return invalid };
                let seed = checker.native_standard_loader_state(provider, entry, reference);
                if seed.incomplete || seed.ty != TypeId::TRUE { return seed }
                match checker.native_standard_scope_writes(provider, reference, at, backedges, None, StandardScope { loader: true, ..StandardScope::ROOT }, None) {
                    NativeCallPreservation::Preserved => seed,
                    NativeCallPreservation::Unknown => invalid,
                    NativeCallPreservation::Pending => NativeFlowResult { incomplete: true, ..invalid },
                }
            },
            _ => invalid,
        });
        if self.leave(query).is_err() || completed.is_none() || result.incomplete { result.incomplete = true; self.mark_tainted_from(0); }
        if !result.incomplete && !self.is_stack_low() { self.native_alias_cache.standard_loaders.insert(key, result); }
        result
    }

    pub(super) fn native_assumed_environment(&self) -> Option<NativeEnvironmentId> {
        let globals = self.native_globals_provider?;
        globals.assumed_standard_environment().filter(|&environment| globals.contains_global_environment(environment))
    }

    pub(super) fn native_standard_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> Origins {
        if self.is_stack_low() { self.mark_tainted_from(0); return Origins::unknown() }
        if value.snapshot != input.graph.snapshot || provider.node(input).is_none() { return Origins::unavailable() }
        let key = (value, input, self.flow_memo.native_state.effect_epoch, self.native_alias_cache.frame());
        if let Some(result) = self.native_alias_cache.standard_values.get(&key) { return result.clone() }
        let query = Query::NativeStandardValue(value, input, key.2, key.3);
        if !self.enter(query) { self.mark_tainted_from(0); return Origins::unknown() }
        let (mut result, completed) = self.run_memoizable(|checker| {
            if let Some(member) = provider.value_alias_environment_member(value, input)
                && let NativeAliasInitial::GlobalValue(binding) = provider.reference_alias_initial(member.reference)
                && checker.native_assumed_environment() == Some(binding.environment)
            {
                let receiver = checker.native_standard_value(provider, member.receiver.value, member.receiver.input);
                if receiver.uncertain { return receiver }
                if !receiver.unknown && receiver.tokens.len() == 1 && receiver.tokens.contains(&Origin::Environment(binding.environment)) {
                    return checker.native_standard_reference(provider, member.reference, member.input);
                }
            }
            match provider.value_alias_source(value, input) {
                NativeAliasValue::Reference { reference, input } => checker.native_standard_reference(provider, reference, input),
                NativeAliasValue::Projection { value, input, index: 0 } => checker.native_standard_value(provider, value, input),
                // Function syntax is an original callee recipe, not a type or
                // signature-shaped claim. No body is demanded here.
                NativeAliasValue::Function { .. } => checker.native_alias_value(provider, value, input, 0),
                NativeAliasValue::Allocation => Origins::one(Origin::Allocation(value, input), false),
                NativeAliasValue::Call { callee, input: callee_input } => checker.native_standard_return(provider, value, input, callee, callee_input),
                _ => Origins::unavailable(),
            }
        });
        if self.leave(query).is_err() || completed.is_none() { result.uncertain = true; self.mark_tainted_from(0); }
        if !result.uncertain && !self.is_stack_low() { self.native_alias_cache.standard_values.insert(key, result.clone()); }
        result
    }

    fn native_standard_projected_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey, path: &[NativeTableKey]) -> Origins {
        let Some((&key, rest)) = path.split_first() else { return self.native_standard_value(provider, value, input) };
        // Only original literal field recipes supply this projection. No raw
        // dispatch/purity proof is launched to classify a language operation.
        if let Some(field) = provider.value_alias_member_source(value, input, key, self) {
            return self.native_standard_projected_value(provider, field.value, field.input, rest);
        }
        Origins::unavailable()
    }

    fn native_standard_return(&mut self, provider: &dyn NativeFlowProvider, call: NativeValueKey, input: NativeFlowKey, callee: NativeValueKey, callee_input: NativeFlowKey) -> Origins {
        let Some(recipe) = provider.alias_call_source(call, input) else { return Origins::unavailable() };
        if recipe.callee.value != callee || recipe.callee.input != callee_input || provider.node(recipe.execution_input).is_none() { return Origins::unavailable() }
        let callee = self.native_standard_value(provider, callee, callee_input);
        if callee.uncertain { return callee }
        if callee.unknown || callee.non_table || callee.tokens.len() != 1 { return Origins::unavailable() }
        let Some(Origin::Function(function, creation, activation)) = callee.tokens.into_iter().next() else { return Origins::unavailable() };
        if activation != 0 && !self.native_alias_cache.executions.iter().any(|frame| frame.id == activation) { return Origins::unavailable() }
        if self.native_alias_cache.executions.iter().any(|frame| frame.function == Some((function, creation, activation))) { self.mark_tainted_from(0); return Origins::unknown() }
        let Some(execution) = provider.function_alias_execution(function, creation) else { return Origins::unavailable() };
        let Some(rows) = provider.function_alias_returns(function) else { return Origins::unavailable() };
        let Some(exit) = provider.function_alias_exit(function) else { return Origins::unavailable() };
        if !matches!(provider.node(execution.entry), Some(NativeFlowNode::Start)) || exit.graph != execution.entry.graph || rows.iter().any(|row| row.at.graph != execution.entry.graph) { return Origins::unavailable() }
        self.with_native_alias_execution(call, recipe.execution_input, &execution, Some((function, creation, activation)), |checker| {
            let mut result = Origins::default();
            for row in rows {
                if !matches!(provider.node(row.at), Some(NativeFlowNode::Return { .. })) { return Origins::unavailable() }
                let Some(value) = row.values.first() else { result.non_table = true; continue };
                result.merge(checker.native_standard_value(provider, value.value, value.input));
            }
            // Structural terminal shape is sufficient for these simple return
            // recipes. More complex implicit exits stay ordinary; no typed
            // reachability or body inference is demanded here.
            if !matches!(provider.node(exit), Some(NativeFlowNode::Unreachable | NativeFlowNode::Return { .. })) { result.non_table = true; }
            result
        })
    }

    fn native_standard_reference(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey) -> Origins {
        if self.is_stack_low() { self.mark_tainted_from(0); return Origins::unknown() }
        if reference.graph != at.graph || !provider.contains_reference(reference) || provider.node(at).is_none() { return Origins::unavailable() }
        let key = (reference, at, self.flow_memo.native_state.effect_epoch, self.native_alias_cache.frame());
        if let Some(result) = self.native_alias_cache.standard_references.get(&key) { return result.clone() }
        let query = Query::NativeStandardReference(reference, at, key.2, key.3);
        if !self.enter(query) { self.mark_tainted_from(0); return Origins::unknown() }
        let (mut result, completed) = self.run_memoizable(|checker| checker.native_standard_reference_worker(provider, reference, at));
        if self.leave(query).is_err() || completed.is_none() { result.uncertain = true; self.mark_tainted_from(0); }
        if !result.uncertain && !self.is_stack_low() { self.native_alias_cache.standard_references.insert(key, result.clone()); }
        result
    }

    fn native_standard_reference_worker(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey) -> Origins {
        match provider.node(at) {
            Some(NativeFlowNode::Start) => {
                let declaration = provider.reference_declaration(reference);
                let own_cell = provider.reference_is_cell(reference) && declaration.as_ref().is_some_and(|declaration| declaration.owner == at.graph);
                if let Some(index) = self.native_alias_cache.executions.iter().rposition(|frame| frame.entry == at).filter(|_| !own_cell) {
                    let caller = self.native_alias_cache.executions[index].caller;
                    if let Some(reference) = provider.reference_in_graph(reference, caller.graph) {
                        return self.with_native_alias_parent(index, |checker| checker.native_standard_reference(provider, reference, caller));
                    }
                }
                if let Some(declaration) = declaration
                    && declaration.owner != reference.graph
                    && let Some(creation) = provider.graph_creation(reference.graph)
                    && provider.graph_creation(declaration.owner).is_none()
                    && (!matches!(provider.reference_alias_initial(reference), NativeAliasInitial::Opaque)
                        || declaration.writes.iter().all(|write| write.owner == declaration.owner && write.statement_end <= creation.position))
                    && let Some(outer) = provider.reference_in_graph(reference, creation.input.graph)
                { return self.native_standard_reference(provider, outer, creation.input) }
                let initial = self.native_alias_initial(provider, reference, at);
                let environment = self.native_assumed_environment();
                if initial.tokens.iter().all(|origin| match origin {
                    Origin::Standard(site) => Some(site.binding.environment) == environment,
                    Origin::Environment(value) => Some(*value) == environment,
                    _ => false,
                }) { initial } else { Origins::unavailable() }
            },
            Some(NativeFlowNode::Write { before, target, value, input }) => {
                // Always transfer an observed assignment before considering
                // unsupported dispatch. A later unknown operation cannot
                // restore the original standard value.
                let relation = if provider.reference_is_cell(reference) { provider.write_affects(reference, target, at) }
                    else { provider.write_affects_with_types(reference, target, at, self) };
                match relation {
                    NativeWriteRelation::Exact => self.native_standard_value(provider, value, input),
                    NativeWriteRelation::PossibleExact => {
                        let mut result = self.native_standard_reference(provider, reference, before);
                        result.merge(self.native_standard_value(provider, value, input));
                        result
                    },
                    NativeWriteRelation::Ancestor => provider.reference_member_path(target, reference, self)
                        .map_or_else(Origins::unavailable, |path| self.native_standard_projected_value(provider, value, input, &path)),
                    NativeWriteRelation::PossibleAncestor => Origins::unavailable(),
                    _ => self.native_standard_reference(provider, reference, before),
                }
            },
            Some(NativeFlowNode::Call { before, value, input }) => match self.native_standard_call_continuation(provider, reference, value, input, StandardScope::ROOT, None) {
                NativeCallPreservation::Preserved => self.native_standard_reference(provider, reference, before),
                NativeCallPreservation::Unknown => Origins::unavailable(),
                NativeCallPreservation::Pending => Origins::unknown(),
            },
            Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }
                | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. }) => self.native_standard_reference(provider, reference, before),
            Some(NativeFlowNode::Merge { edges }) => {
                let mut result = Origins::default();
                for edge in edges { result.merge(self.native_standard_reference(provider, reference, edge)); }
                result
            },
            Some(NativeFlowNode::Loop { edges }) => {
                let Some((&entry, backedges)) = edges.split_first() else { return Origins::unavailable() };
                let seed = self.native_standard_reference(provider, reference, entry);
                if seed.uncertain || seed.unknown || seed.tokens.len() != 1 { return seed }
                let invariant = StandardLoop { reference, head: at, entry, seed };
                match self.native_standard_scope_writes(provider, reference, at, backedges, None, StandardScope::ROOT, Some(&invariant)) {
                    NativeCallPreservation::Preserved => invariant.seed,
                    NativeCallPreservation::Unknown => Origins::unavailable(),
                    NativeCallPreservation::Pending => Origins::unknown(),
                }
            },
            _ => Origins::unavailable(),
        }
    }

    pub fn native_flow_standard_call_continuation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey) -> NativeCallPreservation {
        let provider = self.native_function_provider.and_then(|provider| provider.project_flow_provider()).unwrap_or(provider);
        if self.native_assumed_environment().is_none() { return self.native_alias_call_preservation(provider, reference, call, input) }
        self.native_standard_call_continuation(provider, reference, call, input, StandardScope::ROOT, None)
    }

    fn native_standard_call_continuation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey, scope: StandardScope, callee: Option<Origins>) -> NativeCallPreservation {
        // Unknown calls are ordinary continuations under the selected policy.
        // Inspect at most one module top level and one reached function.
        // There is no recursive host/metamethod purity proof.
        if !scope.function && !scope.module { return NativeCallPreservation::Preserved }
        let query = Query::NativeStandardCall(reference, call, input, self.flow_memo.native_state.effect_epoch, self.native_alias_cache.frame());
        if !self.enter(query) { self.mark_tainted_from(0); return NativeCallPreservation::Pending }
        let (result, completed) = self.run_memoizable(|checker| checker.native_standard_call_worker(provider, reference, call, input, scope, callee));
        if self.leave(query).is_err() || completed.is_none() || result == NativeCallPreservation::Pending {
            self.mark_tainted_from(0);
            return NativeCallPreservation::Pending;
        }
        result
    }

    fn native_standard_call_worker(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey, scope: StandardScope, callee: Option<Origins>) -> NativeCallPreservation {
        let Some(mut recipe) = provider.alias_call_source(call, input) else { return NativeCallPreservation::Preserved };
        let callee = callee.unwrap_or_else(|| self.native_standard_value(provider, recipe.callee.value, recipe.callee.input));
        if callee.uncertain { return NativeCallPreservation::Pending }
        if callee.unknown || callee.tokens.len() != 1 { return NativeCallPreservation::Preserved }
        let (executions, function, next) = match callee.tokens.into_iter().next().unwrap() {
            Origin::Function(function, creation, activation) if scope.function => {
                let Some(execution) = provider.function_alias_execution(function, creation) else { return NativeCallPreservation::Preserved };
                (vec![execution], Some((function, creation, activation)), StandardScope { function: false, module: false, loader: scope.loader })
            },
            allocation @ Origin::Allocation(..) if scope.function => {
                // Reuse only the bounded original setmetatable/__call target
                // recipe. Unknown intervening effects still fail that raw
                // lookup; no purity or absence certificate is inferred here.
                let target = match self.native_alias_callable_table(provider, &allocation, recipe.execution_input) {
                    Ok(Some(target)) => target,
                    Ok(None) => return NativeCallPreservation::Preserved,
                    Err(()) => return NativeCallPreservation::Pending,
                };
                let Some(execution) = provider.function_alias_execution(target.function, target.creation) else { return NativeCallPreservation::Preserved };
                let receiver = recipe.callee;
                recipe.callee = target.callee;
                recipe.arguments.insert(0, receiver);
                (vec![execution], Some((target.function, target.creation, target.activation)), StandardScope { function: false, module: false, loader: scope.loader })
            },
            Origin::Standard(site) if site.kind == NativeIntrinsicKind::Require && scope.module => {
                // An original import recipe is only a candidate execution.
                // A reached loader replacement must not activate the former
                // target module while classifying ordinary builtin operations.
                let Some(loader) = provider.require_alias_loader_reference(call, input) else { return NativeCallPreservation::Preserved };
                let state = self.native_standard_loader_state(provider, recipe.execution_input, loader);
                if state.incomplete { return NativeCallPreservation::Pending }
                if state.ty != crate::types::TypeId::TRUE { return NativeCallPreservation::Preserved }
                let Some(executions) = provider.require_alias_execution(call, input) else { return NativeCallPreservation::Preserved };
                (executions, None, StandardScope { function: scope.function, module: false, loader: scope.loader })
            },
            _ => return NativeCallPreservation::Preserved,
        };
        for execution in executions {
            let Some(watched) = provider.reference_in_graph(reference, execution.entry.graph) else { continue };
            let result = self.with_native_alias_execution(call, recipe.execution_input, &execution, function, |checker| {
                checker.native_standard_scope_writes(provider, watched, execution.entry, &execution.terminals, Some(execution.entry), next, None)
            });
            if result != NativeCallPreservation::Preserved { return result }
        }
        NativeCallPreservation::Preserved
    }

    fn native_standard_scope_writes(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, boundary: NativeFlowKey, terminals: &[NativeFlowKey], entry: Option<NativeFlowKey>, scope: StandardScope, invariant: Option<&StandardLoop>) -> NativeCallPreservation {
        let mut work = terminals.to_vec();
        let mut visited = HashSet::new();
        while let Some(at) = work.pop() {
            if at == boundary { continue }
            if self.is_stack_low() { self.mark_tainted_from(0); return NativeCallPreservation::Pending }
            if at.graph != boundary.graph || visited.len() >= 4_096 { return NativeCallPreservation::Preserved }
            if !visited.insert(at) { continue }
            match provider.node(at) {
                Some(NativeFlowNode::Start) if Some(at) == entry => {},
                Some(NativeFlowNode::Write { before, target, .. }) => {
                    let relation = if provider.reference_is_cell(reference) { provider.write_affects(reference, target, at) }
                        else if let Some(invariant) = invariant {
                            match self.native_standard_loop_environment_write(provider, reference, target, at, invariant) {
                                Ok(Some(relation)) => relation,
                                Ok(None) => provider.write_affects_with_types(reference, target, at, self),
                                Err(()) => return NativeCallPreservation::Pending,
                            }
                        } else { provider.write_affects_with_types(reference, target, at, self) };
                    if matches!(relation, NativeWriteRelation::Exact | NativeWriteRelation::PossibleExact | NativeWriteRelation::Ancestor | NativeWriteRelation::PossibleAncestor) {
                        return NativeCallPreservation::Unknown;
                    }
                    work.push(before);
                },
                Some(NativeFlowNode::Call { before, value, input }) => {
                    if scope.loader && provider.known_standard_loader_write(at) { return NativeCallPreservation::Unknown }
                    let callee = invariant.and_then(|invariant| provider.alias_call_source(value, input).map(|recipe| {
                        self.native_standard_loop_value(provider, recipe.callee.value, recipe.callee.input, invariant, &mut HashSet::new())
                    }));
                    let result = self.native_standard_call_continuation(provider, reference, value, input, scope, callee);
                    if result != NativeCallPreservation::Preserved { return result }
                    work.push(before);
                },
                Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }
                    | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. }
                    | NativeFlowNode::Return { before }) => work.push(before),
                Some(NativeFlowNode::Merge { edges } | NativeFlowNode::Loop { edges }) => work.extend(edges),
                _ => {},
            }
        }
        NativeCallPreservation::Preserved
    }

    fn native_standard_loop_environment_write(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, invariant: &StandardLoop) -> Result<Option<NativeWriteRelation>, ()> {
        let NativeAliasInitial::GlobalValue(binding) = provider.reference_alias_initial(reference) else { return Ok(None) };
        if self.native_assumed_environment() != Some(binding.environment) { return Ok(None) }
        let Some(receiver) = provider.write_alias_receiver(at) else { return Ok(None) };
        // Use the store's original frozen receiver, not its flowed type: typing
        // a captured environment alias here can demand this loop's call effect.
        // The local loop substitution is never published as physical evidence.
        let receiver = self.native_standard_loop_value(provider, receiver.value, receiver.input, invariant, &mut HashSet::new());
        if receiver.uncertain { return Err(()) }
        if receiver.unknown || receiver.non_table || receiver.tokens.len() != 1
            || !receiver.tokens.contains(&Origin::Environment(binding.environment)) { return Ok(None) }
        let relation = match provider.write_member_key(reference, target, at, self) {
            Some(NativeTableKey::String(name)) => {
                let written = self.native_globals_provider.and_then(|globals| globals.global_environment_binding(binding.environment, self.atoms().bytes(name)));
                if written == Some(binding) { NativeWriteRelation::Exact } else { NativeWriteRelation::Unaffected }
            },
            Some(_) => NativeWriteRelation::Unaffected,
            None => NativeWriteRelation::PossibleExact,
        };
        Ok(Some(relation))
    }

    // Loop substitutions are local to this metadata walk. Only the watched
    // original slot uses the entry hypothesis; other cells start at the real
    // entry epoch. No hypothetical value enters an ordinary alias cache.
    fn native_standard_loop_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey, invariant: &StandardLoop, active: &mut HashSet<(NativeReferenceKey, NativeFlowKey)>) -> Origins {
        match provider.value_alias_source(value, input) {
            NativeAliasValue::Reference { reference, input } => self.native_standard_loop_reference(provider, reference, input, invariant, active),
            NativeAliasValue::Projection { value, input, index: 0 } => self.native_standard_loop_value(provider, value, input, invariant, active),
            NativeAliasValue::Function { .. } => self.native_alias_value(provider, value, input, 0),
            _ => Origins::unavailable(),
        }
    }

    fn native_standard_loop_reference(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey, invariant: &StandardLoop, active: &mut HashSet<(NativeReferenceKey, NativeFlowKey)>) -> Origins {
        if at == invariant.head {
            return if reference == invariant.reference { invariant.seed.clone() }
                else { self.native_standard_reference(provider, reference, invariant.entry) };
        }
        if active.len() >= 4_096 || !active.insert((reference, at)) { return Origins::unavailable() }
        let result = match provider.node(at) {
            Some(NativeFlowNode::Write { before, target, value, input }) => match provider.write_affects(reference, target, at) {
                NativeWriteRelation::Exact => self.native_standard_loop_value(provider, value, input, invariant, active),
                NativeWriteRelation::Unaffected => self.native_standard_loop_reference(provider, reference, before, invariant, active),
                _ => Origins::unavailable(),
            },
            Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. } | NativeFlowNode::Call { before, .. }
                | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. }) => self.native_standard_loop_reference(provider, reference, before, invariant, active),
            Some(NativeFlowNode::Merge { edges }) => {
                let mut result = Origins::default();
                for edge in edges { result.merge(self.native_standard_loop_reference(provider, reference, edge, invariant, active)); }
                result
            },
            _ => self.native_standard_reference(provider, reference, at),
        };
        active.remove(&(reference, at));
        result
    }
}
