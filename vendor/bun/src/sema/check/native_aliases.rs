//! Object identity provenance from immutable original Lua flow recipes.
//! These are allocation/copy tokens, never fabricated type or syntax IDs.
use super::{Checker, Query};
use super::native_flow::*;
use super::task::Stored;
use crate::util::FxHashMap;
use crate::types::NativeTableKey;
use crate::util::FxHashSet as HashSet;
use super::native_globals::{NativeEnvironmentId, NativeGlobalBindingId, NativeGlobalSourceRole};
use super::native_intrinsics::{NativeIntrinsicKind, NativeIntrinsicSite};
use crate::types::NativeFunctionId;

#[path = "native_alias_loops.rs"]
mod loops;
#[path = "native_standard_assumptions.rs"]
mod standard_assumptions;

#[derive(Clone, Copy, Debug)]
pub enum NativeAliasValue {
    Reference { reference: NativeReferenceKey, input: NativeFlowKey },
    Allocation,
    Function { function: NativeFunctionId, creation: NativeFlowKey },
    Call { callee: NativeValueKey, input: NativeFlowKey },
    Projection { value: NativeValueKey, input: NativeFlowKey, index: usize },
    /// An original number, string or boolean value: no object identity, but
    /// unlike nil its raw table slot is present even when the value is false.
    NonNilScalar,
    NotTable,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub struct NativeAliasReturnValue { pub value: NativeValueKey, pub input: NativeFlowKey }
/// An original fixed member read and its candidate physical environment slot.
/// The native worker must prove the receiver before using the candidate slot.
#[derive(Clone, Copy, Debug)]
pub struct NativeAliasEnvironmentMember {
    pub receiver: NativeAliasReturnValue,
    pub reference: NativeReferenceKey,
    pub input: NativeFlowKey,
}
#[derive(Clone, Debug)]
pub struct NativeAliasReturn {
    pub at: NativeFlowKey,
    pub values: Vec<NativeAliasReturnValue>,
    pub expands_last: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeAliasRelation { Same, Different, Possible }

#[derive(Clone, Copy, Debug)]
pub enum NativeAliasInitial { Opaque, GlobalValue(NativeGlobalBindingId), Environment(NativeEnvironmentId) }
#[derive(Clone, Debug)]
pub struct NativeAliasExecution { pub entry: NativeFlowKey, pub terminals: Vec<NativeFlowKey> }
#[derive(Clone, Debug)]
pub struct NativeAliasCall {
    pub callee: NativeAliasReturnValue,
    pub arguments: Vec<NativeAliasReturnValue>,
    /// Actual invocation starts after all operand evaluations.
    pub execution_input: NativeFlowKey,
}
#[derive(Clone, Copy, Debug)]
pub enum NativeAliasDispatch { NoDispatch, ReferenceRead(NativeReferenceKey), Unknown }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCallPreservation { Preserved, Unknown, Pending }
pub(super) enum NativeReachedCallee {
    Function { function: NativeFunctionId, creation: NativeFlowKey, activation: u64, call: NativeAliasCall },
    Unsupported,
    Pending,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeAliasIntrinsic { Standard(NativeIntrinsicSite), Unknown, Pending }
/// Ordinary language operation under the selected host policy. This is not
/// runtime identity evidence for heap, call-preservation, or metatable proofs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStandardOperation { Standard(NativeIntrinsicSite), Ordinary, Pending }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Origin {
    Function(NativeFunctionId, NativeFlowKey, u64),
    Standard(NativeIntrinsicSite),
    Environment(NativeEnvironmentId),
    Allocation(NativeValueKey, NativeFlowKey),
    OpaqueReference(NativeReferenceKey, NativeFlowKey),
    OpaqueProjection(NativeReferenceKey, NativeFlowKey, Vec<NativeTableKey>),
    OpaqueValue(NativeValueKey, NativeFlowKey, usize),
    CallAllocation(NativeValueKey, NativeFlowKey, Box<Origin>),
    CallOpaque(NativeValueKey, NativeFlowKey, Box<Origin>),
}
#[derive(Clone, Default)]
struct Origins { tokens: HashSet<Origin>, unknown: bool, non_table: bool, uncertain: bool, maybe_nil: bool }

/// Completed provenance facts belong to the checker that owns their query
/// dependencies. Flow epochs separate speculative effects from stable inputs.
#[derive(Default)]
pub(super) struct NativeAliasCache {
    references: FxHashMap<(NativeReferenceKey, NativeFlowKey, u64, u64), Origins>,
    standard_references: FxHashMap<(NativeReferenceKey, NativeFlowKey, u64, u64), Origins>,
    standard_loaders: FxHashMap<(NativeReferenceKey, NativeFlowKey, u64, u64), NativeFlowResult>,
    standard_values: FxHashMap<(NativeValueKey, NativeFlowKey, u64, u64), Origins>,
    values: FxHashMap<(NativeValueKey, NativeFlowKey, usize, u64, u64), Origins>,
    // These are interned query paths, not identities of invented source nodes.
    paths: Vec<Vec<NativeTableKey>>,
    path_ids: FxHashMap<Vec<NativeTableKey>, u32>,
    projected_references: FxHashMap<(NativeReferenceKey, NativeFlowKey, u32, u64, u64), Origins>,
    projected_values: FxHashMap<(NativeValueKey, NativeFlowKey, usize, u32, u64, u64), Origins>,
    preservations: FxHashMap<(NativeReferenceKey, NativeValueKey, NativeFlowKey, u64, u64), NativeCallPreservation>,
    metatable_absence: FxHashMap<(NativeValueKey, NativeFlowKey, NativeFlowKey, u64, u64), bool>,
    newindex_absence: FxHashMap<(NativeValueKey, NativeFlowKey, NativeFlowKey, u64, u64), bool>,
    callable_tables: FxHashMap<(NativeValueKey, NativeFlowKey, NativeFlowKey, u64, u64), Option<AliasCallableTable>>,
    execution_ids: FxHashMap<(NativeValueKey, NativeFlowKey, NativeFlowKey, u64), u64>,
    executions: Vec<AliasExecutionFrame>,
}
#[derive(Clone, Copy)]
struct AliasExecutionFrame { id: u64, entry: NativeFlowKey, caller: NativeFlowKey, function: Option<(NativeFunctionId, NativeFlowKey, u64)> }
#[derive(Clone, Copy)]
struct AliasCallableTable {
    callee: NativeAliasReturnValue,
    function: NativeFunctionId,
    creation: NativeFlowKey,
    activation: u64,
}
#[derive(Clone, Copy)]
enum AliasMetatableCall { Preserved, Replaced(NativeAliasReturnValue), Unknown, Pending }
impl NativeAliasCache {
    fn reference(&mut self, key: (NativeReferenceKey, NativeFlowKey, u64, u64), result: Origins, _: Stored) { self.references.insert(key, result); }
    fn value(&mut self, key: (NativeValueKey, NativeFlowKey, usize, u64, u64), result: Origins, _: Stored) { self.values.insert(key, result); }
    fn path(&mut self, path: &[NativeTableKey]) -> u32 {
        if let Some(&id) = self.path_ids.get(path) { return id }
        let id = self.paths.len().try_into().expect("native alias path identities fit u32");
        self.paths.push(path.to_vec());
        self.path_ids.insert(path.to_vec(), id);
        id
    }
    fn projected_reference(&mut self, key: (NativeReferenceKey, NativeFlowKey, u32, u64, u64), result: Origins, _: Stored) { self.projected_references.insert(key, result); }
    fn projected_value(&mut self, key: (NativeValueKey, NativeFlowKey, usize, u32, u64, u64), result: Origins, _: Stored) { self.projected_values.insert(key, result); }
    fn frame(&self) -> u64 { self.executions.last().map_or(0, |frame| frame.id) }
}
impl Origins {
    fn unknown() -> Self { Self { unknown: true, uncertain: true, ..Self::default() } }
    /// Completed original metadata can leave runtime provenance unsupported
    /// without depending on an unresolved or recursive checker query.
    fn unavailable() -> Self { Self { unknown: true, ..Self::default() } }
    fn non_table() -> Self { Self { non_table: true, maybe_nil: true, ..Self::default() } }
    fn non_nil_scalar() -> Self { Self { non_table: true, ..Self::default() } }
    fn one(token: Origin, unknown: bool) -> Self { Self { tokens: HashSet::from_iter([token]), unknown, non_table: false, uncertain: false, maybe_nil: false } }
    fn merge(&mut self, other: Self) { self.tokens.extend(other.tokens); self.unknown |= other.unknown; self.non_table |= other.non_table; self.uncertain |= other.uncertain; self.maybe_nil |= other.maybe_nil; }
    fn relation(&self, other: &Self) -> NativeAliasRelation {
        if self.tokens.is_empty() && !self.unknown || other.tokens.is_empty() && !other.unknown { return NativeAliasRelation::Different }
        if self.tokens.len() == 1 && self.tokens == other.tokens && !self.non_table && !other.non_table && !self.uncertain && !other.uncertain { return NativeAliasRelation::Same }
        if !self.unknown && !other.unknown && self.tokens.is_disjoint(&other.tokens) { NativeAliasRelation::Different } else { NativeAliasRelation::Possible }
    }
}
impl Origin {
    fn allocation_input(&self) -> Option<NativeFlowKey> {
        match self { Self::Allocation(_, input) | Self::CallAllocation(_, input, _) => Some(*input), _ => None }
    }
}

impl<'p, 's> Checker<'p, 's> {
    pub(super) fn native_alias_execution_key(&self) -> u64 { self.native_alias_cache.frame() }
    pub(super) fn native_alias_execution_depth(&self) -> usize { self.native_alias_cache.executions.len() }
    /// Selects an original callee only within the caller-state replay capability.
    pub(super) fn native_reached_callee(&mut self, provider: &dyn NativeFlowProvider, call: NativeValueKey, input: NativeFlowKey) -> NativeReachedCallee {
        let Some(recipe) = provider.alias_call_source(call, input) else { return NativeReachedCallee::Unsupported };
        // Formal/actual substitution is unsupported. Reject the original
        // argument recipe before asking for physical callee provenance.
        if !recipe.arguments.is_empty() { return NativeReachedCallee::Unsupported }
        if recipe.execution_input.graph != input.graph || provider.node(recipe.execution_input).is_none() { return NativeReachedCallee::Unsupported }
        let origins = self.native_alias_value(provider, recipe.callee.value, recipe.callee.input, 0);
        if origins.uncertain { return NativeReachedCallee::Pending }
        if origins.unknown || origins.non_table || origins.tokens.len() != 1 { return NativeReachedCallee::Unsupported }
        let Origin::Function(function, creation, activation) = origins.tokens.into_iter().next().unwrap() else { return NativeReachedCallee::Unsupported };
        // The first transfer capability covers only a closure invoked in its
        // actual creating activation. Escaped closures need a separate proof.
        if creation.graph != recipe.execution_input.graph
            || activation != 0 && !self.native_alias_cache.executions.iter().any(|frame| frame.id == activation)
        { return NativeReachedCallee::Unsupported }
        if self.native_alias_cache.executions.iter().any(|frame| frame.function.is_some_and(|active| active.0 == function)) { return NativeReachedCallee::Pending }
        NativeReachedCallee::Function { function, creation, activation, call: recipe }
    }
    fn native_alias_stable_cell(&self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey) -> bool {
        provider.reference_is_cell(reference) && provider.reference_declaration(reference).is_some_and(|declaration| declaration.writes.iter().all(|write| write.owner == declaration.owner))
    }
    pub fn native_flow_alias_intrinsic(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> NativeAliasIntrinsic {
        let provider = self.native_function_provider.and_then(|provider| provider.project_flow_provider()).unwrap_or(provider);
        let origins = self.native_alias_value(provider, value, input, 0);
        if origins.uncertain { self.mark_tainted_from(0); return NativeAliasIntrinsic::Pending }
        if origins.unknown || origins.non_table || origins.tokens.len() != 1 { return NativeAliasIntrinsic::Unknown }
        match origins.tokens.into_iter().next().unwrap() {
            Origin::Standard(site) => NativeAliasIntrinsic::Standard(site),
            _ => NativeAliasIntrinsic::Unknown,
        }
    }

    pub fn native_flow_call_preservation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey) -> NativeCallPreservation {
        let provider = self.native_function_provider.and_then(|provider| provider.project_flow_provider()).unwrap_or(provider);
        self.native_alias_call_preservation(provider, reference, call, input)
    }

    fn native_alias_initial(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey) -> Origins {
        let opaque = || Origins::one(Origin::OpaqueReference(reference, at), true);
        let Some(globals) = self.native_globals_provider else { return opaque() };
        match provider.reference_alias_initial(reference) {
            NativeAliasInitial::Opaque => opaque(),
            NativeAliasInitial::Environment(environment) => {
                if globals.contains_global_environment(environment) { Origins::one(Origin::Environment(environment), false) } else { opaque() }
            },
            NativeAliasInitial::GlobalValue(binding) => {
                if !globals.contains_global_environment(binding.environment) || !globals.contains_global_binding(binding) { return opaque() }
                if globals.global_environment_self_binding(binding.environment) == Some(binding) {
                    return Origins::one(Origin::Environment(binding.environment), false);
                }
                let Some(functions) = self.native_function_provider else { return opaque() };
                let declarations = globals.global_binding_declarations(binding);
                if declarations.is_empty() { return opaque() }
                let mut selected = None;
                for declaration in declarations {
                    if !globals.contains_value_declaration(declaration) { return opaque() }
                    // Implementation owners execute at their original writes;
                    // their mere future presence cannot change the initial library.
                    if globals.value_declaration_header(declaration).role != NativeGlobalSourceRole::Declaration { continue }
                    let Some(heads) = globals.value_declaration_function_signatures(declaration).filter(|heads| !heads.is_empty()) else { return opaque() };
                    for head in heads {
                        if !functions.contains_signature(head) { return opaque() }
                        let Some(site) = functions.signature_intrinsic(head) else { return opaque() };
                        if site.binding != binding || site.declaration != declaration || selected.is_some_and(|prior| prior != site) { return opaque() }
                        selected = Some(site);
                    }
                }
                selected.map_or_else(opaque, |site| Origins::one(Origin::Standard(site), false))
            },
        }
    }

    pub(super) fn with_native_alias_parent<R>(&mut self, index: usize, work: impl FnOnce(&mut Self) -> R) -> R {
        let outside = self.native_alias_cache.executions.split_off(index);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(self)));
        self.native_alias_cache.executions.extend(outside);
        result.unwrap_or_else(|error| std::panic::resume_unwind(error))
    }

    pub(super) fn with_native_alias_execution<R>(&mut self, call: NativeValueKey, caller: NativeFlowKey, execution: &NativeAliasExecution, function: Option<(NativeFunctionId, NativeFlowKey, u64)>, work: impl FnOnce(&mut Self) -> R) -> R {
        let parent = self.native_alias_cache.frame();
        let next = self.native_alias_cache.execution_ids.len() as u64 + 1;
        let id = *self.native_alias_cache.execution_ids.entry((call, caller, execution.entry, parent)).or_insert(next);
        self.native_alias_cache.executions.push(AliasExecutionFrame { id, entry: execution.entry, caller, function });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(self)));
        self.native_alias_cache.executions.pop();
        result.unwrap_or_else(|error| std::panic::resume_unwind(error))
    }

    pub(super) fn native_alias_dispatch_safe(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, before: NativeFlowKey) -> bool {
        match provider.alias_node_dispatch(at) {
            NativeAliasDispatch::NoDispatch => true,
            NativeAliasDispatch::Unknown => false,
            NativeAliasDispatch::ReferenceRead(reference) => {
                if reference.graph != at.graph { return false }
                if provider.reference_is_cell(reference) { return true }
                let origins = self.native_alias_reference(provider, reference, before);
                if origins.uncertain {
                    // A captured future slot may have no original spelling
                    // in the caller. Its contents are unnecessary when the
                    // independent allocation proof excludes __newindex.
                    if self.native_alias_fresh_raw_write(provider, at, before)
                        || self.native_alias_fresh_raw_read(provider, at, before) { return true }
                    self.mark_tainted_from(0);
                    return false;
                }
                if !origins.unknown && !origins.maybe_nil && (origins.non_table || !origins.tokens.is_empty()) { return true }
                self.native_alias_fresh_raw_write(provider, at, before)
                    || self.native_alias_fresh_raw_read(provider, at, before)
            },
        }
    }

    /// The read uses its original evaluated receiver, including when a later
    /// computed key rebinds that receiver's source variable. Only absence of a
    /// metatable proves a missing raw slot cannot invoke __index; absence of
    /// __newindex is a different capability used exclusively for writes.
    fn native_alias_fresh_raw_read(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, before: NativeFlowKey) -> bool {
        let Some(receiver) = provider.read_alias_receiver(at) else { return false };
        if receiver.input.graph != before.graph { return false }
        let origins = self.native_alias_value(provider, receiver.value, receiver.input, 0);
        if origins.uncertain { self.mark_tainted_from(0); return false }
        if origins.unknown || origins.non_table || origins.tokens.is_empty() { return false }
        origins.tokens.iter().all(|allocation| self.native_alias_allocation_without_metatable(provider, allocation, before))
    }

    /// A literal Lua allocation starts without a metatable. Prove that its
    /// current write cannot invoke __newindex using original execution facts,
    /// not the receiver's structural type or allocation age alone.
    fn native_alias_fresh_raw_write(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, before: NativeFlowKey) -> bool {
        if !matches!(provider.node(at), Some(NativeFlowNode::Write { .. })) { return false }
        let Some(receiver) = provider.write_alias_receiver(at) else { return false };
        if receiver.input.graph != before.graph { return false }
        let origins = self.native_alias_value(provider, receiver.value, receiver.input, 0);
        if origins.uncertain { self.mark_tainted_from(0); return false }
        if origins.unknown || origins.non_table || origins.tokens.is_empty() { return false }
        // Several reached allocations do not identify one receiver. They can
        // nevertheless all be metatable-free, proving only that the store has
        // no implicit dispatch; alias relations retain their alternatives.
        origins.tokens.iter().all(|allocation| {
            self.native_alias_allocation_without_metatable(provider, allocation, before)
                || self.native_alias_allocation_without_newindex(provider, allocation, before)
        })
    }

    /// A reached literal metatable can lack __newindex while still supplying
    /// __call. Keep this distinct from absence of a metatable altogether.
    fn native_alias_allocation_without_newindex(&mut self, provider: &dyn NativeFlowProvider, allocation: &Origin, before: NativeFlowKey) -> bool {
        let Origin::Allocation(value, input) = allocation else { return false };
        if !provider.contains_graph(before.graph) || provider.node(*input).is_none() { return false }
        let epoch = self.flow_memo.native_state.effect_epoch;
        let frame = self.native_alias_cache.frame();
        let key = (*value, *input, before, epoch, frame);
        if let Some(&absent) = self.native_alias_cache.newindex_absence.get(&key) { return absent }
        let query = Query::NativeAliasNewIndexAbsence(*value, *input, before, epoch, frame);
        if !self.enter(query) { self.mark_tainted_from(0); return false }
        let (absent, completed) = self.run_memoizable(|checker| checker.native_alias_newindex_absent_worker(provider, allocation, *input, before));
        if self.leave(query).is_err() || completed.is_none() { self.mark_tainted_from(0); return false }
        if !self.is_stack_low() { self.native_alias_cache.newindex_absence.insert(key, absent); }
        absent
    }

    fn native_alias_newindex_absent_worker(&mut self, provider: &dyn NativeFlowProvider, allocation: &Origin, input: NativeFlowKey, before: NativeFlowKey) -> bool {
        let mut active = HashSet::default();
        let mut completed = HashSet::default();
        let mut work = vec![(before, false)];
        let mut steps = 0usize;
        while let Some((at, leaving)) = work.pop() {
            steps += 1;
            if steps > 16_384 || at.graph != before.graph || self.is_stack_low() { return false }
            if at == input { continue }
            if leaving { active.remove(&at); completed.insert(at); continue }
            if completed.contains(&at) { continue }
            if !active.insert(at) { return false }
            let mut edges = Vec::new();
            match provider.node(at) {
                Some(NativeFlowNode::Start) => {
                    let Some(index) = self.native_alias_cache.executions.iter().rposition(|frame| frame.entry == at) else { return false };
                    let caller = self.native_alias_cache.executions[index].caller;
                    if !self.with_native_alias_parent(index, |checker| checker.native_alias_allocation_without_newindex(provider, allocation, caller)) { return false }
                },
                Some(NativeFlowNode::Write { before, .. } | NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }) => {
                    if !self.native_alias_dispatch_safe(provider, at, before) { return false }
                    edges.push(before);
                },
                Some(NativeFlowNode::Call { before, value, input }) => match self.native_alias_metatable_call(provider, allocation, value, input) {
                    AliasMetatableCall::Preserved => edges.push(before),
                    AliasMetatableCall::Replaced(metatable) => {
                        let key = NativeTableKey::String(self.atoms().intern(b"__newindex"));
                        // The direct literal was not exposed before this
                        // installation. Retrieving it afterward requires a
                        // call; unsupported calls remain barriers below.
                        if !provider.value_alias_member_absent(metatable.value, metatable.input, key, self) { return false }
                    },
                    AliasMetatableCall::Pending => { self.mark_tainted_from(0); return false },
                    AliasMetatableCall::Unknown => return false,
                },
                Some(NativeFlowNode::Merge { edges: incoming }) => {
                    if incoming.is_empty() || !matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) { return false }
                    edges.extend(incoming);
                },
                _ => return false,
            }
            work.push((at, true));
            work.extend(edges.into_iter().rev().map(|edge| (edge, false)));
        }
        true
    }

    fn native_alias_metatable_call(&mut self, provider: &dyn NativeFlowProvider, allocation: &Origin, call: NativeValueKey, input: NativeFlowKey) -> AliasMetatableCall {
        let Origin::Allocation(value, allocated) = allocation else { return AliasMetatableCall::Unknown };
        let query = Query::NativeAliasMetatableCall(*value, *allocated, call, input, self.flow_memo.native_state.effect_epoch, self.native_alias_cache.frame());
        if !self.enter(query) { self.mark_tainted_from(0); return AliasMetatableCall::Pending }
        let (result, completed) = self.run_memoizable(|checker| checker.native_alias_metatable_call_worker(provider, allocation, call, input));
        if self.leave(query).is_err() || completed.is_none() || matches!(result, AliasMetatableCall::Pending) {
            self.mark_tainted_from(0);
            return AliasMetatableCall::Pending;
        }
        result
    }

    fn native_alias_metatable_call_worker(&mut self, provider: &dyn NativeFlowProvider, allocation: &Origin, call: NativeValueKey, input: NativeFlowKey) -> AliasMetatableCall {
        let Some(mut recipe) = provider.alias_call_source(call, input) else { return AliasMetatableCall::Unknown };
        if recipe.execution_input.graph != input.graph || provider.node(recipe.execution_input).is_none() { return AliasMetatableCall::Unknown }
        let callee = self.native_alias_value(provider, recipe.callee.value, recipe.callee.input, 0);
        if callee.uncertain { return AliasMetatableCall::Pending }
        if callee.unknown || callee.non_table || callee.tokens.len() != 1 { return AliasMetatableCall::Unknown }
        let (function, creation, activation) = match callee.tokens.into_iter().next().unwrap() {
            Origin::Standard(site) => return match site.kind {
                NativeIntrinsicKind::Type | NativeIntrinsicKind::Assert => AliasMetatableCall::Preserved,
                NativeIntrinsicKind::SetMetatable => {
                    let Some(receiver) = recipe.arguments.first() else { return AliasMetatableCall::Unknown };
                    let receiver = self.native_alias_value(provider, receiver.value, receiver.input, 0);
                    if receiver.uncertain { return AliasMetatableCall::Pending }
                    if receiver.unknown || receiver.non_table || receiver.tokens.is_empty() { return AliasMetatableCall::Unknown }
                    if !receiver.tokens.contains(allocation) { AliasMetatableCall::Preserved }
                    else if receiver.tokens.len() == 1 {
                        recipe.arguments.get(1).copied().map_or(AliasMetatableCall::Unknown, AliasMetatableCall::Replaced)
                    } else { AliasMetatableCall::Unknown }
                },
                NativeIntrinsicKind::Require => AliasMetatableCall::Unknown,
            },
            Origin::Function(function, creation, activation) => (function, creation, activation),
            target @ Origin::Allocation(..) => {
                let callable = match self.native_alias_callable_table(provider, &target, recipe.execution_input) {
                    Ok(Some(callable)) => callable,
                    Ok(None) => return AliasMetatableCall::Unknown,
                    Err(()) => return AliasMetatableCall::Pending,
                };
                let receiver = recipe.callee;
                recipe.callee = callable.callee;
                recipe.arguments.insert(0, receiver);
                (callable.function, callable.creation, callable.activation)
            },
            _ => return AliasMetatableCall::Unknown,
        };
        if activation != 0 && !self.native_alias_cache.executions.iter().any(|frame| frame.id == activation) { return AliasMetatableCall::Unknown }
        if self.native_alias_cache.executions.iter().any(|frame| frame.function == Some((function, creation, activation))) { return AliasMetatableCall::Pending }
        let Some(execution) = provider.function_alias_execution(function, creation) else { return AliasMetatableCall::Unknown };
        if !provider.contains_graph(execution.entry.graph) || !matches!(provider.node(execution.entry), Some(NativeFlowNode::Start)) || execution.terminals.is_empty() { return AliasMetatableCall::Unknown }
        self.with_native_alias_execution(call, recipe.execution_input, &execution, Some((function, creation, activation)), |checker| {
            let mut active = HashSet::default();
            let mut completed = HashSet::default();
            let mut work: Vec<_> = execution.terminals.iter().rev().map(|&at| (at, false)).collect();
            let mut steps = 0usize;
            while let Some((at, leaving)) = work.pop() {
                steps += 1;
                if steps > 16_384 || at.graph != execution.entry.graph || checker.is_stack_low() { return AliasMetatableCall::Unknown }
                if leaving { active.remove(&at); completed.insert(at); continue }
                if completed.contains(&at) { continue }
                if !active.insert(at) { return AliasMetatableCall::Unknown }
                let mut edges = Vec::new();
                match provider.node(at) {
                    Some(NativeFlowNode::Start) if at == execution.entry => {},
                    Some(NativeFlowNode::Write { before, .. } | NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }) => {
                        if !checker.native_alias_dispatch_safe(provider, at, before) { return AliasMetatableCall::Unknown }
                        edges.push(before);
                    },
                    Some(NativeFlowNode::Call { before, value, input }) => match checker.native_alias_metatable_call(provider, allocation, value, input) {
                        AliasMetatableCall::Preserved => edges.push(before),
                        AliasMetatableCall::Pending => return AliasMetatableCall::Pending,
                        // Object-identity preservation does not attest an
                        // unchanged metatable, even for standard setmetatable.
                        AliasMetatableCall::Replaced(_) | AliasMetatableCall::Unknown => return AliasMetatableCall::Unknown,
                    },
                    Some(NativeFlowNode::Return { before }) => edges.push(before),
                    Some(NativeFlowNode::Merge { edges: incoming }) => {
                        if !matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) { return AliasMetatableCall::Unknown }
                        edges.extend(incoming);
                    },
                    Some(NativeFlowNode::Unreachable) => {},
                    _ => return AliasMetatableCall::Unknown,
                }
                work.push((at, true));
                work.extend(edges.into_iter().rev().map(|edge| (edge, false)));
            }
            AliasMetatableCall::Preserved
        })
    }

    fn native_alias_allocation_without_metatable(&mut self, provider: &dyn NativeFlowProvider, allocation: &Origin, before: NativeFlowKey) -> bool {
        let (value, input) = match allocation { Origin::Allocation(value, input) => (*value, *input), _ => return false };
        if !provider.contains_graph(before.graph) || !provider.contains_graph(input.graph) || provider.node(input).is_none() { return false }
        let epoch = self.flow_memo.native_state.effect_epoch;
        let frame = self.native_alias_cache.frame();
        let key = (value, input, before, epoch, frame);
        if let Some(&absent) = self.native_alias_cache.metatable_absence.get(&key) { return absent }
        let query = Query::NativeAliasMetatableAbsence(value, input, before, epoch, frame);
        if !self.enter(query) { self.mark_tainted_from(0); return false }
        let begun = self.begin_memoizable();
        let absent = self.native_alias_allocation_clear_since(provider, allocation, input, before);
        let stored = self.end_memoizable(begun);
        let left = self.leave(query);
        if stored.is_none() || left.is_err() { self.mark_tainted_from(0); return false }
        if !self.is_stack_low() { self.native_alias_cache.metatable_absence.insert(key, absent); }
        absent
    }

    fn native_alias_allocation_clear_since(&mut self, provider: &dyn NativeFlowProvider, allocation: &Origin, input: NativeFlowKey, before: NativeFlowKey) -> bool {
        let graph = before.graph;
        let mut active = HashSet::default();
        let mut completed = HashSet::default();
        let mut work = vec![(before, false)];
        let mut steps = 0usize;
        while let Some((at, leaving)) = work.pop() {
            steps += 1;
            if steps > 16_384 || at.graph != graph || self.is_stack_low() { return false }
            if at == input { continue }
            if leaving { active.remove(&at); completed.insert(at); continue }
            if completed.contains(&at) { continue }
            if !active.insert(at) { return false }
            let Some(node) = provider.node(at) else { return false };
            let mut edges = Vec::new();
            match node {
                NativeFlowNode::Start => {
                    let Some(index) = self.native_alias_cache.executions.iter().rposition(|frame| frame.entry == at) else { return false };
                    let caller = self.native_alias_cache.executions[index].caller;
                    // A captured allocation is checked at the actual caller's
                    // input, under its parent execution capability. This is the
                    // only graph boundary; arbitrary foreign edges still fail.
                    if !self.with_native_alias_parent(index, |checker| checker.native_alias_allocation_clear_since(provider, allocation, input, caller)) { return false }
                },
                NativeFlowNode::Write { before, .. } => {
                    // The earlier store must independently have a present raw
                    // slot or metatable-free receivers at its own earlier input.
                    // This cannot certify an unknown store merely because it
                    // happens to target a different syntactic path.
                    if !self.native_alias_dispatch_safe(provider, at, before) { return false }
                    edges.push(before);
                },
                NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. } => {
                    // A prior raw member read can be proven non-dispatching
                    // even though its original opcode is ReferenceRead.
                    if !self.native_alias_dispatch_safe(provider, at, before) { return false }
                    edges.push(before);
                },
                NativeFlowNode::Merge { edges: incoming } => {
                    if incoming.is_empty() || !matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) { return false }
                    edges.extend(incoming);
                },
                NativeFlowNode::Call { before, value, input } => {
                    // Preserve absence only when the original call cannot
                    // change this allocation's metatable pointer. Raw-field
                    // preservation alone would also admit setmetatable.
                    match self.native_alias_metatable_call(provider, allocation, value, input) {
                        AliasMetatableCall::Preserved => edges.push(before),
                        AliasMetatableCall::Pending => { self.mark_tainted_from(0); return false },
                        AliasMetatableCall::Replaced(_) | AliasMetatableCall::Unknown => return false,
                    }
                },
                NativeFlowNode::Loop { .. }
                | NativeFlowNode::IteratorStart { .. } | NativeFlowNode::Iteration { .. }
                | NativeFlowNode::Return { .. }
                | NativeFlowNode::Unreachable | NativeFlowNode::Unsupported(_) => return false,
            }
            work.push((at, true));
            work.extend(edges.into_iter().rev().map(|edge| (edge, false)));
        }
        true
    }

    fn native_alias_call_preservation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey) -> NativeCallPreservation {
        if !provider.contains_reference(reference) || !provider.contains_graph(input.graph) || provider.node(input).is_none() || call.snapshot != input.graph.snapshot || self.is_stack_low() { return NativeCallPreservation::Unknown }
        let epoch = self.flow_memo.native_state.effect_epoch;
        let frame = self.native_alias_cache.frame();
        let key = (reference, call, input, epoch, frame);
        if let Some(&result) = self.native_alias_cache.preservations.get(&key) { return result }
        let query = Query::NativeCallPreserves(reference, call, input, epoch, frame);
        if !self.enter(query) { self.mark_tainted_from(0); return NativeCallPreservation::Pending }
        let begun = self.begin_memoizable();
        let result = self.native_alias_call_preservation_uncached(provider, reference, call, input);
        if result == NativeCallPreservation::Pending { self.mark_tainted_from(0); }
        let stored = self.end_memoizable(begun);
        let left = self.leave(query);
        if stored.is_none() || left.is_err() { return NativeCallPreservation::Pending }
        if !self.is_stack_low() { self.native_alias_cache.preservations.insert(key, result); }
        result
    }

    fn native_alias_call_preservation_uncached(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey) -> NativeCallPreservation {
        let Some(recipe) = provider.alias_call_source(call, input) else { return NativeCallPreservation::Unknown };
        if recipe.execution_input.graph != input.graph || provider.node(recipe.execution_input).is_none() { return NativeCallPreservation::Unknown }
        let callee = self.native_alias_value(provider, recipe.callee.value, recipe.callee.input, 0);
        self.native_alias_call_preservation_for_callee(provider, reference, call, input, recipe, callee)
    }

    fn native_alias_call_preservation_for_callee(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, input: NativeFlowKey, recipe: NativeAliasCall, callee: Origins) -> NativeCallPreservation {
        if callee.uncertain { return NativeCallPreservation::Pending }
        if callee.unknown || callee.non_table || callee.tokens.len() != 1 { return NativeCallPreservation::Unknown }
        match callee.tokens.into_iter().next().unwrap() {
            Origin::Standard(site) => match site.kind {
                NativeIntrinsicKind::Type | NativeIntrinsicKind::Assert | NativeIntrinsicKind::SetMetatable => NativeCallPreservation::Preserved,
                NativeIntrinsicKind::Require => {
                    let Some(loader) = provider.require_alias_loader_reference(call, input) else { return NativeCallPreservation::Unknown };
                    let state = self.native_alias_loader_state(provider, loader, recipe.execution_input);
                    if state.incomplete { return NativeCallPreservation::Pending }
                    if state.ty != crate::types::TypeId::TRUE { return NativeCallPreservation::Unknown }
                    let Some(executions) = provider.require_alias_execution(call, input) else { return NativeCallPreservation::Unknown };
                    for execution in executions {
                        let result = self.native_alias_execution_preservation(provider, reference, call, &recipe, &execution, None);
                        if result != NativeCallPreservation::Preserved { return result }
                    }
                    NativeCallPreservation::Preserved
                },
            },
            Origin::Function(function, creation, activation) => self.native_alias_function_preservation(provider, reference, call, &recipe, function, creation, activation),
            allocation @ Origin::Allocation(..) => {
                let callable = match self.native_alias_callable_table(provider, &allocation, recipe.execution_input) {
                    Ok(Some(callable)) => callable,
                    Ok(None) => return NativeCallPreservation::Unknown,
                    Err(()) => return NativeCallPreservation::Pending,
                };
                // Lua passes the frozen table as the first physical __call
                // argument. Keep it even when a later entry-state capability
                // cannot yet transport parameter aliases.
                let receiver = recipe.callee;
                let mut recipe = recipe;
                recipe.callee = callable.callee;
                recipe.arguments.insert(0, receiver);
                self.native_alias_function_preservation(provider, reference, call, &recipe, callable.function, callable.creation, callable.activation)
            },
            _ => NativeCallPreservation::Unknown,
        }
    }

    fn native_alias_function_preservation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, recipe: &NativeAliasCall, function: NativeFunctionId, creation: NativeFlowKey, activation: u64) -> NativeCallPreservation {
        if activation != 0 && !self.native_alias_cache.executions.iter().any(|frame| frame.id == activation) { return NativeCallPreservation::Unknown }
        if self.native_alias_cache.executions.iter().any(|frame| frame.function == Some((function, creation, activation))) { return NativeCallPreservation::Pending }
        let Some(execution) = provider.function_alias_execution(function, creation) else { return NativeCallPreservation::Unknown };
        self.native_alias_execution_preservation(provider, reference, call, recipe, &execution, Some((function, creation, activation)))
    }

    fn native_alias_callable_table(&mut self, provider: &dyn NativeFlowProvider, allocation: &Origin, before: NativeFlowKey) -> Result<Option<AliasCallableTable>, ()> {
        let Origin::Allocation(value, input) = allocation else { return Ok(None) };
        if input.graph != before.graph { return Ok(None) }
        let key = (*value, *input, before, self.flow_memo.native_state.effect_epoch, self.native_alias_cache.frame());
        // The proof is independent of the watched reference whose call
        // continuation requested it. Reuse only a completed result.
        if let Some(&result) = self.native_alias_cache.callable_tables.get(&key) { return Ok(result) }
        let query = Query::NativeAliasCallableTable(key.0, key.1, key.2, key.3, key.4);
        if !self.enter(query) { self.mark_tainted_from(0); return Err(()) }
        let (result, completed) = self.run_memoizable(|checker| checker.native_alias_callable_table_worker(provider, allocation, *input, before));
        if self.leave(query).is_err() || completed.is_none() || result.is_err() {
            self.mark_tainted_from(0);
            return Err(());
        }
        if let Ok(result) = result && !self.is_stack_low() { self.native_alias_cache.callable_tables.insert(key, result); }
        result
    }

    /// Find the reached physical metamethod, rather than trusting a callable
    /// type or a future source-level setmetatable candidate. This first proof
    /// accepts a direct literal metatable and no intervening heap effects.
    fn native_alias_callable_table_worker(&mut self, provider: &dyn NativeFlowProvider, allocation: &Origin, input: NativeFlowKey, mut at: NativeFlowKey) -> Result<Option<AliasCallableTable>, ()> {
        let mut visited = HashSet::default();
        while at != input {
            if at.graph != input.graph || visited.len() >= 16_384 || !visited.insert(at) || self.is_stack_low() { return Ok(None) }
            match provider.node(at) {
                Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. })
                    if matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) => at = before,
                Some(NativeFlowNode::Write { before, target, .. })
                    if provider.reference_is_cell(target) && matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) => at = before,
                Some(NativeFlowNode::Call { before, value, input: call_input }) => {
                    let Some(recipe) = provider.alias_call_source(value, call_input) else { return Ok(None) };
                    if recipe.execution_input != before || recipe.arguments.len() < 2 { return Ok(None) }
                    let metatable = recipe.arguments[1];
                    // Recover only a field of the original literal itself.
                    // A mutable alias needs a separate reached-heap proof. This
                    // structural requirement precedes the physical callee and
                    // receiver proofs: without it no callable table is reached.
                    let key = NativeTableKey::String(self.atoms().intern(b"__call"));
                    let Some(callee) = provider.value_alias_member_source(metatable.value, metatable.input, key, self) else { return Ok(None) };
                    let setmetatable = self.native_alias_value(provider, recipe.callee.value, recipe.callee.input, 0);
                    if setmetatable.uncertain { return Err(()) }
                    if setmetatable.unknown || setmetatable.non_table || setmetatable.tokens.len() != 1 { return Ok(None) }
                    if !matches!(setmetatable.tokens.into_iter().next(), Some(Origin::Standard(site)) if site.kind == NativeIntrinsicKind::SetMetatable) { return Ok(None) }
                    let receiver = recipe.arguments[0];
                    let receiver = self.native_alias_value(provider, receiver.value, receiver.input, 0);
                    if receiver.uncertain { return Err(()) }
                    if receiver.unknown || receiver.non_table || receiver.tokens.len() != 1 || !receiver.tokens.contains(allocation) { return Ok(None) }
                    let function = self.native_alias_value(provider, callee.value, callee.input, 0);
                    if function.uncertain { return Err(()) }
                    if function.unknown || function.non_table || function.tokens.len() != 1 { return Ok(None) }
                    let Some(Origin::Function(function, creation, activation)) = function.tokens.into_iter().next() else { return Ok(None) };
                    return Ok(Some(AliasCallableTable { callee, function, creation, activation }));
                },
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    fn native_alias_loader_state(&mut self, provider: &dyn NativeFlowProvider, loader: NativeReferenceKey, input: NativeFlowKey) -> NativeFlowResult {
        use crate::types::TypeId;
        let unknown = NativeFlowResult { ty: TypeId::FALSE, incomplete: false, unresolved_auto: false };
        if loader.graph != input.graph || !provider.contains_reference(loader) || !provider.contains_graph(input.graph) || provider.node(input).is_none() { return unknown }
        // Cyclic module execution can create fresh alias-execution frames for
        // the same original loader input. Guard that original dependency,
        // rather than treating every new execution frame as independent work.
        let query = Query::NativeAliasLoaderState(loader, input);
        if !self.enter(query) {
            self.mark_tainted_from(0);
            return NativeFlowResult { incomplete: true, ..unknown };
        }
        let (mut result, completed) = self.run_memoizable(|checker| checker.native_alias_loader_state_worker(provider, loader, input));
        if self.leave(query).is_err() || completed.is_none() || result.incomplete {
            self.mark_tainted_from(0);
            result.incomplete = true;
        }
        result
    }

    fn native_alias_loader_state_worker(&mut self, provider: &dyn NativeFlowProvider, loader: NativeReferenceKey, input: NativeFlowKey) -> NativeFlowResult {
        use crate::types::TypeId;
        let unknown = NativeFlowResult { ty: TypeId::FALSE, incomplete: false, unresolved_auto: false };
        let initial = if let Some(index) = self.native_alias_cache.executions.iter().rposition(|frame| frame.entry.graph == input.graph) {
            let frame = self.native_alias_cache.executions[index];
            let Some(parent) = provider.reference_in_graph(loader, frame.caller.graph) else { return unknown };
            let parent = self.with_native_alias_parent(index, |checker| checker.native_alias_loader_state(provider, parent, frame.caller));
            if parent.incomplete { return parent }
            parent.ty
        } else { TypeId::TRUE };
        self.native_flow_type(provider, input, loader, TypeId::FALSE, initial)
    }

    fn native_alias_execution_preservation(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, call: NativeValueKey, recipe: &NativeAliasCall, execution: &NativeAliasExecution, function: Option<(NativeFunctionId, NativeFlowKey, u64)>) -> NativeCallPreservation {
        if !provider.contains_graph(execution.entry.graph) || !matches!(provider.node(execution.entry), Some(NativeFlowNode::Start)) || execution.terminals.is_empty() { return NativeCallPreservation::Unknown }
        let Some(watched) = provider.reference_in_graph(reference, execution.entry.graph) else { return NativeCallPreservation::Unknown };
        if !provider.contains_reference(watched) { return NativeCallPreservation::Unknown }
        self.with_native_alias_execution(call, recipe.execution_input, execution, function, |checker| {
            let mut active = HashSet::default();
            let mut completed = HashSet::default();
            let mut work: Vec<_> = execution.terminals.iter().rev().map(|&at| (at, false)).collect();
            let mut steps = 0usize;
            while let Some((at, leaving)) = work.pop() {
                steps += 1;
                if steps > 16_384 || at.graph != execution.entry.graph || checker.is_stack_low() { return NativeCallPreservation::Unknown }
                if leaving { active.remove(&at); completed.insert(at); continue }
                if completed.contains(&at) { continue }
                if !active.insert(at) { return NativeCallPreservation::Unknown }
                let Some(node) = provider.node(at) else { return NativeCallPreservation::Unknown };
                let mut edges = Vec::new();
                match node {
                    NativeFlowNode::Start if at == execution.entry => {},
                    NativeFlowNode::Write { before, target, .. } => {
                        if !checker.native_alias_dispatch_safe(provider, at, before) { return NativeCallPreservation::Unknown }
                        let local = provider.reference_is_cell(target) && provider.reference_declaration(target).is_some_and(|declaration| declaration.owner == execution.entry.graph);
                        if !local && provider.write_affects_with_types(watched, target, at, checker) != NativeWriteRelation::Unaffected { return NativeCallPreservation::Unknown }
                        edges.push(before);
                    },
                    NativeFlowNode::Call { before, value, input } => {
                        let result = checker.native_alias_call_preservation(provider, watched, value, input);
                        if result != NativeCallPreservation::Preserved { return result }
                        edges.push(before);
                    },
                    NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. } | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. } => {
                        if !checker.native_alias_dispatch_safe(provider, at, before) { return NativeCallPreservation::Unknown }
                        edges.push(before);
                    },
                    NativeFlowNode::Return { before } => edges.push(before),
                    NativeFlowNode::Merge { edges: incoming } => {
                        if !matches!(provider.alias_node_dispatch(at), NativeAliasDispatch::NoDispatch) { return NativeCallPreservation::Unknown }
                        edges.extend(incoming);
                    },
                    NativeFlowNode::Unreachable => {},
                    _ => return NativeCallPreservation::Unknown,
                }
                work.push((at, true));
                work.extend(edges.into_iter().rev().map(|edge| (edge, false)));
            }
            NativeCallPreservation::Preserved
        })
    }

    pub fn native_flow_alias_relation(&mut self, provider: &dyn NativeFlowProvider, left: NativeReferenceKey, left_at: NativeFlowKey, right: NativeValueKey, right_at: NativeFlowKey) -> NativeAliasRelation {
        let provider = self.native_function_provider.and_then(|provider| provider.project_flow_provider()).unwrap_or(provider);
        let left_contract = provider.reference_written_contract(left, self);
        if left_contract.is_some_and(|contract| self.native_alias_written_non_table(contract)) { return NativeAliasRelation::Different }
        let right_contract = if let NativeAliasValue::Reference { reference, .. } = provider.value_alias_source(right, right_at) { provider.reference_written_contract(reference, self) } else { None };
        if right_contract.is_some_and(|contract| self.native_alias_written_non_table(contract)) { return NativeAliasRelation::Different }
        if let (Some(left), Some(right)) = (left_contract, right_contract)
            && self.native_alias_written_disjoint(left, right, &mut HashSet::default())
        { return NativeAliasRelation::Different }
        let left = self.native_alias_reference(provider, left, left_at);
        let right = self.native_alias_value(provider, right, right_at, 0);
        left.relation(&right)
    }

    /// A structural call signature can describe a callable Lua table. Only
    /// physical scalar domains and the explicit raw function top exclude tables.
    fn native_alias_written_non_table(&self, ty: crate::types::TypeId) -> bool {
        if self.is_union(ty) { return self.parts(ty).iter().all(|&part| self.native_alias_written_non_table(part)) }
        if self.is_instantiable(ty) || self.is_intersection(ty) { return false }
        ty.is_never() || self.is_primitive(ty) || matches!(self.data(ty), crate::types::TypeData::NativeRuntimeTop { kind: crate::types::NativeRuntimeKind::Function, .. })
    }

    /// Missing requirements do not make structural objects disjoint: one
    /// physical object can have both sets of fields. A proof needs incompatible
    /// runtime/scalar kinds, or conflicting bounds on a required common field.
    fn native_alias_written_disjoint(&mut self, left: crate::types::TypeId, right: crate::types::TypeId, seen: &mut HashSet<(crate::types::TypeId, crate::types::TypeId)>) -> bool {
        if left == right || !seen.insert((left, right)) { return false }
        if self.is_instantiable(left) || self.is_instantiable(right) || self.is_intersection(left) || self.is_intersection(right) { return false }
        if self.is_union(left) { return self.parts(left).iter().all(|&part| self.native_alias_written_disjoint(part, right, &mut seen.clone())) }
        if self.is_union(right) { return self.parts(right).iter().all(|&part| self.native_alias_written_disjoint(left, part, &mut seen.clone())) }
        if self.are_comparable(left, right) { return false }
        if !self.is_object_type(left) || !self.is_object_type(right) { return self.native_alias_written_non_table(left) || self.native_alias_written_non_table(right) }
        if let (crate::types::TypeData::NativeRuntimeTop { kind: left, .. }, crate::types::TypeData::NativeRuntimeTop { kind: right, .. }) = (self.data(left), self.data(right))
            && left != right { return true }
        for (contract, other) in [(left, right), (right, left)] {
            let Some(description) = self.native_table_object_description(contract) else { continue };
            for field in &description.fields {
                if field.optional || self.contains_undefined(field.value) { continue }
                let Ok(other) = self.native_table_read(other, crate::types::NativeTableAccessKey::Literal(field.key)) else { continue };
                if self.native_alias_written_disjoint(field.value, other, &mut seen.clone()) { return true }
            }
        }
        false
    }

    fn native_alias_reference(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey) -> Origins {
        if reference.graph != at.graph || !provider.contains_graph(at.graph) || !provider.contains_reference(reference) { return Origins::unknown() }
        let frame = self.native_alias_cache.frame();
        let key = (reference, at, self.flow_memo.native_state.effect_epoch, frame);
        if let Some(result) = self.native_alias_cache.references.get(&key) { return result.clone() }
        let query = Query::NativeAlias(reference, at, frame);
        if !self.enter(query) { self.mark_tainted_from(0); return Origins::unknown() }
        let begun = self.begin_memoizable();
        let mut result = match provider.node(at) {
            Some(NativeFlowNode::Start) => {
                let declaration = provider.reference_declaration(reference);
                let execution = self.native_alias_cache.executions.iter().rposition(|frame| frame.entry == at);
                let own_cell = provider.reference_is_cell(reference) && declaration.as_ref().is_some_and(|declaration| declaration.owner == at.graph);
                if let Some(index) = execution.filter(|_| !own_cell) {
                    let frame = self.native_alias_cache.executions[index];
                    if let Some(caller) = provider.reference_in_graph(reference, frame.caller.graph) {
                        self.with_native_alias_parent(index, |checker| checker.native_alias_reference(provider, caller, frame.caller))
                    } else { Origins::unknown() }
                } else if let Some(declaration) = declaration
                    && declaration.owner != reference.graph
                    && let Some(creation) = provider.graph_creation(reference.graph)
                    // A captured local from another function activation needs
                    // that activation's allocation provenance. Syntax/typed
                    // capture facets alone cannot identify its physical object.
                    && provider.graph_creation(declaration.owner).is_none()
                    && !declaration.writes.iter().any(|write| write.owner != declaration.owner || write.statement_end > creation.position)
                    && let Some(outer) = provider.reference_in_graph(reference, creation.input.graph)
                { self.native_alias_reference(provider, outer, creation.input) }
                else { self.native_alias_initial(provider, reference, at) }
            }
            Some(NativeFlowNode::Write { before, .. }) if !self.native_alias_stable_cell(provider, reference) && !self.native_alias_dispatch_safe(provider, at, before) => Origins::one(Origin::OpaqueReference(reference, at), true),
            Some(NativeFlowNode::Write { before, target, value, input }) => match if provider.reference_is_cell(reference) {
                // Heap stores cannot rebind a lexical cell. Resolve only the
                // cell's original assignments; classifying heap aliases here
                // recursively asks for the identity we are already tracing.
                provider.write_affects(reference, target, at)
            } else { provider.write_affects_ignoring_descendants(reference, target, at, self) } {
                NativeWriteRelation::Exact => self.native_alias_value(provider, value, input, 0),
                NativeWriteRelation::PossibleExact => {
                    let mut previous = self.native_alias_reference(provider, reference, before);
                    previous.merge(self.native_alias_value(provider, value, input, 0));
                    previous
                }
                NativeWriteRelation::Ancestor => {
                    match provider.reference_member_path(target, reference, self).filter(|path| !path.is_empty()) {
                        Some(path) => self.native_alias_projected_value(provider, value, input, 0, &path),
                        None => Origins::unavailable(),
                    }
                },
                // A completed write relation, not an unfinished dependency: the
                // possibly replaced ancestor leaves provenance unsupported. An
                // uncertain result here is never cached, and every walk through
                // later stores repeats it (exponential in straight-line stores).
                NativeWriteRelation::PossibleAncestor => Origins::unavailable(),
                _ => self.native_alias_reference(provider, reference, before),
            },
            Some(NativeFlowNode::Call { before, value, input }) => {
                let stable_cell = self.native_alias_stable_cell(provider, reference);
                let preservation = if stable_cell && !provider.call_invalidates_reference(reference, at, self) {
                    NativeCallPreservation::Preserved
                } else { self.native_alias_call_preservation(provider, reference, value, input) };
                if preservation == NativeCallPreservation::Preserved {
                    self.native_alias_reference(provider, reference, before)
                } else if preservation == NativeCallPreservation::Pending { Origins::unknown() }
                else { Origins::one(Origin::OpaqueReference(reference, at), true) }
            }
            Some(NativeFlowNode::Merge { edges }) => {
                let mut origins = Origins::default();
                for edge in edges { origins.merge(self.native_alias_reference(provider, reference, edge)); }
                origins
            }
            // A static allocation site on a backedge can denote another
            // physical object. Never equate iterations through a cycle token.
            // This is a completed limit of the provenance proof, not an
            // unresolved query. The memo scope below still propagates any
            // actual recursive dependency as uncertain.
            Some(NativeFlowNode::Loop { edges }) => self.native_alias_loop_reference(provider, reference, at, &edges),
            Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }) => {
                if self.native_alias_stable_cell(provider, reference) || self.native_alias_dispatch_safe(provider, at, before) { self.native_alias_reference(provider, reference, before) }
                else { Origins::one(Origin::OpaqueReference(reference, at), true) }
            },
            Some(NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. }) => {
                if self.native_alias_dispatch_safe(provider, at, before) { self.native_alias_reference(provider, reference, before) }
                else { Origins::one(Origin::OpaqueReference(reference, at), true) }
            },
            Some(NativeFlowNode::Unreachable | NativeFlowNode::Return { .. }) => Origins::non_table(),
            _ => Origins::unknown(),
        };
        let memoizable = self.end_memoizable(begun);
        match self.leave(query) {
            Ok(stored) if memoizable.is_some() && !result.uncertain && !self.is_stack_low() => self.native_alias_cache.reference(key, result.clone(), stored),
            Ok(_) if memoizable.is_some() => {},
            _ => { result.unknown = true; result.uncertain = true; self.mark_tainted_from(0); },
        }
        result
    }

    fn native_alias_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey, column: usize) -> Origins {
        if value.snapshot != input.graph.snapshot || !provider.contains_graph(input.graph) || provider.node(input).is_none() { return Origins::unknown() }
        let frame = self.native_alias_cache.frame();
        let key = (value, input, column, self.flow_memo.native_state.effect_epoch, frame);
        if let Some(result) = self.native_alias_cache.values.get(&key) { return result.clone() }
        let query = Query::NativeAliasValue(value, input, column, frame);
        if !self.enter(query) { self.mark_tainted_from(0); return Origins::unknown() }
        let begun = self.begin_memoizable();
        let environment = if column == 0 && provider.alias_environment_candidate(value, input) {
            let (origin, complete) = self.run_memoizable(|checker| {
                let ty = provider.alias_value_type(value, input, checker);
                let mut selected = None;
                let unique = checker.parts(ty).iter().all(|&part| {
                    let crate::types::TypeData::NativeTable(table) = checker.data(part) else { return false };
                    let Some(origin) = table.environment_origin else { return false };
                    if selected.is_some_and(|previous| previous != origin) { return false }
                    selected = Some(origin);
                    true
                });
                unique.then_some(selected).flatten()
            });
            if complete.is_some() && !self.is_stack_low() { origin } else { None }
        } else { None };
        let member = if column == 0 { provider.value_alias_environment_member(value, input) } else { None };
        let member = member.and_then(|member| self.native_alias_environment_member(provider, member));
        let mut result = if let Some(member) = member {
            member
        } else if let Some(environment) = environment {
            Origins::one(Origin::Environment(environment), false)
        } else { match provider.value_alias_source(value, input) {
            NativeAliasValue::Reference { reference, input } => if column == 0 { self.native_alias_reference(provider, reference, input) } else { Origins::non_table() },
            NativeAliasValue::Allocation => if column == 0 { Origins::one(Origin::Allocation(value, input), false) } else { Origins::non_table() },
            NativeAliasValue::Function { function, creation } => {
                if column != 0 { Origins::non_table() }
                else if self.native_function_provider.is_some_and(|provider| provider.contains_function(function)) && provider.contains_graph(creation.graph) && provider.node(creation).is_some() {
                    let activation = self.native_alias_cache.executions.iter().rev().find(|frame| frame.entry.graph == creation.graph).map_or(0, |frame| frame.id);
                    Origins::one(Origin::Function(function, creation, activation), false)
                } else { Origins::unknown() }
            },
            NativeAliasValue::Projection { value, input, index } => match index.checked_add(column) {
                Some(index) => self.native_alias_value(provider, value, input, index),
                None => Origins::unknown(),
            },
            NativeAliasValue::Call { callee, input: callee_input } => self.native_alias_call(provider, value, input, column, callee, callee_input),
            NativeAliasValue::NonNilScalar => if column == 0 { Origins::non_nil_scalar() } else { Origins::non_table() },
            NativeAliasValue::NotTable => Origins::non_table(),
            NativeAliasValue::Unknown => Origins::one(Origin::OpaqueValue(value, input, column), true),
        } };
        let memoizable = self.end_memoizable(begun);
        match self.leave(query) {
            Ok(stored) if memoizable.is_some() && !result.uncertain && !self.is_stack_low() => self.native_alias_cache.value(key, result.clone(), stored),
            Ok(_) if memoizable.is_some() => {},
            _ => { result.unknown = true; result.uncertain = true; self.mark_tainted_from(0); },
        }
        result
    }

    fn native_alias_environment_member(&mut self, provider: &dyn NativeFlowProvider, member: NativeAliasEnvironmentMember) -> Option<Origins> {
        if member.reference.graph != member.input.graph || !provider.contains_reference(member.reference) || provider.node(member.input).is_none() { return None }
        let NativeAliasInitial::GlobalValue(binding) = provider.reference_alias_initial(member.reference) else { return None };
        if !self.native_globals_provider.is_some_and(|globals| globals.contains_global_binding(binding) && globals.contains_global_environment(binding.environment)) { return None }
        let receiver = self.native_alias_value(provider, member.receiver.value, member.receiver.input, 0);
        if receiver.uncertain { self.mark_tainted_from(0); return Some(receiver) }
        if receiver.unknown || receiver.non_table || receiver.tokens.len() != 1 || !receiver.tokens.contains(&Origin::Environment(binding.environment)) { return None }
        // Resolve the physical slot at the lookup epoch, after evaluating a
        // computed key, rather than at the receiver's earlier frozen input.
        Some(self.native_alias_reference(provider, member.reference, member.input))
    }

    fn native_alias_projected_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey, column: usize, path: &[NativeTableKey]) -> Origins {
        if path.is_empty() { return self.native_alias_value(provider, value, input, column) }
        if value.snapshot != input.graph.snapshot || !provider.contains_graph(input.graph) || provider.node(input).is_none() { return Origins::unknown() }
        let Some(path) = path.iter().map(|key| key.normalize().ok()).collect::<Option<Vec<_>>>() else { return Origins::unknown() };
        let path_id = self.native_alias_cache.path(&path);
        let epoch = self.flow_memo.native_state.effect_epoch;
        let frame = self.native_alias_cache.frame();
        let key = (value, input, column, path_id, epoch, frame);
        if let Some(result) = self.native_alias_cache.projected_values.get(&key) { return result.clone() }
        let query = Query::NativeAliasProjectedValue(value, input, column, path_id, epoch, frame);
        if !self.enter(query) { self.mark_tainted_from(0); return Origins::unknown() }
        let begun = self.begin_memoizable();
        let mut result = if column == 0 && let Some(field) = provider.value_alias_member_source(value, input, path[0], self) {
            self.native_alias_projected_value(provider, field.value, field.input, 0, &path[1..])
        } else {
            match provider.value_alias_source(value, input) {
                NativeAliasValue::Reference { reference, input } if column == 0 => self.native_alias_projected_reference(provider, reference, input, &path),
                NativeAliasValue::Projection { value, input, index } => match index.checked_add(column) {
                    Some(column) => self.native_alias_projected_value(provider, value, input, column, &path),
                    None => Origins::unknown(),
                },
                NativeAliasValue::NonNilScalar | NativeAliasValue::NotTable => Origins::non_table(),
                NativeAliasValue::Reference { .. } | NativeAliasValue::Allocation if column != 0 => Origins::non_table(),
                _ => Origins::unavailable(),
            }
        };
        let memoizable = self.end_memoizable(begun);
        match self.leave(query) {
            Ok(stored) if memoizable.is_some() && !result.uncertain && !self.is_stack_low() => self.native_alias_cache.projected_value(key, result.clone(), stored),
            Ok(_) if memoizable.is_some() => {},
            _ => { result.unknown = true; result.uncertain = true; self.mark_tainted_from(0); },
        }
        result
    }

    fn native_alias_projected_reference(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey, path: &[NativeTableKey]) -> Origins {
        if path.is_empty() { return self.native_alias_reference(provider, reference, at) }
        if reference.graph != at.graph || !provider.contains_graph(at.graph) || !provider.contains_reference(reference) { return Origins::unknown() }
        let path_id = self.native_alias_cache.path(path);
        let epoch = self.flow_memo.native_state.effect_epoch;
        let frame = self.native_alias_cache.frame();
        let key = (reference, at, path_id, epoch, frame);
        if let Some(result) = self.native_alias_cache.projected_references.get(&key) { return result.clone() }
        let query = Query::NativeAliasProjectedReference(reference, at, path_id, epoch, frame);
        if !self.enter(query) { self.mark_tainted_from(0); return Origins::unknown() }
        let context = self.native_query_context();
        let begun = self.begin_memoizable();
        let mut result = match provider.node(at) {
            // An immutable captured cell preserves its table identity, not
            // the identities stored in that table. Creation-time fields may
            // be replaced before invocation or by an earlier invocation;
            // lexical assignment positions cannot attest heap invariance.
            // An opaque entry token preserves the identity of this one frozen
            // projection within its activation, without claiming an original
            // constructor object. Call returns wrap it with the actual call.
            Some(NativeFlowNode::Start) => Origins::one(Origin::OpaqueProjection(reference, at, path.to_vec()), true),
            Some(NativeFlowNode::Write { before, .. }) if !self.native_alias_dispatch_safe(provider, at, before) => Origins::one(Origin::OpaqueProjection(reference, at, path.to_vec()), true),
            Some(NativeFlowNode::Write { before, target, value, input }) => {
                self.native_alias_projected_write(provider, reference, at, path, before, target, value, input)
            },
            Some(NativeFlowNode::Merge { edges }) => {
                let mut origins = Origins::default();
                for edge in edges { origins.merge(self.native_alias_projected_reference(provider, reference, edge, path)); }
                origins
            },
            Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }
                | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. }) => {
                // The root cell can be immutable while a dispatched operation
                // replaces an object stored anywhere along this heap path.
                if self.native_alias_dispatch_safe(provider, at, before) { self.native_alias_projected_reference(provider, reference, before, path) }
                else { Origins::one(Origin::OpaqueProjection(reference, at, path.to_vec()), true) }
            },
            // Heap contents may change across a call even when the base cell
            // remains stable. Neither a loop site nor a captured activation is
            // an identity proof for the projected object.
            Some(NativeFlowNode::Call { .. } | NativeFlowNode::Loop { .. }) => Origins::unavailable(),
            Some(NativeFlowNode::Unreachable | NativeFlowNode::Return { .. }) => Origins::non_table(),
            _ => Origins::unknown(),
        };
        let memoizable = self.end_memoizable(begun);
        match self.leave(query) {
            Ok(stored) if memoizable.is_some() && !result.uncertain && !self.is_stack_low() => self.native_alias_cache.projected_reference(key, result.clone(), stored),
            Ok(_) if memoizable.is_some() => {},
            Err(_) if self.native_complete_without_cache(context, memoizable.is_some()) => {},
            _ => { result.unknown = true; result.uncertain = true; self.mark_tainted_from(0); },
        }
        result
    }

    fn native_alias_projected_write(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey, path: &[NativeTableKey], before: NativeFlowKey, target: NativeReferenceKey, value: NativeValueKey, input: NativeFlowKey) -> Origins {
        let cell_write = provider.reference_is_cell(target);
        if cell_write || !provider.reference_is_cell(reference) {
            let relation = if cell_write { provider.write_affects(reference, target, at) }
                else { provider.write_affects_ignoring_descendants(reference, target, at, self) };
            match relation {
                NativeWriteRelation::Exact => return self.native_alias_projected_value(provider, value, input, 0, path),
                NativeWriteRelation::Ancestor => {
                    let Some(mut suffix) = provider.reference_member_path(target, reference, self) else { return Origins::unknown() };
                    suffix.extend_from_slice(path);
                    return self.native_alias_projected_value(provider, value, input, 0, &suffix);
                },
                NativeWriteRelation::PossibleExact | NativeWriteRelation::PossibleAncestor => return Origins::unknown(),
                _ => {},
            }
            if cell_write { return self.native_alias_projected_reference(provider, reference, before, path) }
        }
        let written = provider.write_member_key(reference, target, at, self).and_then(|key| key.normalize().ok());
        if written.is_some_and(|key| !path.contains(&key)) {
            return self.native_alias_projected_reference(provider, reference, before, path);
        }
        let Some(receiver) = provider.write_alias_receiver(at) else { return Origins::unknown() };
        let receiver = self.native_alias_value(provider, receiver.value, receiver.input, 0);
        // The backward graph walk finds the latest reached store. Within one
        // store, an outer replacement changes the route to every deeper field,
        // so examine original proper prefixes from the base outwards.
        for length in 0..path.len() {
            if written.is_some_and(|key| key != path[length]) { continue }
            let prefix = self.native_alias_projected_reference(provider, reference, before, &path[..length]);
            match prefix.relation(&receiver) {
                NativeAliasRelation::Different => {},
                NativeAliasRelation::Same if written.is_some() => return self.native_alias_projected_value(provider, value, input, 0, &path[length + 1..]),
                NativeAliasRelation::Same | NativeAliasRelation::Possible => return Origins::unknown(),
            }
        }
        self.native_alias_projected_reference(provider, reference, before, path)
    }

    fn native_alias_call(&mut self, provider: &dyn NativeFlowProvider, call: NativeValueKey, input: NativeFlowKey, column: usize, callee: NativeValueKey, callee_input: NativeFlowKey) -> Origins {
        let opaque = || Origins::one(Origin::OpaqueValue(call, input, column), true);
        let Some(recipe) = provider.alias_call_source(call, input) else { return opaque() };
        if recipe.callee.value != callee || recipe.callee.input != callee_input || recipe.execution_input.graph != input.graph || provider.node(recipe.execution_input).is_none() { return opaque() }
        let callee = self.native_alias_value(provider, callee, callee_input, 0);
        if callee.uncertain { return Origins::unknown() }
        if callee.unknown || callee.non_table || callee.tokens.len() != 1 { return opaque() }
        let origin = callee.tokens.into_iter().next().unwrap();
        if let Origin::Standard(site) = origin {
            if site.kind == NativeIntrinsicKind::SetMetatable {
                return if column == 0 { recipe.arguments.first().map_or_else(opaque, |first| self.native_alias_value(provider, first.value, first.input, 0)) } else { Origins::non_table() };
            }
            return opaque();
        }
        let Origin::Function(function, creation, activation) = origin else { return opaque() };
        if activation != 0 && !self.native_alias_cache.executions.iter().any(|frame| frame.id == activation) { return opaque() }
        if self.native_alias_cache.executions.iter().any(|frame| frame.function == Some((function, creation, activation))) { return Origins::unknown() }
        let Some(execution) = provider.function_alias_execution(function, creation) else { return opaque() };
        if !matches!(provider.node(execution.entry), Some(NativeFlowNode::Start)) || !provider.contains_graph(execution.entry.graph) { return opaque() }
        let Some(rows) = provider.function_alias_returns(function) else { return opaque() };
        let Some(exit) = provider.function_alias_exit(function) else { return opaque() };
        if exit.graph != execution.entry.graph || rows.iter().any(|row| row.at.graph != execution.entry.graph) { return opaque() }
        let query = Query::NativeAliasReturn(function, column);
        if !self.enter(query) { return Origins::unknown() }
        let mut result = self.with_native_alias_execution(call, recipe.execution_input, &execution, Some((function, creation, activation)), |checker| {
        let mut result = Origins::default();
        for row in rows {
            let Some(NativeFlowNode::Return { before }) = provider.node(row.at) else { result.unknown = true; result.uncertain = true; continue };
            if !checker.native_flow_reachable(provider, before) { continue; }
            let selected = if column < row.values.len() { Some((row.values[column], 0)) }
                else if row.expands_last { row.values.last().copied().map(|last| (last, column - row.values.len() + 1)) }
                else { None };
            let Some((selected, offset)) = selected else { result.non_table = true; continue };
            let origins = checker.native_alias_value(provider, selected.value, selected.input, offset);
            result.unknown |= origins.unknown;
            result.uncertain |= origins.uncertain;
            result.non_table |= origins.non_table;
            for origin in origins.tokens {
                if matches!(origin, Origin::Environment(_)) {
                    result.tokens.insert(origin);
                    continue;
                }
                if let Some(allocated) = origin.allocation_input() {
                    if allocated.graph == row.at.graph {
                        result.tokens.insert(Origin::CallAllocation(call, input, Box::new(origin)));
                    } else { result.tokens.insert(origin); }
                } else if matches!(origin, Origin::Standard(_) | Origin::Environment(_)) {
                    result.tokens.insert(origin);
                } else {
                    result.tokens.insert(Origin::CallOpaque(call, input, Box::new(origin)));
                }
            }
        }
        if checker.native_flow_reachable(provider, exit) { result.non_table = true; }
        result
        });
        if self.leave(query).is_err() { result.unknown = true; result.uncertain = true; }
        result
    }
}
