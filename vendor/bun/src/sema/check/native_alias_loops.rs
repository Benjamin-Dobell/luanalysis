//! An original canonical value can survive a loop without identifying any
//! repeated allocation site. The induction hypothesis stays in this local
//! metadata walk; normal alias/flow caches never receive speculative origins.
use super::*;

struct Invariant { reference: NativeReferenceKey, head: NativeFlowKey, seed: Origins }

impl<'p, 's> Checker<'p, 's> {
    pub(super) fn native_alias_loop_reference(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, head: NativeFlowKey, edges: &[NativeFlowKey]) -> Origins {
        let Some((&entry, backedges)) = edges.split_first() else { return Origins::unavailable() };
        // This first certificate is for original environment roots/slots.
        // Do not demand unrelated captured/local provenance to discover that
        // the loop capability does not cover its allocation identity.
        if matches!(provider.reference_alias_initial(reference), NativeAliasInitial::Opaque) { return Origins::unavailable() }
        let seed = self.native_alias_reference(provider, reference, entry);
        if seed.uncertain { return seed }
        // Canonical runtime roots have an identity independent of an
        // allocation/closure site. Other provenance remains unsupported here.
        if seed.unknown || seed.non_table || seed.maybe_nil || seed.tokens.len() != 1
            || !seed.tokens.iter().all(|origin| matches!(origin, Origin::Standard(_) | Origin::Environment(_)))
        { return Origins::unavailable() }
        let invariant = Invariant { reference, head, seed };
        let mut work: Vec<_> = backedges.iter().map(|&at| (at, false)).collect();
        let mut active = HashSet::new();
        let mut completed = HashSet::new();
        let mut steps = 0usize;
        while let Some((at, leaving)) = work.pop() {
            steps += 1;
            if steps > 16_384 || at.graph != head.graph || self.is_stack_low() { return Origins::unavailable() }
            if at == head { continue }
            if leaving { active.remove(&at); completed.insert(at); continue }
            if completed.contains(&at) { continue }
            if !active.insert(at) { return Origins::unavailable() }
            let before = match provider.node(at) {
                Some(NativeFlowNode::Write { before, target, .. })
                    if provider.reference_is_cell(target)
                        && provider.write_affects(reference, target, at) == NativeWriteRelation::Unaffected
                        && Self::native_loop_dispatch_safe(provider, at, &invariant) => before,
                Some(NativeFlowNode::Call { before, value, input }) => {
                    if self.native_loop_call_preservation(provider, reference, value, input, &invariant, &mut HashSet::new()) != NativeCallPreservation::Preserved { return Origins::unavailable() }
                    before
                },
                Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }
                    | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. })
                    if Self::native_loop_dispatch_safe(provider, at, &invariant) => before,
                Some(NativeFlowNode::Merge { edges }) if Self::native_loop_dispatch_safe(provider, at, &invariant) => {
                    work.push((at, true));
                    work.extend(edges.into_iter().map(|edge| (edge, false)));
                    continue;
                },
                _ => return Origins::unavailable(),
            };
            work.push((at, true));
            work.push((before, false));
        }
        invariant.seed
    }

    fn native_loop_dispatch_safe(provider: &dyn NativeFlowProvider, at: NativeFlowKey, invariant: &Invariant) -> bool {
        match provider.alias_node_dispatch(at) {
            NativeAliasDispatch::NoDispatch => true,
            NativeAliasDispatch::ReferenceRead(reference) => reference.graph == at.graph && provider.contains_reference(reference)
                && (provider.reference_is_cell(reference) || reference == invariant.reference),
            NativeAliasDispatch::Unknown => false,
        }
    }

    fn native_loop_call_preservation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey, invariant: &Invariant, active: &mut HashSet<(NativeReferenceKey, NativeFlowKey)>) -> NativeCallPreservation {
        let Some(recipe) = provider.alias_call_source(call, input) else { return NativeCallPreservation::Unknown };
        if recipe.execution_input.graph != input.graph || provider.node(recipe.execution_input).is_none() { return NativeCallPreservation::Unknown }
        let Some(callee) = self.native_loop_origin_value(provider, recipe.callee.value, recipe.callee.input, invariant, active) else { return NativeCallPreservation::Unknown };
        if callee.uncertain { return NativeCallPreservation::Pending }
        if callee.unknown || callee.non_table || callee.tokens.len() != 1 { return NativeCallPreservation::Unknown }
        // Imported execution, callable-table dispatch, and nested calls need
        // a more general invariant proof. Do not launch a cyclic identity
        // demand merely to discover that this certificate lacks it.
        match callee.tokens.iter().next().unwrap() {
            Origin::Standard(site) if site.kind != NativeIntrinsicKind::Require => {},
            Origin::Function(function, creation, _) if Self::native_loop_pure_execution(provider, *function, *creation) => {},
            _ => return NativeCallPreservation::Unknown,
        }
        self.native_alias_call_preservation_for_callee(provider, reference, call, input, recipe, callee)
    }

    fn native_loop_pure_execution(provider: &dyn NativeFlowProvider, function: NativeFunctionId, creation: NativeFlowKey) -> bool {
        let Some(execution) = provider.function_alias_execution(function, creation) else { return false };
        if execution.terminals.is_empty() || !matches!(provider.node(execution.entry), Some(NativeFlowNode::Start)) { return false }
        let mut work = execution.terminals;
        let mut visited = HashSet::new();
        while let Some(at) = work.pop() {
            if at.graph != execution.entry.graph || visited.len() >= 16_384 { return false }
            if !visited.insert(at) { continue }
            match provider.node(at) {
                Some(NativeFlowNode::Start) if at == execution.entry => {},
                Some(NativeFlowNode::Write { before, target, .. }) if provider.reference_is_cell(target)
                    && provider.reference_declaration(target).is_some_and(|declaration| declaration.owner == execution.entry.graph)
                    && matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) => work.push(before),
                Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. })
                    if matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) => work.push(before),
                Some(NativeFlowNode::Return { before }) => work.push(before),
                Some(NativeFlowNode::Merge { edges }) if matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) => work.extend(edges),
                Some(NativeFlowNode::Unreachable) => {},
                _ => return false,
            }
        }
        true
    }

    fn native_loop_origin_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey, invariant: &Invariant, active: &mut HashSet<(NativeReferenceKey, NativeFlowKey)>) -> Option<Origins> {
        if value.snapshot != input.graph.snapshot || provider.node(input).is_none() || self.is_stack_low() { return None }
        match provider.value_alias_source(value, input) {
            NativeAliasValue::Reference { reference, input } => self.native_loop_origin_reference(provider, reference, input, invariant, active),
            NativeAliasValue::Function { .. } => Some(self.native_alias_value(provider, value, input, 0)),
            NativeAliasValue::Projection { value, input, index: 0 } => self.native_loop_origin_value(provider, value, input, invariant, active),
            _ => None,
        }
    }

    fn native_loop_origin_reference(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey, invariant: &Invariant, active: &mut HashSet<(NativeReferenceKey, NativeFlowKey)>) -> Option<Origins> {
        if reference.graph != at.graph || !provider.contains_reference(reference) || active.len() >= 16_384 || self.is_stack_low() { return None }
        if at == invariant.head { return (reference == invariant.reference).then(|| invariant.seed.clone()) }
        if !active.insert((reference, at)) { return None }
        let result = match provider.node(at) {
            Some(NativeFlowNode::Start) => Some(self.native_alias_reference(provider, reference, at)),
            Some(NativeFlowNode::Write { before, target, value, input })
                if provider.reference_is_cell(target) && Self::native_loop_dispatch_safe(provider, at, invariant) => {
                match provider.write_affects(reference, target, at) {
                    NativeWriteRelation::Exact => self.native_loop_origin_value(provider, value, input, invariant, active),
                    NativeWriteRelation::Unaffected => self.native_loop_origin_reference(provider, reference, before, invariant, active),
                    _ => None,
                }
            },
            Some(NativeFlowNode::Call { before, value, input }) => {
                if self.native_loop_call_preservation(provider, reference, value, input, invariant, active) == NativeCallPreservation::Preserved {
                    self.native_loop_origin_reference(provider, reference, before, invariant, active)
                } else { None }
            },
            Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }
                | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. })
                if Self::native_loop_dispatch_safe(provider, at, invariant) => self.native_loop_origin_reference(provider, reference, before, invariant, active),
            Some(NativeFlowNode::Merge { edges }) => {
                let mut origins = Origins::default();
                let mut complete = true;
                for edge in edges {
                    if let Some(origin) = self.native_loop_origin_reference(provider, reference, edge, invariant, active) { origins.merge(origin) }
                    else { complete = false; break }
                }
                complete.then_some(origins)
            },
            _ => None,
        };
        active.remove(&(reference, at));
        result
    }
}
