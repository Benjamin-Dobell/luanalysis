//! Original-source graph inputs for the shared TS/native flow traversal.
//! Graph records supply semantic operations; they never supply branch joins or
//! loop fixed points. All callbacks re-enter the requesting checker task.

use super::flow_walk::{FlowDomain, FlowStep, FlowType, ReachDomain, ReachStep, WalkState};
use super::*;
use crate::types::TypeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeGraphKey {
    pub snapshot: u64,
    pub region: u32,
    /// Unique immutable contextual-body facet; zero for the ordinary source view.
    pub context: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeFlowKey {
    pub graph: NativeGraphKey,
    pub node: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeReferenceKey {
    pub graph: NativeGraphKey,
    /// Canonical binding/path/address identity within this graph facet.
    pub reference: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeValueKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeWriteRelation {
    Exact,
    /// Replacing an ancestor invalidates the descendant reference facts.
    Ancestor,
    Unaffected,
}

#[derive(Clone, Debug)]
pub enum NativeRuntimeType {
    Nil,
    Boolean,
    Number,
    String,
}

/// Lua lexical number facts preserve integer precision separately from the
/// checker's structural Number type. They are not loop state or joined types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeNumericConstant {
    Integer(i64),
    Float(f64),
}

#[derive(Clone, Copy, Debug)]
pub struct NativeNumericInput {
    pub value: NativeValueKey,
    pub input: NativeFlowKey,
    pub literal: Option<NativeNumericConstant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeNumericIterationStage {
    First,
    Next,
}

#[derive(Clone, Debug)]
pub enum NativePredicate {
    Truthy {
        subject: Option<NativeReferenceKey>,
        value: NativeValueKey,
        input: NativeFlowKey,
    },
    Equal {
        subject: NativeReferenceKey,
        other: NativeValueKey,
        input: NativeFlowKey,
        /// Whether the source operator tests equality, as opposed to inequality.
        equals: bool,
    },
    RuntimeType {
        subject: NativeReferenceKey,
        value: NativeValueKey,
        input: NativeFlowKey,
        kind: NativeRuntimeType,
        equals: bool,
    },
    NumericIteration {
        start: NativeNumericInput,
        limit: NativeNumericInput,
        step: Option<NativeNumericInput>,
        stage: NativeNumericIterationStage,
    },
    Unsupported(&'static str),
}

#[derive(Clone, Debug)]
pub enum NativeFlowNode {
    Start,
    Pass {
        before: NativeFlowKey,
    },
    Write {
        before: NativeFlowKey,
        target: NativeReferenceKey,
        value: NativeValueKey,
        /// Frozen RHS evaluation context, prior to every store in its transaction.
        input: NativeFlowKey,
    },
    Predicate {
        before: NativeFlowKey,
        predicate: NativePredicate,
        sense: bool,
    },
    Merge {
        edges: Vec<NativeFlowKey>,
    },
    /// Entry edge first, followed by back edges; the backend owns their traversal.
    Loop {
        edges: Vec<NativeFlowKey>,
    },
    Call {
        before: NativeFlowKey,
        value: NativeValueKey,
        input: NativeFlowKey,
    },
    /// This is an unreachable continuation; return operands query their own input.
    Return {
        before: NativeFlowKey,
    },
    Unreachable,
    Unsupported(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCallEffects {
    None,
    Never,
    Unsupported(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFlowError {
    InvalidGraph(NativeGraphKey),
    InvalidNode(NativeFlowKey),
    InvalidReference(NativeReferenceKey),
    InvalidValue(NativeValueKey),
    ForeignEdge {
        from: NativeFlowKey,
        to: NativeFlowKey,
    },
    Unsupported(&'static str),
    TooDeep(NativeFlowKey),
    StepLimit(NativeFlowKey),
}

#[derive(Clone, Copy, Debug)]
pub struct NativeFlowResult {
    pub ty: TypeId,
    pub incomplete: bool,
}

pub trait NativeFlowProvider {
    fn contains_graph(&self, graph: NativeGraphKey) -> bool;
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool;
    fn node(&self, node: NativeFlowKey) -> Option<NativeFlowNode>;
    fn is_shared(&self, node: NativeFlowKey) -> bool;
    /// Matching uses frozen address identity, not the frontend's latest writes.
    fn write_affects(
        &self,
        reference: NativeReferenceKey,
        target: NativeReferenceKey,
        at: NativeFlowKey,
    ) -> NativeWriteRelation;
    fn value_type(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        checker: &mut Checker<'_, '_>,
    ) -> TypeId;
    fn call_effects(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        checker: &mut Checker<'_, '_>,
    ) -> NativeCallEffects;
    fn flow_error(&self, at: NativeFlowKey, error: NativeFlowError);
}

type ValueContext = (NativeValueKey, NativeFlowKey);
type LoopKey = (NativeFlowKey, NativeReferenceKey, TypeId, TypeId);

pub(super) struct NativeLoopFrame {
    key: LoopKey,
    ty: TypeId,
    pub depth: usize,
}

/// Checker/task-owned native tables. The graph key includes source snapshot and
/// contextual body facet; value caches also include the frozen input flow.
#[derive(Default)]
pub(super) struct NativeFlowState {
    loop_types: FxHashMap<LoopKey, TypeId>,
    pub loops: Vec<NativeLoopFrame>,
    value_types: FxHashMap<ValueContext, TypeId>,
    call_effects: FxHashMap<ValueContext, NativeCallEffects>,
    reachable: FxHashMap<NativeFlowKey, bool>,
    latest_reachable: Option<(NativeFlowKey, bool)>,
    disabled: FxHashMap<NativeGraphKey, ()>,
}

struct NativeWalk {
    reference: NativeReferenceKey,
    declared: TypeId,
    initial: TypeId,
    traversal: WalkState<NativeFlowKey>,
}

struct NativeDomain<'a> {
    provider: &'a dyn NativeFlowProvider,
    graph: NativeGraphKey,
}

enum NativeTransfer {
    Predicate(NativePredicate, bool),
    Nothing,
}

enum NumericEntry {
    Impossible,
    Known(bool),
    Unknown,
}

fn numeric_first_entry(
    start: Option<NativeNumericConstant>,
    limit: Option<NativeNumericConstant>,
    step: Option<NativeNumericConstant>,
) -> NumericEntry {
    let (Some(start), Some(limit), Some(step)) = (start, limit, step) else {
        return NumericEntry::Unknown;
    };
    let positive = match step {
        NativeNumericConstant::Integer(0) => return NumericEntry::Impossible,
        NativeNumericConstant::Integer(step) => step > 0,
        NativeNumericConstant::Float(step) if step == 0.0 => return NumericEntry::Impossible,
        NativeNumericConstant::Float(step) if step.is_nan() => return NumericEntry::Unknown,
        NativeNumericConstant::Float(step) => step > 0.0,
    };
    if let (NativeNumericConstant::Integer(start), NativeNumericConstant::Integer(_)) = (start, step) {
        return match limit {
            NativeNumericConstant::Integer(limit) => NumericEntry::Known(if positive { start <= limit } else { start >= limit }),
            NativeNumericConstant::Float(limit) if start.unsigned_abs() < 9_007_199_254_740_992 && !limit.is_nan() => {
                NumericEntry::Known(if positive { (start as f64) <= limit } else { (start as f64) >= limit })
            }
            _ => NumericEntry::Unknown,
        };
    }
    // Lua selects its float loop when initial value or step is a float.
    // Convert integer controls there, preserving Lua's own rounding semantics.
    let number = |value| match value {
        NativeNumericConstant::Integer(value) => value as f64,
        NativeNumericConstant::Float(value) => value,
    };
    let (start, limit) = (number(start), number(limit));
    if start.is_nan() || limit.is_nan() {
        NumericEntry::Unknown
    } else {
        NumericEntry::Known(if positive { start <= limit } else { start >= limit })
    }
}

impl NativeDomain<'_> {
    fn node(&self, checker: &mut Checker<'_, '_>, at: NativeFlowKey) -> Option<NativeFlowNode> {
        if at.graph != self.graph || !self.provider.contains_graph(at.graph) {
            self.provider
                .flow_error(at, NativeFlowError::InvalidGraph(at.graph));
            return None;
        }
        let node = self.provider.node(at);
        if node.is_none() {
            self.provider
                .flow_error(at, NativeFlowError::InvalidNode(at));
        }
        let _ = checker;
        node
    }

    fn edge(&self, from: NativeFlowKey, to: NativeFlowKey) -> bool {
        if to.graph != self.graph {
            self.provider
                .flow_error(from, NativeFlowError::ForeignEdge { from, to });
            false
        } else {
            true
        }
    }

    fn predicate_valid(&self, at: NativeFlowKey, predicate: &NativePredicate) -> bool {
        let (subject, input) = match predicate {
            NativePredicate::Truthy { subject, input, .. } => (*subject, *input),
            NativePredicate::Equal { subject, input, .. } => (Some(*subject), *input),
            NativePredicate::RuntimeType { subject, input, .. } => (Some(*subject), *input),
            NativePredicate::NumericIteration { start, limit, step, .. } => {
                return [Some(*start), Some(*limit), *step].into_iter().flatten().all(|header| {
                    if !self.edge(at, header.input) {
                        return false;
                    }
                    if header.value.snapshot != header.input.graph.snapshot {
                        self.provider.flow_error(at, NativeFlowError::InvalidValue(header.value));
                        return false;
                    }
                    true
                });
            }
            NativePredicate::Unsupported(_) => return true,
        };
        if !self.edge(at, input) {
            return false;
        }
        if let Some(subject) = subject
            && (subject.graph != self.graph || !self.provider.contains_reference(subject))
        {
            self.provider.flow_error(at, NativeFlowError::InvalidReference(subject));
            return false;
        }
        true
    }
}

impl FlowDomain for NativeDomain<'_> {
    type Node = NativeFlowKey;
    type Transfer = NativeTransfer;
    type Walk = NativeWalk;
    fn state(walk: &NativeWalk) -> &WalkState<NativeFlowKey> {
        &walk.traversal
    }
    fn state_mut(walk: &mut NativeWalk) -> &mut WalkState<NativeFlowKey> {
        &mut walk.traversal
    }
    fn declared(walk: &NativeWalk) -> TypeId {
        walk.declared
    }
    fn crossed(_: &NativeWalk) -> u32 {
        0
    }
    fn set_crossed(_: &mut NativeWalk, _: u32) {}
    fn initial(&self, _: &mut Checker<'_, '_>, walk: &mut NativeWalk) -> TypeId {
        walk.initial
    }
    fn starts_as_declared(&self, _: &mut Checker<'_, '_>, walk: &mut NativeWalk) -> bool {
        walk.initial == walk.declared
    }
    fn shared(&self, _: &Checker<'_, '_>, at: NativeFlowKey) -> bool {
        at.graph == self.graph && self.provider.is_shared(at)
    }
    fn step(
        &self,
        checker: &mut Checker<'_, '_>,
        walk: &mut NativeWalk,
        at: NativeFlowKey,
    ) -> FlowStep<NativeFlowKey, NativeTransfer> {
        let Some(node) = self.node(checker, at) else {
            return FlowStep::Value(TypeId::ERROR);
        };
        match node {
            NativeFlowNode::Start => FlowStep::Initial,
            NativeFlowNode::Unreachable | NativeFlowNode::Return { .. } => {
                FlowStep::Value(checker.convert_auto_to_any(walk.declared))
            }
            NativeFlowNode::Unsupported(feature) => {
                self.provider
                    .flow_error(at, NativeFlowError::Unsupported(feature));
                FlowStep::Value(TypeId::ERROR)
            }
            NativeFlowNode::Pass { before } => {
                if !self.edge(at, before) {
                    return FlowStep::Value(TypeId::ERROR);
                }
                FlowStep::Follow {
                    before,
                    transfer: None,
                    count_passed: false,
                }
            }
            NativeFlowNode::Write {
                before,
                target,
                value,
                input,
            } => {
                if !self.edge(at, before) || !self.edge(at, input) {
                    return FlowStep::Value(TypeId::ERROR);
                }
                if target.graph != self.graph || !self.provider.contains_reference(target) {
                    self.provider
                        .flow_error(at, NativeFlowError::InvalidReference(target));
                    return FlowStep::Value(TypeId::ERROR);
                }
                match self.provider.write_affects(walk.reference, target, at) {
                    NativeWriteRelation::Exact => {
                        let assigned = checker.native_flow_value(self.provider, value, input);
                        FlowStep::Value(checker.assignment_reduced_type(walk.declared, assigned))
                    }
                    NativeWriteRelation::Ancestor => FlowStep::Value(walk.declared),
                    NativeWriteRelation::Unaffected => FlowStep::Follow {
                        before,
                        transfer: None,
                        count_passed: false,
                    },
                }
            }
            NativeFlowNode::Predicate {
                before,
                predicate,
                sense,
            } => {
                if !self.edge(at, before) || !self.predicate_valid(at, &predicate) {
                    return FlowStep::Value(TypeId::ERROR);
                }
                let applies = match &predicate {
                    NativePredicate::Truthy { subject, .. } => *subject == Some(walk.reference),
                    NativePredicate::Equal { subject, .. } => *subject == walk.reference,
                    NativePredicate::RuntimeType { subject, .. } => *subject == walk.reference,
                    NativePredicate::NumericIteration { .. } => false,
                    NativePredicate::Unsupported(feature) => {
                        self.provider
                            .flow_error(at, NativeFlowError::Unsupported(feature));
                        return FlowStep::Value(TypeId::ERROR);
                    }
                };
                FlowStep::Follow {
                    before,
                    transfer: applies.then_some(NativeTransfer::Predicate(predicate, sense)),
                    count_passed: true,
                }
            }
            NativeFlowNode::Merge { edges } | NativeFlowNode::Loop { edges } => {
                if edges.iter().any(|&edge| !self.edge(at, edge)) {
                    return FlowStep::Value(TypeId::ERROR);
                }
                if matches!(self.provider.node(at), Some(NativeFlowNode::Loop { .. })) {
                    FlowStep::Loop { edges }
                } else {
                    FlowStep::Branch {
                        edges,
                        bypass: None,
                    }
                }
            }
            NativeFlowNode::Call {
                before,
                value,
                input,
            } => {
                if !self.edge(at, before) || !self.edge(at, input) {
                    return FlowStep::Value(TypeId::ERROR);
                }
                match checker.native_flow_call_effects(self.provider, value, input) {
                    NativeCallEffects::Never => FlowStep::Value(TypeId::UNREACHABLE_NEVER),
                    NativeCallEffects::None => FlowStep::Follow {
                        before,
                        transfer: None,
                        count_passed: false,
                    },
                    NativeCallEffects::Unsupported(feature) => {
                        self.provider
                            .flow_error(at, NativeFlowError::Unsupported(feature));
                        FlowStep::Value(TypeId::ERROR)
                    }
                }
            }
        }
    }

    fn transfer(
        &self,
        checker: &mut Checker<'_, '_>,
        walk: &NativeWalk,
        transfer: NativeTransfer,
        previous: FlowType,
    ) -> FlowType {
        let ty = match transfer {
            NativeTransfer::Nothing => previous.ty,
            NativeTransfer::Predicate(_, _) if previous.ty.is_never() => previous.ty,
            NativeTransfer::Predicate(NativePredicate::Truthy { .. }, sense) => {
                checker.lua_truthiness_type(previous.ty, sense)
            }
            NativeTransfer::Predicate(
                NativePredicate::Equal {
                    other,
                    input,
                    equals,
                    ..
                },
                sense,
            ) => {
                let other = checker.native_flow_value(self.provider, other, input);
                checker.native_equal_flow_type(previous.ty, other, equals == sense)
            }
            NativeTransfer::Predicate(NativePredicate::RuntimeType { kind, equals, .. }, sense) => {
                checker.native_runtime_type(previous.ty, &kind, equals == sense)
            }
            NativeTransfer::Predicate(NativePredicate::NumericIteration { .. }, _) => previous.ty,
            NativeTransfer::Predicate(NativePredicate::Unsupported(_), _) => TypeId::ERROR,
        };
        let _ = walk;
        FlowType::new(ty, previous.incomplete)
    }
    fn empty_transfer(&self) -> NativeTransfer {
        NativeTransfer::Nothing
    }
    fn bypass_live(&self, _: &mut Checker<'_, '_>, _: NativeFlowKey) -> bool {
        true
    }
    fn edge_live(&self, checker: &mut Checker<'_, '_>, at: NativeFlowKey) -> bool {
        let (reachable, stored) = checker.run_memoizable(|checker| {
            checker.native_flow_reachable(self.provider, at)
        });
        // An incomplete loop type can currently make an edge appear impossible.
        // Keep it until the same backend loop publication rules settle it.
        reachable || stored.is_none()
    }
    fn has_key(&self, _: &mut Checker<'_, '_>, _: &mut NativeWalk) -> bool {
        true
    }
    fn loop_cached(
        &self,
        checker: &Checker<'_, '_>,
        walk: &NativeWalk,
        node: NativeFlowKey,
        initial: TypeId,
    ) -> Option<TypeId> {
        checker
            .flow_memo
            .native_state
            .loop_types
            .get(&(node, walk.reference, walk.declared, initial))
            .copied()
    }
    fn loop_active(
        &self,
        checker: &Checker<'_, '_>,
        walk: &NativeWalk,
        node: NativeFlowKey,
        initial: TypeId,
    ) -> Option<(TypeId, usize)> {
        let key = (node, walk.reference, walk.declared, initial);
        checker
            .flow_memo
            .native_state
            .loops
            .iter()
            .rfind(|frame| frame.key == key)
            .map(|frame| (frame.ty, frame.depth))
    }
    fn push_loop(
        &self,
        checker: &mut Checker<'_, '_>,
        walk: &NativeWalk,
        node: NativeFlowKey,
        initial: TypeId,
        ty: TypeId,
    ) {
        checker.flow_memo.native_state.loops.push(NativeLoopFrame {
            key: (node, walk.reference, walk.declared, initial),
            ty,
            depth: checker.stack.len(),
        });
    }
    fn update_loop(&self, checker: &mut Checker<'_, '_>, ty: TypeId) {
        if let Some(frame) = checker.flow_memo.native_state.loops.last_mut() {
            frame.ty = ty;
        }
    }
    fn pop_loop(&self, checker: &mut Checker<'_, '_>) {
        checker.flow_memo.native_state.loops.pop();
    }
    fn save_loop(
        &self,
        checker: &mut Checker<'_, '_>,
        walk: &NativeWalk,
        node: NativeFlowKey,
        initial: TypeId,
        ty: TypeId,
    ) {
        checker
            .flow_memo
            .native_state
            .loop_types
            .insert((node, walk.reference, walk.declared, initial), ty);
        checker.flow_memo.flow_loop_cached_at = checker.work;
    }
    fn disabled(&self, checker: &Checker<'_, '_>, _: &NativeWalk) -> bool {
        checker
            .flow_memo
            .native_state
            .disabled
            .contains_key(&self.graph)
    }
    fn too_deep(&self, checker: &mut Checker<'_, '_>, _: &NativeWalk, at: NativeFlowKey) {
        checker
            .flow_memo
            .native_state
            .disabled
            .insert(self.graph, ());
        self.provider.flow_error(at, NativeFlowError::TooDeep(at));
    }
    fn step_limit(&self, _: &mut Checker<'_, '_>, _: &NativeWalk, at: NativeFlowKey) {
        self.provider.flow_error(at, NativeFlowError::StepLimit(at));
    }
    fn finish(&self, checker: &mut Checker<'_, '_>, walk: &NativeWalk, ty: TypeId) -> TypeId {
        let ty = checker.finalize_evolving_array(ty);
        if ty == TypeId::UNREACHABLE_NEVER {
            walk.declared
        } else {
            ty
        }
    }
    fn back_edge<R>(
        &self,
        checker: &mut Checker<'_, '_>,
        work: impl FnOnce(&mut Checker<'_, '_>) -> R,
    ) -> R {
        let cache = std::mem::take(&mut checker.flow_memo.native_state.value_types);
        let calls = std::mem::take(&mut checker.flow_memo.native_state.call_effects);
        let result = work(checker);
        checker.flow_memo.native_state.value_types = cache;
        checker.flow_memo.native_state.call_effects = calls;
        result
    }
}

impl ReachDomain for NativeDomain<'_> {
    type Node = NativeFlowKey;
    fn latest(&self, checker: &Checker<'_, '_>, at: NativeFlowKey) -> Option<bool> {
        checker
            .flow_memo
            .native_state
            .latest_reachable
            .filter(|entry| entry.0 == at)
            .map(|entry| entry.1)
    }
    fn reset_latest(&self, checker: &mut Checker<'_, '_>) {
        checker.flow_memo.native_state.latest_reachable = None;
    }
    fn shared(&self, _: &Checker<'_, '_>, at: NativeFlowKey) -> bool {
        at.graph == self.graph && self.provider.is_shared(at)
    }
    fn cached(&self, checker: &Checker<'_, '_>, at: NativeFlowKey) -> Option<bool> {
        checker.flow_memo.native_state.reachable.get(&at).copied()
    }
    fn remember(
        &self,
        checker: &mut Checker<'_, '_>,
        at: NativeFlowKey,
        reachable: bool,
        _: Stored,
    ) {
        checker
            .flow_memo
            .native_state
            .reachable
            .insert(at, reachable);
    }
    fn step(
        &self,
        checker: &mut Checker<'_, '_>,
        at: NativeFlowKey,
        _: &[(NativeFlowKey, NativeFlowKey)],
    ) -> ReachStep<NativeFlowKey> {
        let Some(node) = self.node(checker, at) else {
            return ReachStep::Value(false);
        };
        match node {
            NativeFlowNode::Start => ReachStep::Value(true),
            NativeFlowNode::Return { .. } | NativeFlowNode::Unreachable => ReachStep::Value(false),
            NativeFlowNode::Unsupported(feature) => {
                self.provider
                    .flow_error(at, NativeFlowError::Unsupported(feature));
                ReachStep::Value(false)
            }
            NativeFlowNode::Pass { before }
            | NativeFlowNode::Write { before, .. } => {
                if self.edge(at, before) {
                    ReachStep::Follow(before)
                } else {
                    ReachStep::Value(false)
                }
            }
            NativeFlowNode::Predicate { before, predicate, sense } => {
                if !self.edge(at, before) || !self.predicate_valid(at, &predicate) {
                    return ReachStep::Value(false);
                }
                match predicate {
                    NativePredicate::Truthy { value, input, .. } => {
                        let value = checker.native_flow_value(self.provider, value, input);
                        if checker.lua_truthiness_type(value, sense).is_never() {
                            ReachStep::Value(false)
                        } else {
                            ReachStep::Follow(before)
                        }
                    }
                    NativePredicate::Equal { .. } => ReachStep::Follow(before),
                    NativePredicate::RuntimeType { value, input, kind, equals, .. } => {
                        let value = checker.native_flow_value(self.provider, value, input);
                        if checker.native_runtime_type(value, &kind, equals == sense).is_never() {
                            ReachStep::Value(false)
                        } else {
                            ReachStep::Follow(before)
                        }
                    }
                    NativePredicate::NumericIteration { start, limit, step, stage } => {
                        let entry = checker.native_numeric_entry(self.provider, start, limit, step, stage);
                        match entry {
                            NumericEntry::Impossible => ReachStep::Value(false),
                            NumericEntry::Known(enters) if enters != sense => ReachStep::Value(false),
                            NumericEntry::Known(_) | NumericEntry::Unknown => ReachStep::Follow(before),
                        }
                    }
                    NativePredicate::Unsupported(feature) => {
                        self.provider.flow_error(at, NativeFlowError::Unsupported(feature));
                        ReachStep::Value(false)
                    }
                }
            }
            NativeFlowNode::Call {
                before,
                value,
                input,
            } => {
                if !self.edge(at, before) || !self.edge(at, input) {
                    return ReachStep::Value(false);
                }
                match checker.native_flow_call_effects(self.provider, value, input) {
                    NativeCallEffects::None => ReachStep::Follow(before),
                    NativeCallEffects::Never => ReachStep::Value(false),
                    NativeCallEffects::Unsupported(feature) => {
                        self.provider
                            .flow_error(at, NativeFlowError::Unsupported(feature));
                        ReachStep::Value(false)
                    }
                }
            }
            NativeFlowNode::Merge { edges } => {
                if edges.iter().any(|&edge| !self.edge(at, edge)) {
                    ReachStep::Value(false)
                } else {
                    ReachStep::Branch(edges)
                }
            }
            NativeFlowNode::Loop { edges } => match edges.first() {
                Some(&entry) if self.edge(at, entry) => ReachStep::Follow(entry),
                _ => ReachStep::Value(false),
            },
        }
    }
}

impl<'s, 'p> Checker<'s, 'p> {
    fn native_numeric_constant(
        &mut self,
        provider: &dyn NativeFlowProvider,
        header: NativeNumericInput,
    ) -> Option<NativeNumericConstant> {
        let ty = self.native_flow_value(provider, header.value, header.input);
        if header.literal.is_some() {
            return header.literal;
        }
        let TypeData::NumberLit { bits, .. } = *self.data(ty) else {
            return None;
        };
        let number = f64::from_bits(bits);
        // Without a Lua lexical integer/float fact, large integer literals
        // could have lost precision in the shared Number representation.
        (number.is_finite() && number.abs() < 9_007_199_254_740_992.0)
            .then_some(NativeNumericConstant::Float(number))
    }

    fn native_numeric_entry(
        &mut self,
        provider: &dyn NativeFlowProvider,
        start: NativeNumericInput,
        limit: NativeNumericInput,
        step: Option<NativeNumericInput>,
        stage: NativeNumericIterationStage,
    ) -> NumericEntry {
        let known_kinds = start.literal.is_some() && limit.literal.is_some()
            && step.is_none_or(|step| step.literal.is_some());
        let step = step.map_or(Some(NativeNumericConstant::Integer(1)), |step| {
            self.native_numeric_constant(provider, step)
        });
        if matches!(step, Some(NativeNumericConstant::Integer(0)))
            || matches!(step, Some(NativeNumericConstant::Float(value)) if value == 0.0)
        {
            // Lua raises before either continuation; the source header check
            // reports the original zero-step diagnostic independently.
            return NumericEntry::Impossible;
        }
        if stage == NativeNumericIterationStage::Next {
            // A body continuation can iterate or terminate. Its predecessor
            // exists only when the body completes; a guaranteed first return
            // has no such exit. Do not turn a finite literal range into while
            // (true), unroll it, or solve a second loop in this adapter.
            return NumericEntry::Unknown;
        }
        let start = self.native_numeric_constant(provider, start);
        let limit = self.native_numeric_constant(provider, limit);
        if !known_kinds && [start, limit, step].into_iter().flatten().any(|value| match value {
            NativeNumericConstant::Integer(value) => value.unsigned_abs() >= 9_007_199_254_740_992,
            NativeNumericConstant::Float(value) => value.abs() >= 9_007_199_254_740_992.0,
        }) {
            // Structural numeric literal types do not distinguish Lua's
            // integer and float values. At this scale their conversion can
            // change the first-entry comparison; retain both paths.
            return NumericEntry::Unknown;
        }
        numeric_first_entry(start, limit, step)
    }

    fn native_runtime_type(&mut self, ty: TypeId, kind: &NativeRuntimeType, sense: bool) -> TypeId {
        let name = match kind {
            NativeRuntimeType::Nil => known::undefined,
            NativeRuntimeType::Boolean => known::boolean,
            NativeRuntimeType::Number => known::number,
            NativeRuntimeType::String => known::string,
        };
        self.narrow_type_by_typeof_name(ty, name, sense)
    }
    /// Type at an original-source graph node through the same walker used by TS.
    pub fn native_flow_type(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        reference: NativeReferenceKey,
        declared: TypeId,
        initial: TypeId,
    ) -> NativeFlowResult {
        let error = if !provider.contains_graph(at.graph) {
            NativeFlowError::InvalidGraph(at.graph)
        } else if reference.graph != at.graph || !provider.contains_reference(reference) {
            NativeFlowError::InvalidReference(reference)
        } else {
            let domain = NativeDomain {
                provider,
                graph: at.graph,
            };
            let mut walk = NativeWalk {
                reference,
                declared,
                initial,
                traversal: WalkState::default(),
            };
            let result = self.walk_flow_reference(&domain, &mut walk, at);
            if result.incomplete {
                // A source callback can use flow without a TS Query frame.
                // taint_from then has no frame to mark, but publication scopes
                // still must observe the read of an unfinished loop value.
                self.note_cycle();
            }
            return NativeFlowResult {
                ty: result.ty,
                incomplete: result.incomplete,
            };
        };
        provider.flow_error(at, error);
        NativeFlowResult {
            ty: TypeId::ERROR,
            incomplete: false,
        }
    }

    pub fn native_flow_reachable(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
    ) -> bool {
        if !provider.contains_graph(at.graph) {
            provider.flow_error(at, NativeFlowError::InvalidGraph(at.graph));
            return false;
        }
        let domain = NativeDomain {
            provider,
            graph: at.graph,
        };
        let (reachable, stored) = self.run_memoizable(|checker| {
            checker.walk_reachable(&domain, at, &mut Vec::new())
        });
        if stored.is_some() {
            self.flow_memo.native_state.latest_reachable = Some((at, reachable));
        }
        reachable
    }

    fn native_flow_value(
        &mut self,
        provider: &dyn NativeFlowProvider,
        value: NativeValueKey,
        input: NativeFlowKey,
    ) -> TypeId {
        if value.snapshot != input.graph.snapshot {
            provider.flow_error(input, NativeFlowError::InvalidValue(value));
            return TypeId::ERROR;
        }
        if !provider.contains_graph(input.graph) || provider.node(input).is_none() {
            provider.flow_error(input, NativeFlowError::InvalidNode(input));
            return TypeId::ERROR;
        }
        let key = (value, input);
        if let Some(&cached) = self.flow_memo.native_state.value_types.get(&key) {
            return cached;
        }
        let (ty, stored) =
            self.run_memoizable(|checker| provider.value_type(value, input, checker));
        if stored.is_some() && !self.is_stack_low() {
            self.flow_memo.native_state.value_types.insert(key, ty);
        }
        ty
    }

    fn native_flow_call_effects(
        &mut self,
        provider: &dyn NativeFlowProvider,
        value: NativeValueKey,
        input: NativeFlowKey,
    ) -> NativeCallEffects {
        let key = (value, input);
        if value.snapshot != input.graph.snapshot
            || !provider.contains_graph(input.graph)
            || provider.node(input).is_none()
        {
            provider.flow_error(input, NativeFlowError::InvalidValue(value));
            return NativeCallEffects::Unsupported("invalid frozen call context");
        }
        if let Some(&cached) = self.flow_memo.native_state.call_effects.get(&key) {
            return cached;
        }
        let (effects, stored) =
            self.run_memoizable(|checker| provider.call_effects(value, input, checker));
        if stored.is_some() && !self.is_stack_low() {
            self.flow_memo
                .native_state
                .call_effects
                .insert(key, effects);
        }
        effects
    }
}
