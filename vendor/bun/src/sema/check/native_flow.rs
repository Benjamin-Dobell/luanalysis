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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeIteratorStage {
    First,
    Next,
}

#[derive(Clone, Debug)]
pub enum NativePredicate {
    And(Box<NativePredicate>, Box<NativePredicate>),
    Or(Box<NativePredicate>, Box<NativePredicate>),
    Not(Box<NativePredicate>),
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
    GenericIteration {
        frame: NativeReferenceKey,
        input: NativeFlowKey,
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
    IteratorStart {
        before: NativeFlowKey,
        frame: NativeReferenceKey,
        header: NativeValueKey,
        input: NativeFlowKey,
    },
    Iteration {
        before: NativeFlowKey,
        frame: NativeReferenceKey,
        stage: NativeIteratorStage,
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
    /// A completed read still had an automatic declaration constraint. The
    /// returned type is Any recovery; the source owns its diagnostic locations.
    pub unresolved_auto: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeVariableWrite {
    pub owner: NativeGraphKey,
    pub statement_end: usize,
}

#[derive(Clone, Debug)]
pub struct NativeVariableDeclaration {
    pub owner: NativeGraphKey,
    pub reference: NativeReferenceKey,
    pub writes: Vec<NativeVariableWrite>,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeFunctionCreation {
    pub input: NativeFlowKey,
    pub position: usize,
}

/// Original immutable sibling projections of one physical Lua result list.
/// The provider establishes source identity/immutability; the checker narrows
/// the complete result pack through the shared flow traversal.
#[derive(Clone, Copy, Debug)]
pub struct NativePackProjection {
    pub group: NativeReferenceKey,
    pub value: NativeValueKey,
    pub input: NativeFlowKey,
    pub index: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeIteratorProjection {
    pub frame: NativeReferenceKey,
    /// Physical hidden-frame column: iterator/state occupy columns zero/one.
    pub index: usize,
}

pub trait NativeFlowProvider {
    fn reference_declaration(&self, _: NativeReferenceKey) -> Option<NativeVariableDeclaration> { None }
    fn graph_creation(&self, _: NativeGraphKey) -> Option<NativeFunctionCreation> { None }
    fn reference_in_graph(&self, reference: NativeReferenceKey, graph: NativeGraphKey) -> Option<NativeReferenceKey> {
        (reference.graph == graph).then_some(reference)
    }
    fn iterator_error(&self, _: NativeFlowKey, _: NativeIteratorStage, _: TypeId, _: ResolvedTypeCallError, _: &mut Checker<'_, '_>) {}
    fn iterator_projection(&self, _: NativeReferenceKey) -> Option<NativeIteratorProjection> { None }
    fn pack_projection(&self, _: NativeReferenceKey) -> Option<NativePackProjection> { None }
    fn value_pack(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> NativeResultPack {
        let ty = self.value_type(value, input, checker);
        checker.native_scalar_pack(ty)
    }
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
    pack_types: FxHashMap<ValueContext, NativeResultPack>,
    call_effects: FxHashMap<ValueContext, NativeCallEffects>,
    reachable: FxHashMap<NativeFlowKey, bool>,
    latest_reachable: Option<(NativeFlowKey, bool)>,
    disabled: FxHashMap<NativeGraphKey, ()>,
    capture_graphs: Vec<NativeGraphKey>,
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
    PackPredicate { predicate: NativePredicate, sense: bool, group: NativeReferenceKey },
    Iteration(NativeFlowKey, NativeIteratorStage),
    GenericIteration(bool),
    Nothing,
}

enum NumericEntry {
    Impossible,
    Known(bool),
    Unknown,
}

fn predicate_subject(predicate: &NativePredicate) -> Option<NativeReferenceKey> {
    match predicate {
        NativePredicate::Truthy { subject, .. } => *subject,
        NativePredicate::Equal { subject, .. } | NativePredicate::RuntimeType { subject, .. } => Some(*subject),
        _ => None,
    }
}

fn predicate_mentions(provider: &dyn NativeFlowProvider, predicate: &NativePredicate, reference: NativeReferenceKey, pack: bool) -> bool {
    match predicate {
        NativePredicate::And(left, right) | NativePredicate::Or(left, right) => predicate_mentions(provider, left, reference, pack) || predicate_mentions(provider, right, reference, pack),
        NativePredicate::Not(inner) => predicate_mentions(provider, inner, reference, pack),
        _ => predicate_subject(predicate).is_some_and(|subject| if pack { provider.pack_projection(subject).is_some_and(|projection| projection.group == reference) || provider.iterator_projection(subject).is_some_and(|projection| projection.frame == reference) } else { subject == reference }),
    }
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
    fn auto_at_start(&self, checker: &mut Checker<'_, '_>, walk: &NativeWalk, at: NativeFlowKey) -> Option<FlowType> {
        if walk.declared != TypeId::AUTO { return None; }
        let declaration = self.provider.reference_declaration(walk.reference)?;
        let same_owner = |graph: NativeGraphKey| graph.snapshot == declaration.owner.snapshot && graph.region == declaration.owner.region;
        if same_owner(at.graph) { return None; }
        if !self.provider.contains_graph(declaration.owner) || declaration.reference.graph != declaration.owner || !self.provider.contains_reference(declaration.reference) {
            self.provider.flow_error(at, NativeFlowError::InvalidReference(declaration.reference));
            return Some(FlowType::new(TypeId::ERROR, false));
        }
        let Some(creation) = self.provider.graph_creation(at.graph) else { return Some(FlowType::new(TypeId::AUTO, false)) };
        if declaration.writes.iter().any(|write| !same_owner(write.owner)) {
            return Some(FlowType::new(TypeId::AUTO, false));
        }
        let last = declaration.writes.iter().map(|write| write.statement_end).max().unwrap_or(0);
        if !Checker::is_past_last_assignment_position(last, creation.position) {
            return Some(FlowType::new(TypeId::AUTO, false));
        }
        if creation.input.graph.snapshot != at.graph.snapshot || !self.provider.contains_graph(creation.input.graph) {
            self.provider.flow_error(at, NativeFlowError::InvalidGraph(creation.input.graph));
            return Some(FlowType::new(TypeId::ERROR, false));
        }
        let Some(reference) = self.provider.reference_in_graph(walk.reference, creation.input.graph) else {
            self.provider.flow_error(at, NativeFlowError::InvalidReference(walk.reference));
            return Some(FlowType::new(TypeId::ERROR, false));
        };
        let graphs = &mut checker.flow_memo.native_state.capture_graphs;
        if graphs.contains(&at.graph) || graphs.len() >= super::flow_walk::MAX_FLOW_DEPTH as usize {
            self.provider.flow_error(at, NativeFlowError::TooDeep(at));
            return Some(FlowType::new(TypeId::ERROR, false));
        }
        let depth = graphs.len();
        graphs.push(at.graph);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| checker.native_flow_capture_type(self.provider, creation.input, reference, walk.declared, walk.initial)));
        checker.flow_memo.native_state.capture_graphs.truncate(depth);
        let result = match result { Ok(result) => result, Err(panic) => std::panic::resume_unwind(panic) };
        Some(FlowType::new(result.ty, result.incomplete))
    }

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
            NativePredicate::And(left, right) | NativePredicate::Or(left, right) => return self.predicate_valid(at, left) && self.predicate_valid(at, right),
            NativePredicate::Not(inner) => return self.predicate_valid(at, inner),
            NativePredicate::Truthy { subject, input, .. } => (*subject, *input),
            NativePredicate::Equal { subject, input, .. } => (Some(*subject), *input),
            NativePredicate::RuntimeType { subject, input, .. } => (Some(*subject), *input),
            NativePredicate::GenericIteration { frame, input } => (Some(*frame), *input),
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
            NativePredicate::Unsupported(feature) => { self.provider.flow_error(at, NativeFlowError::Unsupported(feature)); return false; },
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
            NativeFlowNode::Start => match self.auto_at_start(checker, walk, at) {
                Some(result) => FlowStep::Complete(result),
                None => FlowStep::Initial,
            },
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
                        FlowStep::Value(checker.resolved_assignment_type(walk.declared, assigned))
                    }
                    NativeWriteRelation::Ancestor => FlowStep::Value(walk.declared),
                    NativeWriteRelation::Unaffected => FlowStep::Follow {
                        before,
                        transfer: None,
                        count_passed: false,
                    },
                }
            }
            NativeFlowNode::IteratorStart { before, frame, header, input } => {
                if !self.edge(at, before) || !self.edge(at, input) {
                    return FlowStep::Value(TypeId::ERROR);
                }
                if frame.graph != self.graph || !self.provider.contains_reference(frame) {
                    self.provider.flow_error(at, NativeFlowError::InvalidReference(frame));
                    return FlowStep::Value(TypeId::ERROR);
                }
                if walk.reference == frame {
                    let header = checker.native_flow_pack_value(self.provider, header, input);
                    FlowStep::Value(checker.native_iterator_header(header).ty)
                } else {
                    FlowStep::Follow { before, transfer: None, count_passed: false }
                }
            }
            NativeFlowNode::Iteration { before, frame, stage } => {
                if !self.edge(at, before) {
                    return FlowStep::Value(TypeId::ERROR);
                }
                if frame.graph != self.graph || !self.provider.contains_reference(frame) {
                    self.provider.flow_error(at, NativeFlowError::InvalidReference(frame));
                    return FlowStep::Value(TypeId::ERROR);
                }
                FlowStep::Follow { before, transfer: (walk.reference == frame).then_some(NativeTransfer::Iteration(at, stage)), count_passed: false }
            }
            NativeFlowNode::Predicate {
                before,
                predicate,
                sense,
            } => {
                if !self.edge(at, before) || !self.predicate_valid(at, &predicate) {
                    return FlowStep::Value(TypeId::ERROR);
                }
                if let NativePredicate::GenericIteration { frame, .. } = predicate {
                    return FlowStep::Follow { before, transfer: (walk.reference == frame).then_some(NativeTransfer::GenericIteration(sense)), count_passed: true };
                }
                let applies = predicate_mentions(self.provider, &predicate, walk.reference, false);
                let projection = predicate_mentions(self.provider, &predicate, walk.reference, true);
                let transfer = if applies {
                    Some(NativeTransfer::Predicate(predicate, sense))
                } else {
                    projection.then_some(NativeTransfer::PackPredicate { predicate, sense, group: walk.reference })
                };
                FlowStep::Follow {
                    before,
                    transfer,
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
            NativeTransfer::Iteration(at, stage) => checker.native_iterator_step(self.provider, at, stage, NativeResultPack { ty: previous.ty }).ty,
            NativeTransfer::GenericIteration(sense) => checker.native_iterator_filter(NativeResultPack { ty: previous.ty }, sense).ty,
            NativeTransfer::PackPredicate { predicate, sense, group } => {
                checker.native_narrow_pack(self.provider, NativeResultPack { ty: previous.ty }, &predicate, sense, group).ty
            }
            NativeTransfer::Predicate(ref predicate @ (NativePredicate::And(..) | NativePredicate::Or(..) | NativePredicate::Not(..)), sense) => checker.native_narrow_predicate(self.provider, previous.ty, predicate, sense, walk.reference),
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
            NativeTransfer::Predicate(NativePredicate::GenericIteration { .. }, _) => previous.ty,
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
        let packs = std::mem::take(&mut checker.flow_memo.native_state.pack_types);
        let result = work(checker);
        checker.flow_memo.native_state.value_types = cache;
        checker.flow_memo.native_state.call_effects = calls;
        checker.flow_memo.native_state.pack_types = packs;
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
            | NativeFlowNode::Write { before, .. }
            | NativeFlowNode::IteratorStart { before, .. }
            | NativeFlowNode::Iteration { before, .. } => {
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
                    predicate @ (NativePredicate::And(..) | NativePredicate::Or(..) | NativePredicate::Not(..)) => {
                        if checker.native_predicate_possible(self.provider, &predicate, sense) { ReachStep::Follow(before) } else { ReachStep::Value(false) }
                    }
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
                    NativePredicate::GenericIteration { frame, input } => {
                        let pack = checker.native_flow_iterator_pack(self.provider, input, frame);
                        if checker.native_iterator_filter(pack, sense).ty.is_never() {
                            ReachStep::Value(false)
                        } else {
                            ReachStep::Follow(before)
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
    fn native_predicate_possible(&mut self, provider: &dyn NativeFlowProvider, predicate: &NativePredicate, sense: bool) -> bool {
        match predicate {
            NativePredicate::And(left, right) => if sense { self.native_predicate_possible(provider, left, true) && self.native_predicate_possible(provider, right, true) } else { self.native_predicate_possible(provider, left, false) || self.native_predicate_possible(provider, right, false) },
            NativePredicate::Or(left, right) => if sense { self.native_predicate_possible(provider, left, true) || self.native_predicate_possible(provider, right, true) } else { self.native_predicate_possible(provider, left, false) && self.native_predicate_possible(provider, right, false) },
            NativePredicate::Not(inner) => self.native_predicate_possible(provider, inner, !sense),
            NativePredicate::Truthy { value, input, .. } => { let value = self.native_flow_value(provider, *value, *input); !self.lua_truthiness_type(value, sense).is_never() },
            NativePredicate::RuntimeType { value, input, kind, equals, .. } => { let value = self.native_flow_value(provider, *value, *input); !self.native_runtime_type(value, kind, *equals == sense).is_never() },
            _ => true,
        }
    }

    fn native_narrow_predicate(&mut self, provider: &dyn NativeFlowProvider, ty: TypeId, predicate: &NativePredicate, sense: bool, reference: NativeReferenceKey) -> TypeId {
        match predicate {
            NativePredicate::Not(inner) => self.native_narrow_predicate(provider, ty, inner, !sense, reference),
            NativePredicate::And(left, right) | NativePredicate::Or(left, right) => {
                let conjunction = matches!(predicate, NativePredicate::And(..));
                if conjunction == sense {
                    let left = self.native_narrow_predicate(provider, ty, left, sense, reference);
                    self.native_narrow_predicate(provider, left, right, sense, reference)
                } else {
                    let bypass = self.native_narrow_predicate(provider, ty, left, sense, reference);
                    let evaluated = self.native_narrow_predicate(provider, ty, left, !sense, reference);
                    let evaluated = self.native_narrow_predicate(provider, evaluated, right, sense, reference);
                    self.union_reduced(&[bypass, evaluated])
                }
            }
            _ if predicate_subject(predicate) != Some(reference) => if self.native_predicate_possible(provider, predicate, sense) { ty } else { TypeId::NEVER },
            NativePredicate::Truthy { .. } => self.lua_truthiness_type(ty, sense),
            NativePredicate::Equal { other, input, equals, .. } => { let other = self.native_flow_value(provider, *other, *input); self.native_equal_flow_type(ty, other, *equals == sense) },
            NativePredicate::RuntimeType { kind, equals, .. } => self.native_runtime_type(ty, kind, *equals == sense),
            _ => ty,
        }
    }

    fn native_narrow_pack(&mut self, provider: &dyn NativeFlowProvider, pack: NativeResultPack, predicate: &NativePredicate, sense: bool, group: NativeReferenceKey) -> NativeResultPack {
        match predicate {
            NativePredicate::Not(inner) => return self.native_narrow_pack(provider, pack, inner, !sense, group),
            NativePredicate::And(left, right) | NativePredicate::Or(left, right) => {
                let conjunction = matches!(predicate, NativePredicate::And(..));
                if conjunction == sense {
                    let left = self.native_narrow_pack(provider, pack, left, sense, group);
                    return self.native_narrow_pack(provider, left, right, sense, group);
                }
                let bypass = self.native_narrow_pack(provider, pack, left, sense, group);
                let evaluated = self.native_narrow_pack(provider, pack, left, !sense, group);
                let evaluated = self.native_narrow_pack(provider, evaluated, right, sense, group);
                return self.native_join_packs(&[bypass, evaluated]);
            }
            _ => {}
        }
        let index = predicate_subject(predicate).and_then(|reference| {
            provider.pack_projection(reference).filter(|projection| projection.group == group).map(|projection| projection.index)
                .or_else(|| provider.iterator_projection(reference).filter(|projection| projection.frame == group).map(|projection| projection.index))
        });
        let Some(index) = index else { return if self.native_predicate_possible(provider, predicate, sense) { pack } else { NativeResultPack { ty: TypeId::NEVER } } };
        let Some(alternatives) = self.native_pack_alternatives(pack) else { return pack };
        let mut narrowed = Vec::new();
        for mut alternative in alternatives {
            let row = self.native_result_pack(std::slice::from_ref(&alternative));
            let value = self.native_pack_slot(row, index);
            let value = match predicate {
                NativePredicate::Truthy { .. } => self.lua_truthiness_type(value, sense),
                NativePredicate::Equal { other, input, equals, .. } => {
                    let other = self.native_flow_value(provider, *other, *input);
                    self.native_equal_flow_type(value, other, *equals == sense)
                }
                NativePredicate::RuntimeType { kind, equals, .. } => self.native_runtime_type(value, kind, *equals == sense),
                _ => value,
            };
            if value.is_never() { continue }
            if let Some(slot) = alternative.fixed.get_mut(index) {
                slot.ty = value;
                if !self.contains_undefined(value) { slot.optional = false; }
            }
            narrowed.push(alternative);
        }
        self.native_result_pack(&narrowed)
    }

    pub fn native_flow_pack_projection(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, projection: NativePackProjection, ordinary: TypeId) -> NativeFlowResult {
        if projection.input.graph != at.graph {
            provider.flow_error(at, NativeFlowError::ForeignEdge { from: at, to: projection.input });
            return NativeFlowResult { ty: TypeId::ERROR, incomplete: false, unresolved_auto: false };
        }
        if projection.group.graph != at.graph || !provider.contains_reference(projection.group) {
            provider.flow_error(at, NativeFlowError::InvalidReference(projection.group));
            return NativeFlowResult { ty: TypeId::ERROR, incomplete: false, unresolved_auto: false };
        }
        let initial = self.native_flow_pack_value(provider, projection.value, projection.input);
        if !self.is_union(initial.ty) {
            return NativeFlowResult { ty: ordinary, incomplete: false, unresolved_auto: false };
        }
        let result = self.native_flow_type(provider, at, projection.group, initial.ty, initial.ty);
        let ty = if result.ty == initial.ty {
            ordinary
        } else {
            let projected = self.native_pack_slot(NativeResultPack { ty: result.ty }, projection.index);
            // Keep the binding's declared view. Correlation removes impossible
            // union constituents; it must not expose constructor-only fields.
            self.map_type(ordinary, |checker, part| {
                if checker.are_comparable(part, projected) { part } else { TypeId::NEVER }
            })
        };
        NativeFlowResult { ty, incomplete: result.incomplete, unresolved_auto: false }
    }

    fn native_flow_pack_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> NativeResultPack {
        if value.snapshot != input.graph.snapshot || !provider.contains_graph(input.graph) || provider.node(input).is_none() {
            provider.flow_error(input, NativeFlowError::InvalidValue(value));
            return NativeResultPack { ty: TypeId::ERROR };
        }
        let key = (value, input);
        if let Some(&pack) = self.flow_memo.native_state.pack_types.get(&key) { return pack }
        let (pack, stored) = self.run_memoizable(|checker| provider.value_pack(value, input, checker));
        if stored.is_some() && !self.is_stack_low() { self.flow_memo.native_state.pack_types.insert(key, pack); }
        pack
    }

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

    pub(super) fn native_runtime_type(&mut self, ty: TypeId, kind: &NativeRuntimeType, sense: bool) -> TypeId {
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
        let mut result = self.native_flow_capture_type(provider, at, reference, declared, initial);
        if result.ty == TypeId::AUTO {
            result.unresolved_auto = !result.incomplete;
            result.ty = TypeId::ANY;
        }
        result
    }

    /// The same traversal used for an immutable capture environment. Incomplete
    /// automatic values remain Auto rather than freezing a provisional loop value.
    pub fn native_flow_capture_type(
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
                ty: if result.incomplete && declared == TypeId::AUTO { TypeId::AUTO } else { result.ty },
                incomplete: result.incomplete,
                unresolved_auto: false,
            };
        };
        provider.flow_error(at, error);
        NativeFlowResult {
            ty: TypeId::ERROR,
            incomplete: false,
            unresolved_auto: false,
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
