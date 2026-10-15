//! Shared flow traversal extracted from flow.rs. Domain adapters retain syntax
//! transfers; this module owns traversal, joins and incomplete loop handling.

use super::flow::Memoizable;
use super::*;
use smallvec::{SmallVec, smallvec};
use std::hash::Hash;

pub(super) const MAX_STEPS: u32 = 2_000_000;
/// `getTypeAtFlowNode`: the invocation that finds this many in progress bails out.
pub(super) const MAX_FLOW_DEPTH: u32 = 2000;

#[derive(Clone, Copy)]
pub(super) struct FlowType {
    pub ty: TypeId,
    pub incomplete: bool,
}

impl FlowType {
    pub fn new(ty: TypeId, incomplete: bool) -> Self {
        Self {
            ty: if incomplete && ty.is_never() {
                TypeId::SILENT_NEVER
            } else {
                ty
            },
            incomplete,
        }
    }
}

pub(super) struct WalkState<K: Copy + Eq + Hash> {
    shared: SmallVec<[(K, FlowType); 8]>,
    many_shared: FxHashMap<K, FlowType>,
    pub reduced: Vec<(K, K)>,
    pub back_edges: u32,
    pub steps: u32,
    pub depth: u32,
    pub too_deep: bool,
}

impl<K: Copy + Eq + Hash> Default for WalkState<K> {
    fn default() -> Self {
        Self {
            shared: SmallVec::new(),
            many_shared: FxHashMap::default(),
            reduced: Vec::new(),
            back_edges: 0,
            steps: 0,
            depth: 0,
            too_deep: false,
        }
    }
}

impl<K: Copy + Eq + Hash> WalkState<K> {
    fn shared(&self, key: K) -> Option<FlowType> {
        self.shared
            .iter()
            .find(|entry| entry.0 == key)
            .map(|entry| entry.1)
            .or_else(|| self.many_shared.get(&key).copied())
    }

    fn remember(&mut self, key: K, ty: FlowType) {
        if self.shared(key).is_some() {
            return;
        }
        if self.shared.len() < self.shared.inline_size() {
            self.shared.push((key, ty));
        } else {
            self.many_shared.insert(key, ty);
        }
    }
}

pub(super) enum FlowStep<K, P> {
    Value(TypeId),
    Complete(FlowType),
    Initial,
    Follow {
        before: K,
        transfer: Option<P>,
        count_passed: bool,
    },
    Branch {
        edges: Vec<K>,
        bypass: Option<K>,
    },
    Loop {
        edges: Vec<K>,
    },
    Reduce {
        before: K,
        label: K,
        instead: K,
    },
}

/// Syntax/operation and storage dispatch. Neither adapter contains traversal or
/// loop solving: both invoke walk_flow below with the same pending-frame logic.
pub(super) trait FlowDomain {
    type Node: Copy + Eq + Hash;
    type Transfer;
    type Walk;

    fn state(walk: &Self::Walk) -> &WalkState<Self::Node>;
    fn state_mut(walk: &mut Self::Walk) -> &mut WalkState<Self::Node>;
    fn declared(walk: &Self::Walk) -> TypeId;
    fn crossed(walk: &Self::Walk) -> u32;
    fn set_crossed(walk: &mut Self::Walk, crossed: u32);
    fn initial(&self, checker: &mut Checker<'_, '_>, walk: &mut Self::Walk) -> TypeId;
    fn join(&self, checker: &mut Checker<'_, '_>, _: &Self::Walk, types: &[TypeId], initial: TypeId) -> TypeId {
        checker.flow_join(types, initial)
    }
    fn starts_as_declared(&self, checker: &mut Checker<'_, '_>, walk: &mut Self::Walk) -> bool;
    fn shared(&self, checker: &Checker<'_, '_>, node: Self::Node) -> bool;
    fn step(
        &self,
        checker: &mut Checker<'_, '_>,
        walk: &mut Self::Walk,
        node: Self::Node,
    ) -> FlowStep<Self::Node, Self::Transfer>;
    fn transfer(
        &self,
        checker: &mut Checker<'_, '_>,
        walk: &Self::Walk,
        transfer: Self::Transfer,
        previous: FlowType,
    ) -> FlowType;
    fn empty_transfer(&self) -> Self::Transfer;
    fn bypass_live(&self, checker: &mut Checker<'_, '_>, node: Self::Node) -> bool;
    /// Source binders may retain syntactic branches that the TS binder discards.
    /// This hook only removes a proven unreachable edge; traversal and joins
    /// remain in this worker, including back-edge memo isolation below.
    fn edge_live(&self, _: &mut Checker<'_, '_>, _: Self::Node) -> bool {
        true
    }
    /// A source binder may leave a label whose every predecessor is unreachable.
    /// This differs from a live predecessor that narrows this reference to Never.
    fn unreachable_branch(&self, _: &mut Checker<'_, '_>, _: &Self::Walk) -> TypeId {
        TypeId::NEVER
    }
    fn has_key(&self, checker: &mut Checker<'_, '_>, walk: &mut Self::Walk) -> bool;
    fn loop_cached(
        &self,
        checker: &Checker<'_, '_>,
        walk: &Self::Walk,
        node: Self::Node,
        initial: TypeId,
    ) -> Option<TypeId>;
    fn loop_active(
        &self,
        checker: &Checker<'_, '_>,
        walk: &Self::Walk,
        node: Self::Node,
        initial: TypeId,
    ) -> Option<(TypeId, usize)>;
    fn push_loop(
        &self,
        checker: &mut Checker<'_, '_>,
        walk: &Self::Walk,
        node: Self::Node,
        initial: TypeId,
        ty: TypeId,
    );
    fn update_loop(&self, checker: &mut Checker<'_, '_>, ty: TypeId);
    fn pop_loop(&self, checker: &mut Checker<'_, '_>);
    fn save_loop(
        &self,
        checker: &mut Checker<'_, '_>,
        walk: &Self::Walk,
        node: Self::Node,
        initial: TypeId,
        ty: TypeId,
    );
    fn disabled(&self, checker: &Checker<'_, '_>, walk: &Self::Walk) -> bool;
    fn too_deep(&self, checker: &mut Checker<'_, '_>, walk: &Self::Walk, at: Self::Node);
    fn step_limit(&self, checker: &mut Checker<'_, '_>, walk: &Self::Walk, at: Self::Node);
    fn finish(&self, checker: &mut Checker<'_, '_>, walk: &Self::Walk, ty: TypeId) -> TypeId;
    fn back_edge<R>(
        &self,
        checker: &mut Checker<'_, '_>,
        work: impl FnOnce(&mut Checker<'_, '_>) -> R,
    ) -> R;
}

pub(super) enum ReachStep<K> {
    Value(bool),
    Follow(K),
    Branch(Vec<K>),
    Reduce { before: K, label: K, instead: K },
}

pub(super) trait ReachDomain {
    type Node: Copy;
    fn latest(&self, checker: &Checker<'_, '_>, node: Self::Node) -> Option<bool>;
    fn reset_latest(&self, checker: &mut Checker<'_, '_>);
    fn shared(&self, checker: &Checker<'_, '_>, node: Self::Node) -> bool;
    fn cached(&self, checker: &Checker<'_, '_>, node: Self::Node) -> Option<bool>;
    fn remember(
        &self,
        checker: &mut Checker<'_, '_>,
        node: Self::Node,
        reachable: bool,
        stored: Stored,
    );
    fn step(
        &self,
        checker: &mut Checker<'_, '_>,
        node: Self::Node,
        reduced: &[(Self::Node, Self::Node)],
    ) -> ReachStep<Self::Node>;
}

impl<'s, 'p> Checker<'s, 'p> {
    pub(super) fn walk_flow_reference<D: FlowDomain>(
        &mut self,
        domain: &D,
        walk: &mut D::Walk,
        at: D::Node,
    ) -> FlowType {
        let declared = D::declared(walk);
        if domain.disabled(self, walk) {
            return FlowType::new(TypeId::ERROR, false);
        }
        self.flow_invocation_count += 1;
        let outer = std::mem::replace(&mut self.walk_declared, declared);
        let evolved = self.walk_flow(domain, walk, at);
        self.walk_declared = outer;
        let depth = self.flow_type_cache_depth;
        if evolved.incomplete && depth != usize::MAX && self.is_flow_loop_visible(depth) {
            self.taint_from(depth);
        }
        if D::state(walk).too_deep {
            domain.too_deep(self, walk, at);
            return FlowType::new(TypeId::ERROR, false);
        }
        if D::state(walk).steps >= MAX_STEPS {
            domain.step_limit(self, walk, at);
            return FlowType::new(declared, false);
        }
        FlowType::new(domain.finish(self, walk, evolved.ty), evolved.incomplete)
    }

    /// Extracted getTypeAtFlowNode: the pending stack, joins and single-pass loop
    /// traversal are shared by the unchanged TS adapter and original-source graph.
    pub(super) fn walk_flow<D: FlowDomain>(
        &mut self,
        domain: &D,
        walk: &mut D::Walk,
        start: D::Node,
    ) -> FlowType {
        if D::state(walk).too_deep {
            return FlowType::new(TypeId::ERROR, false);
        }
        let mut pending: SmallVec<[(D::Transfer, Option<D::Node>); 8]> = SmallVec::new();
        let mut passed = 0;
        let mut flow = start;
        let mut shared_flow = None;
        let (depth, crossed) = (D::state(walk).depth, D::crossed(walk));
        let mut incomplete = false;
        let declared = D::declared(walk);
        let mut ty = loop {
            let state = D::state_mut(walk);
            state.steps += 1;
            if state.steps >= MAX_STEPS {
                break declared;
            }
            state.depth = depth + 1 + pending.len() as u32 + passed;
            if state.depth > MAX_FLOW_DEPTH {
                state.too_deep = true;
                break TypeId::ERROR;
            }
            if domain.shared(self, flow) {
                if let Some(known) = D::state(walk).shared(flow) {
                    incomplete = known.incomplete;
                    shared_flow = None;
                    break known.ty;
                }
                shared_flow = Some(flow);
            }
            match domain.step(self, walk, flow) {
                FlowStep::Value(ty) => break ty,
                FlowStep::Complete(result) => { incomplete = result.incomplete; break result.ty; }
                FlowStep::Initial => break domain.initial(self, walk),
                FlowStep::Follow {
                    before,
                    transfer,
                    count_passed,
                } => {
                    if let Some(transfer) = transfer {
                        pending.push((transfer, shared_flow.take()));
                    } else if count_passed {
                        if shared_flow.is_some() {
                            pending.push((domain.empty_transfer(), shared_flow.take()));
                        } else {
                            passed += 1;
                        }
                    }
                    flow = before;
                }
                FlowStep::Reduce {
                    before,
                    label,
                    instead,
                } => {
                    D::state_mut(walk).reduced.push((label, instead));
                    let result = self.walk_flow(domain, walk, before);
                    D::state_mut(walk).reduced.pop();
                    incomplete = result.incomplete;
                    break result.ty;
                }
                FlowStep::Branch { edges, bypass } => {
                    if let [only] = *edges
                        && bypass.is_none()
                    {
                        if !domain.edge_live(self, only) {
                            break domain.unreachable_branch(self, walk);
                        }
                        flow = only;
                        continue;
                    }
                    let mut types: SmallVec<[TypeId; 4]> = SmallVec::new();
                    let mut settled = false;
                    for edge in edges {
                        if !domain.edge_live(self, edge) {
                            continue;
                        }
                        let result = self.walk_flow(domain, walk, edge);
                        if result.ty == declared && domain.starts_as_declared(self, walk) {
                            types.clear();
                            types.push(result.ty);
                            incomplete = false;
                            settled = true;
                            break;
                        }
                        if !types.contains(&result.ty) {
                            types.push(result.ty);
                        }
                        incomplete |= result.incomplete;
                    }
                    if !settled && let Some(edge) = bypass {
                        let result = self.walk_flow(domain, walk, edge);
                        if !result.ty.is_never()
                            && !types.contains(&result.ty)
                            && domain.bypass_live(self, edge)
                        {
                            incomplete |= result.incomplete;
                            if result.ty == declared && domain.starts_as_declared(self, walk) {
                                types.clear();
                                incomplete = false;
                            }
                            types.push(result.ty);
                        }
                    }
                    if types.is_empty() {
                        break domain.unreachable_branch(self, walk);
                    }
                    let initial = if types.len() < 2 {
                        TypeId::UNRESOLVED
                    } else {
                        domain.initial(self, walk)
                    };
                    break FlowType::new(domain.join(self, walk, &types, initial), incomplete).ty;
                }
                FlowStep::Loop { edges } => {
                    match *edges {
                        [] => break self.convert_auto_to_any(declared),
                        [only] => {
                            flow = only;
                            continue;
                        }
                        _ => {}
                    }
                    if !domain.has_key(self, walk) {
                        break declared;
                    }
                    let initial = domain.initial(self, walk);
                    if let Some(cached) = domain.loop_cached(self, walk, flow, initial) {
                        break cached;
                    }
                    if let Some((so_far, depth)) = domain.loop_active(self, walk, flow, initial)
                        && self.is_flow_loop_visible(depth)
                    {
                        self.taint_from(depth);
                        incomplete = true;
                        break FlowType::new(so_far, true).ty;
                    }
                    let entry = self.walk_flow(domain, walk, edges[0]);
                    incomplete = entry.incomplete;
                    let mut types: SmallVec<[TypeId; 4]> = smallvec![entry.ty];
                    let refusals = self.unmarked_refusals;
                    if entry.ty != declared {
                        D::state_mut(walk).back_edges += 1;
                        domain.push_loop(self, walk, flow, initial, entry.ty);
                        let mut restarted = None;
                        for &edge in &edges[1..] {
                            let cache = std::mem::take(&mut self.flow_type_cache);
                            let depth = std::mem::replace(
                                &mut self.flow_type_cache_depth,
                                self.stack.len(),
                            );
                            let result = domain.back_edge(self, |checker| {
                                if domain.edge_live(checker, edge) {
                                    checker.walk_flow(domain, walk, edge)
                                } else {
                                    FlowType::new(TypeId::NEVER, false)
                                }
                            }).ty;
                            self.flow_type_cache = cache;
                            self.flow_type_cache_depth = depth;
                            restarted = domain
                                .loop_cached(self, walk, flow, initial)
                                .filter(|_| self.unmarked_refusals == refusals);
                            if restarted.is_some() {
                                break;
                            }
                            if result == declared {
                                types.push(result);
                                break;
                            }
                            if !types.contains(&result) {
                                types.push(result);
                                let so_far = self.union_or_evolving_with(&types, false);
                                domain.update_loop(self, so_far);
                            }
                        }
                        domain.pop_loop(self);
                        D::state_mut(walk).back_edges -= 1;
                        if let Some(cached) = restarted {
                            incomplete = false;
                            break cached;
                        }
                    }
                    let result = FlowType::new(domain.join(self, walk, &types, initial), incomplete);
                    if !incomplete
                        && !D::state(walk).too_deep
                        && D::state(walk).steps < MAX_STEPS
                        && self.unmarked_refusals == refusals
                    {
                        domain.save_loop(self, walk, flow, initial, result.ty);
                    }
                    break result.ty;
                }
            }
        };
        D::state_mut(walk).depth = depth;
        D::set_crossed(walk, crossed);
        if D::state(walk).too_deep {
            return FlowType::new(TypeId::ERROR, false);
        }
        if let Some(shared) = shared_flow {
            D::state_mut(walk).remember(shared, FlowType { ty, incomplete });
        }
        while let Some((transfer, shared)) = pending.pop() {
            let result = domain.transfer(self, walk, transfer, FlowType { ty, incomplete });
            ty = result.ty;
            incomplete = result.incomplete;
            if let Some(shared) = shared {
                D::state_mut(walk).remember(shared, result);
            }
        }
        FlowType { ty, incomplete }
    }

    /// Extracted isReachableFlowNodeWorker; the memoizable shared-node writes and
    /// label/reduction recursion use the same implementation for both domains.
    pub(super) fn walk_reachable<D: ReachDomain>(
        &mut self,
        domain: &D,
        mut flow: D::Node,
        reduced: &mut Vec<(D::Node, D::Node)>,
    ) -> bool {
        let mut shared: SmallVec<[(D::Node, Memoizable); 2]> = SmallVec::new();
        let reachable = loop {
            if let Some(latest) = domain.latest(self, flow) {
                break latest;
            }
            if domain.shared(self, flow) {
                if let Some(cached) = domain.cached(self, flow) {
                    break cached;
                }
                shared.push((flow, self.begin_memoizable()));
            }
            match domain.step(self, flow, reduced) {
                ReachStep::Value(reachable) => break reachable,
                ReachStep::Follow(before) => flow = before,
                ReachStep::Reduce {
                    before,
                    label,
                    instead,
                } => {
                    if self.is_stack_low() {
                        break true;
                    }
                    domain.reset_latest(self);
                    reduced.push((label, instead));
                    let reachable = self.walk_reachable(domain, before, reduced);
                    reduced.pop();
                    break reachable;
                }
                ReachStep::Branch(edges) => {
                    if self.is_stack_low() {
                        break true;
                    }
                    break edges
                        .into_iter()
                        .any(|edge| self.walk_reachable(domain, edge, reduced));
                }
            }
        };
        while let Some((flow, begun)) = shared.pop() {
            if let Some(stored) = self.end_memoizable(begun) {
                domain.remember(self, flow, reachable, stored);
            }
        }
        reachable
    }
}
