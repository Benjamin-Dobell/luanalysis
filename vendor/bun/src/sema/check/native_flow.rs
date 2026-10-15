//! Original-source graph inputs for the shared TS/native flow traversal.
//! Graph records supply semantic operations; they never supply branch joins or
//! loop fixed points. All callbacks re-enter the requesting checker task.

use super::flow_walk::{FlowDomain, FlowStep, FlowType, ReachDomain, ReachStep, WalkState};
use super::*;
use crate::types::TypeId;
#[path = "native_flow_calls.rs"]
mod calls;

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
    /// A single frozen literal child of the queried receiver. The provider supplies its key;
    /// the native transfer, rather than the source graph, owns the resulting value view.
    Descendant,
    PossibleExact,
    PossibleAncestor,
    PossibleDescendant,
    Unaffected,
}

#[derive(Clone, Debug)]
pub enum NativeRuntimeType {
    Nil,
    Boolean,
    Number,
    String,
    Table,
    Function,
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
    Assert { condition: NativeValueKey, input: NativeFlowKey },
    Unsupported(&'static str),
}

#[derive(Clone, Copy)]
enum NativeEffectResult {
    Complete(NativeCallEffects),
    Incomplete { _effects: NativeCallEffects },
    /// An active query depends on the loop approximation being computed. The
    /// caller retains its antecedent facts but cannot publish a final result.
    Pending,
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

/// Original sibling projections of one physical Lua result list. The provider
/// establishes source identity; reached writes invalidate the binding relation
/// and the checker narrows the complete pack through shared flow traversal.
#[derive(Clone, Copy, Debug)]
pub struct NativePackProjection {
    pub group: NativeReferenceKey,
    pub value: NativeValueKey,
    pub input: NativeFlowKey,
    pub index: usize,
}

/// A physical scalar read, frozen before later expressions in its Lua list.
#[derive(Clone, Copy, Debug)]
pub struct NativeFrozenReference {
    pub reference: NativeReferenceKey,
    pub input: NativeFlowKey,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeFlowPackResult {
    pub pack: NativeResultPack,
    pub incomplete: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeIteratorProjection {
    pub frame: NativeReferenceKey,
    /// Physical hidden-frame column: iterator/state occupy columns zero/one.
    pub index: usize,
}

pub trait NativeFlowProvider {
    /// Inspect a value for identity analysis without publishing speculative
    /// source diagnostics; ordinary call checking remains a separate demand.
    fn alias_value_type(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> TypeId { self.value_type(value, input, checker) }
    /// Candidate metadata only; native value origin must attest the object.
    fn alias_environment_candidate(&self, _: NativeValueKey, _: NativeFlowKey) -> bool { false }
    fn reference_alias_initial(&self, _: NativeReferenceKey) -> super::native_aliases::NativeAliasInitial { super::native_aliases::NativeAliasInitial::Opaque }
    fn alias_call_source(&self, _: NativeValueKey, _: NativeFlowKey) -> Option<super::native_aliases::NativeAliasCall> { None }
    fn alias_node_dispatch(&self, _: NativeFlowKey) -> super::native_aliases::NativeAliasDispatch { super::native_aliases::NativeAliasDispatch::Unknown }
    fn function_alias_execution(&self, _: crate::types::NativeFunctionId, _: NativeFlowKey) -> Option<super::native_aliases::NativeAliasExecution> { None }
    fn require_alias_execution(&self, _: NativeValueKey, _: NativeFlowKey) -> Option<Vec<super::native_aliases::NativeAliasExecution>> { None }
    fn require_alias_loader_reference(&self, _: NativeValueKey, _: NativeFlowKey) -> Option<NativeReferenceKey> { None }
    /// Inspect a callee for identity analysis without publishing speculative
    /// source diagnostics; ordinary call checking remains a separate demand.
    fn alias_callee_type(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> TypeId { self.value_type(value, input, checker) }
    /// A lexical variable cell, rather than a slot in a mutable heap object.
    fn reference_is_cell(&self, _: NativeReferenceKey) -> bool { false }
    /// An invariant original written bound, never an inferred or narrowed view.
    fn reference_written_contract(&self, _: NativeReferenceKey, _: &mut Checker<'_, '_>) -> Option<TypeId> { None }
    fn value_alias_source(&self, _: NativeValueKey, _: NativeFlowKey) -> super::native_aliases::NativeAliasValue { super::native_aliases::NativeAliasValue::Unknown }
    /// Candidate canonical slot for an original fixed-member read. Its frozen
    /// receiver's environment identity is proved by the native alias worker.
    fn value_alias_environment_member(&self, _: NativeValueKey, _: NativeFlowKey) -> Option<super::native_aliases::NativeAliasEnvironmentMember> { None }
    /// A unique original literal field at its frozen evaluation input. A
    /// structural property type cannot establish runtime value provenance.
    fn value_alias_member_source(&self, _: NativeValueKey, _: NativeFlowKey, _: NativeTableKey, _: &Checker<'_, '_>) -> Option<super::native_aliases::NativeAliasReturnValue> { None }
    /// Original receiver evaluation for a heap store, frozen before its RHS.
    fn write_alias_receiver(&self, _: NativeFlowKey) -> Option<super::native_aliases::NativeAliasReturnValue> { None }
    fn function_alias_returns(&self, _: crate::types::NativeFunctionId) -> Option<Vec<super::native_aliases::NativeAliasReturn>> { None }
    fn function_alias_exit(&self, _: crate::types::NativeFunctionId) -> Option<NativeFlowKey> { None }
    /// Value capabilities require the assigned runtime value rather than the
    /// declaration-shape reduction used by ordinary TS variable flow.
    fn reference_tracks_runtime_value(&self, _: NativeReferenceKey) -> bool { false }
    fn reference_follows_creation(&self, _: NativeReferenceKey) -> bool { false }
    fn runtime_type_operation(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut Checker<'_, '_>) -> bool { true }
    fn write_member_key(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: NativeFlowKey, _: &mut Checker<'_, '_>) -> Option<NativeTableKey> { None }
    /// A tested fixed child of this original receiver, independent of write metadata.
    fn reference_member_key(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: &mut Checker<'_, '_>) -> Option<NativeTableKey> { None }
    /// An original fixed descendant path. This metadata does not evaluate a
    /// property or infer the identity of an intermediate receiver.
    fn reference_member_path(&self, reference: NativeReferenceKey, member: NativeReferenceKey, checker: &mut Checker<'_, '_>) -> Option<Vec<NativeTableKey>> {
        self.reference_member_key(reference, member, checker).map(|key| vec![key])
    }
    /// Original immutable predicate recipe for a verified assertion argument.
    fn assertion_predicate(&self, _: NativeValueKey, _: NativeFlowKey) -> Option<NativePredicate> { None }
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
    fn write_affects_with_types(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, _: &mut Checker<'_, '_>) -> NativeWriteRelation {
        self.write_affects(reference, target, at)
    }
    fn call_invalidates_reference(&self, _: NativeReferenceKey, _: NativeFlowKey, _: &mut Checker<'_, '_>) -> bool { false }
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
    outer_effect_epoch: u64,
}

/// Checker/task-owned native tables. The graph key includes source snapshot and
/// contextual body facet; value caches also include the frozen input flow.
#[derive(Default)]
pub(super) struct NativeFlowState {
    loop_types: FxHashMap<LoopKey, TypeId>,
    pub loops: Vec<NativeLoopFrame>,
    value_types: FxHashMap<ValueContext, TypeId>,
    pack_types: FxHashMap<ValueContext, NativeResultPack>,
    call_effects: FxHashMap<(ValueContext, u64), NativeCallEffects>,
    pub(super) contextual_owners: FxHashMap<(NativeFunctionId, NativeFlowKey, u64), Option<TypeId>>,
    // Provisional query handles name actual effect records, never TypeIds.
    effect_records: Vec<NativeCallEffects>,
    pub(super) effect_epoch: u64,
    next_effect_epoch: u64,
    reachable: FxHashMap<NativeFlowKey, bool>,
    latest_reachable: Option<(NativeFlowKey, bool)>,
    disabled: FxHashMap<NativeGraphKey, ()>,
    capture_graphs: Vec<NativeGraphKey>,
    call_executions: Vec<calls::NativeCallStateFrame>,
}

impl NativeFlowState {
    fn fresh_effect_epoch(&mut self) {
        self.next_effect_epoch = self.next_effect_epoch.checked_add(1)
            .expect("native effect epoch overflow");
        self.effect_epoch = self.next_effect_epoch;
    }
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
    Call { value: NativeValueKey, input: NativeFlowKey },
    PossibleWrite { at: NativeFlowKey, target: NativeReferenceKey, value: NativeValueKey, input: NativeFlowKey, relation: NativeWriteRelation },
    IncompleteCall,
    DefinitionWrite { at: NativeFlowKey, target: NativeReferenceKey, value: NativeValueKey, input: NativeFlowKey },
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

fn predicate_mentions(provider: &dyn NativeFlowProvider, predicate: &NativePredicate, reference: NativeReferenceKey, pack: bool, checker: &mut Checker<'_, '_>) -> bool {
    match predicate {
        NativePredicate::And(left, right) | NativePredicate::Or(left, right) => predicate_mentions(provider, left, reference, pack, checker) || predicate_mentions(provider, right, reference, pack, checker),
        NativePredicate::Not(inner) => predicate_mentions(provider, inner, reference, pack, checker),
        _ => predicate_subject(predicate).is_some_and(|subject| if pack { provider.pack_projection(subject).is_some_and(|projection| projection.group == reference) || provider.iterator_projection(subject).is_some_and(|projection| projection.frame == reference) } else { subject == reference || provider.reference_member_key(reference, subject, checker).is_some() }),
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
        if walk.declared != TypeId::AUTO && !self.provider.reference_follows_creation(walk.reference) { return None; }
        let declaration = self.provider.reference_declaration(walk.reference)?;
        let same_owner = |graph: NativeGraphKey| graph.snapshot == declaration.owner.snapshot && graph.region == declaration.owner.region;
        if same_owner(at.graph) { return None; }
        if !self.provider.contains_graph(declaration.owner) || declaration.reference.graph != declaration.owner || !self.provider.contains_reference(declaration.reference) {
            self.provider.flow_error(at, NativeFlowError::InvalidReference(declaration.reference));
            return Some(FlowType::new(TypeId::ERROR, false));
        }
        let Some(creation) = self.provider.graph_creation(at.graph) else { return Some(FlowType::new(walk.declared, false)) };
        if declaration.writes.iter().any(|write| !same_owner(write.owner)) {
            return Some(FlowType::new(walk.declared, false));
        }
        let last = declaration.writes.iter().map(|write| write.statement_end).max().unwrap_or(0);
        if !Checker::is_past_last_assignment_position(last, creation.position) {
            return Some(FlowType::new(walk.declared, false));
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

    fn assertion_predicate(&self, at: NativeFlowKey, condition: NativeValueKey, input: NativeFlowKey) -> Result<Option<NativePredicate>, ()> {
        if !self.edge(at, input) { return Err(()) }
        if condition.snapshot != input.graph.snapshot {
            self.provider.flow_error(at, NativeFlowError::InvalidValue(condition));
            return Err(());
        }
        if self.provider.node(input).is_none() {
            self.provider.flow_error(at, NativeFlowError::InvalidNode(input));
            return Err(());
        }
        Ok(self.provider.assertion_predicate(condition, input))
    }

    fn predicate_flow_step(&self, checker: &mut Checker<'_, '_>, at: NativeFlowKey, before: NativeFlowKey, predicate: NativePredicate, sense: bool, walk: &NativeWalk) -> FlowStep<NativeFlowKey, NativeTransfer> {
        if !self.edge(at, before) || !self.predicate_valid(at, &predicate) { return FlowStep::Value(TypeId::ERROR) }
        if let NativePredicate::GenericIteration { frame, .. } = predicate {
            return FlowStep::Follow { before, transfer: (walk.reference == frame).then_some(NativeTransfer::GenericIteration(sense)), count_passed: true };
        }
        let applies = predicate_mentions(self.provider, &predicate, walk.reference, false, checker);
        let projection = predicate_mentions(self.provider, &predicate, walk.reference, true, checker);
        let transfer = if applies { Some(NativeTransfer::Predicate(predicate, sense)) }
            else { projection.then_some(NativeTransfer::PackPredicate { predicate, sense, group: walk.reference }) };
        FlowStep::Follow { before, transfer, count_passed: true }
    }

    fn predicate_reach_step(&self, checker: &mut Checker<'_, '_>, at: NativeFlowKey, before: NativeFlowKey, predicate: NativePredicate, sense: bool) -> ReachStep<NativeFlowKey> {
        if !self.edge(at, before) || !self.predicate_valid(at, &predicate) { return ReachStep::Value(false) }
        match predicate {
            predicate @ (NativePredicate::And(..) | NativePredicate::Or(..) | NativePredicate::Not(..)) => {
                if checker.native_predicate_possible(self.provider, &predicate, sense) { ReachStep::Follow(before) } else { ReachStep::Value(false) }
            }
            NativePredicate::Truthy { value, input, .. } => {
                let value = checker.native_flow_value(self.provider, value, input);
                if checker.lua_truthiness_type(value, sense).is_never() { ReachStep::Value(false) } else { ReachStep::Follow(before) }
            }
            NativePredicate::Equal { .. } => ReachStep::Follow(before),
            NativePredicate::RuntimeType { value, input, kind, equals, .. } => {
                // A lexical `type` candidate is insufficient after a real
                // environment write. Source providers prove the original call.
                if !self.provider.runtime_type_operation(value, input, checker) { return ReachStep::Follow(before); }
                let value = checker.native_flow_value(self.provider, value, input);
                if checker.native_runtime_type(value, &kind, equals == sense).is_never() { ReachStep::Value(false) } else { ReachStep::Follow(before) }
            }
            NativePredicate::NumericIteration { start, limit, step, stage } => {
                match checker.native_numeric_entry(self.provider, start, limit, step, stage) {
                    NumericEntry::Impossible => ReachStep::Value(false),
                    NumericEntry::Known(enters) if enters != sense => ReachStep::Value(false),
                    NumericEntry::Known(_) | NumericEntry::Unknown => ReachStep::Follow(before),
                }
            }
            NativePredicate::GenericIteration { frame, input } => {
                let pack = checker.native_flow_iterator_pack(self.provider, input, frame);
                if checker.native_iterator_filter(pack, sense).ty.is_never() { ReachStep::Value(false) } else { ReachStep::Follow(before) }
            }
            NativePredicate::Unsupported(feature) => {
                self.provider.flow_error(at, NativeFlowError::Unsupported(feature));
                ReachStep::Value(false)
            }
        }
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
    fn join(&self, checker: &mut Checker<'_, '_>, walk: &NativeWalk, types: &[TypeId], initial: TypeId) -> TypeId {
        if self.provider.reference_tracks_runtime_value(walk.reference) || types.iter().any(|&ty| checker.native_has_runtime_table(ty)) {
            // Structural subtyping cannot remove an alternative runtime value:
            // an ordinary function may share assert's shape without its effect.
            checker.union_or_evolving_with(types, false)
        } else { checker.flow_join(types, initial) }
    }
    fn starts_as_declared(&self, checker: &mut Checker<'_, '_>, walk: &mut NativeWalk) -> bool {
        // A runtime table's initial view is one concrete continuation. It is
        // not an absorbing declaration bound for sibling mutation branches.
        walk.initial == walk.declared && !checker.native_has_runtime_table(walk.initial)
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
            NativeFlowNode::Start => match checker.native_call_entry(self.provider, at, walk).or_else(|| self.auto_at_start(checker, walk, at)) {
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
                match self.provider.write_affects_with_types(walk.reference, target, at, checker) {
                    NativeWriteRelation::Exact => {
                        let assigned = checker.native_flow_value(self.provider, value, input);
                        let runtime = checker.native_has_runtime_table(walk.declared)
                            || checker.native_runtime_table_schema(walk.declared).is_some();
                        let assigned = if runtime || checker.native_has_runtime_table(assigned) { checker.regular_type_of_object_literal(assigned) } else { assigned };
                        FlowStep::Value(if self.provider.reference_tracks_runtime_value(walk.reference) || runtime { assigned } else {
                            // Runtime RHS queries retain original capability alternatives.
                            // Ordinary CFA consumes the normalized expression type: a
                            // redundant T & nil arm must not retain nil beside T.
                            let parts = checker.parts(assigned).to_vec();
                            let assigned = checker.union_reduced(&parts);
                            checker.resolved_assignment_type(walk.declared, assigned)
                        })
                    }
                    NativeWriteRelation::Ancestor => FlowStep::Value(walk.declared),
                    NativeWriteRelation::Descendant => FlowStep::Follow {
                        before,
                        transfer: Some(NativeTransfer::DefinitionWrite { at, target, value, input }),
                        count_passed: false,
                    },
                    relation @ (NativeWriteRelation::PossibleExact | NativeWriteRelation::PossibleAncestor | NativeWriteRelation::PossibleDescendant) => FlowStep::Follow {
                        before,
                        transfer: Some(NativeTransfer::PossibleWrite { at, target, value, input, relation }),
                        count_passed: false,
                    },
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
                self.predicate_flow_step(checker, at, before, predicate, sense, walk)
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
                if self.provider.call_invalidates_reference(walk.reference, at, checker) {
                    return FlowStep::Value(walk.declared);
                }
                match checker.native_flow_call_effects(self.provider, value, input) {
                    NativeEffectResult::Pending | NativeEffectResult::Incomplete { .. } => FlowStep::Follow {
                        before,
                        transfer: Some(NativeTransfer::IncompleteCall),
                        count_passed: false,
                    },
                    NativeEffectResult::Complete(NativeCallEffects::Never) => FlowStep::Value(TypeId::UNREACHABLE_NEVER),
                    NativeEffectResult::Complete(NativeCallEffects::Assert { condition, input }) => match self.assertion_predicate(at, condition, input) {
                        Ok(Some(predicate)) => self.predicate_flow_step(checker, at, before, predicate, true, walk),
                        Ok(None) => FlowStep::Follow { before, transfer: None, count_passed: false },
                        Err(()) => FlowStep::Value(TypeId::ERROR),
                    },
                    NativeEffectResult::Complete(NativeCallEffects::None) => FlowStep::Follow {
                        before,
                        transfer: Some(NativeTransfer::Call { value, input }),
                        count_passed: false,
                    },
                    NativeEffectResult::Complete(NativeCallEffects::Unsupported(feature)) => {
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
        if matches!(transfer, NativeTransfer::IncompleteCall) {
            return FlowType::new(previous.ty, true);
        }
        let ty = match transfer {
            NativeTransfer::Call { value, input } => return checker.native_call_state_transfer(self.provider, walk, value, input, previous),
            NativeTransfer::PossibleWrite { at, target, value, input, relation } => {
                let assigned = match relation {
                    NativeWriteRelation::PossibleExact => checker.native_flow_value(self.provider, value, input),
                    NativeWriteRelation::PossibleDescendant => {
                        if checker.native_definition_flow_target(previous.ty)
                            && let Some(key) = self.provider.write_member_key(walk.reference, target, at, checker) {
                            let value = checker.native_flow_value(self.provider, value, input);
                            checker.native_definition_flow_write(previous.ty, key, value)
                        } else { previous.ty }
                    }
                    NativeWriteRelation::PossibleAncestor => checker.native_runtime_table_absent_view(walk.initial),
                    _ => unreachable!("only possible writes enter this transfer"),
                };
                checker.union_or_evolving_with(&[previous.ty, assigned], false)
            }
            NativeTransfer::IncompleteCall => unreachable!(),
            NativeTransfer::DefinitionWrite { at, target, value, input } => {
                if checker.native_definition_flow_target(previous.ty)
                    && let Some(key) = self.provider.write_member_key(walk.reference, target, at, checker) {
                    let value = checker.native_flow_value(self.provider, value, input);
                    checker.native_definition_flow_write(previous.ty, key, value)
                } else { previous.ty }
            }
            NativeTransfer::Iteration(at, stage) => checker.native_iterator_step(self.provider, at, stage, NativeResultPack { ty: previous.ty }).ty,
            NativeTransfer::GenericIteration(sense) => checker.native_iterator_filter(NativeResultPack { ty: previous.ty }, sense).ty,
            NativeTransfer::PackPredicate { predicate, sense, group } => {
                checker.native_narrow_pack(self.provider, NativeResultPack { ty: previous.ty }, &predicate, sense, group).ty
            }
            NativeTransfer::Nothing => previous.ty,
            NativeTransfer::Predicate(_, _) if previous.ty.is_never() => previous.ty,
            NativeTransfer::Predicate(ref predicate, sense) => checker.native_narrow_predicate(self.provider, previous.ty, predicate, sense, walk.reference),
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
            outer_effect_epoch: checker.flow_memo.native_state.effect_epoch,
        });
        checker.flow_memo.native_state.fresh_effect_epoch();
    }
    fn update_loop(&self, checker: &mut Checker<'_, '_>, ty: TypeId) {
        if let Some(frame) = checker.flow_memo.native_state.loops.last_mut() {
            if frame.ty != ty {
                frame.ty = ty;
                checker.flow_memo.native_state.fresh_effect_epoch();
            }
        }
    }
    fn pop_loop(&self, checker: &mut Checker<'_, '_>) {
        if let Some(frame) = checker.flow_memo.native_state.loops.pop() {
            checker.flow_memo.native_state.effect_epoch = frame.outer_effect_epoch;
        }
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
        let epoch = checker.flow_memo.native_state.effect_epoch;
        checker.flow_memo.native_state.fresh_effect_epoch();
        let result = work(checker);
        checker.flow_memo.native_state.value_types = cache;
        checker.flow_memo.native_state.call_effects = calls;
        checker.flow_memo.native_state.pack_types = packs;
        checker.flow_memo.native_state.effect_epoch = epoch;
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
                self.predicate_reach_step(checker, at, before, predicate, sense)
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
                    NativeEffectResult::Pending | NativeEffectResult::Incomplete { .. } => ReachStep::Follow(before),
                    NativeEffectResult::Complete(NativeCallEffects::None) => ReachStep::Follow(before),
                    NativeEffectResult::Complete(NativeCallEffects::Never) => ReachStep::Value(false),
                    NativeEffectResult::Complete(NativeCallEffects::Assert { condition, input }) => match self.assertion_predicate(at, condition, input) {
                        Ok(Some(predicate)) => self.predicate_reach_step(checker, at, before, predicate, true),
                        Ok(None) => ReachStep::Follow(before),
                        Err(()) => ReachStep::Value(false),
                    },
                    NativeEffectResult::Complete(NativeCallEffects::Unsupported(feature)) => {
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
    pub fn is_native_call_execution(&self, graph: NativeGraphKey) -> bool {
        self.flow_memo.native_state.call_executions.last().is_some_and(|frame| frame.entry.graph == graph)
    }
    /// Original runtime capabilities must not lose a structurally redundant
    /// alternative before intrinsic provenance is classified.
    pub fn native_runtime_value_union(&mut self, types: &[TypeId]) -> TypeId {
        self.union_or_evolving_with(types, false)
    }

    fn native_predicate_possible(&mut self, provider: &dyn NativeFlowProvider, predicate: &NativePredicate, sense: bool) -> bool {
        match predicate {
            NativePredicate::And(left, right) => if sense { self.native_predicate_possible(provider, left, true) && self.native_predicate_possible(provider, right, true) } else { self.native_predicate_possible(provider, left, false) || self.native_predicate_possible(provider, right, false) },
            NativePredicate::Or(left, right) => if sense { self.native_predicate_possible(provider, left, true) || self.native_predicate_possible(provider, right, true) } else { self.native_predicate_possible(provider, left, false) && self.native_predicate_possible(provider, right, false) },
            NativePredicate::Not(inner) => self.native_predicate_possible(provider, inner, !sense),
            NativePredicate::Truthy { value, input, .. } => { let value = self.native_flow_value(provider, *value, *input); !self.lua_truthiness_type(value, sense).is_never() },
            NativePredicate::RuntimeType { value, input, kind, equals, .. } => { if !provider.runtime_type_operation(*value, *input, self) { return true; } let value = self.native_flow_value(provider, *value, *input); !self.native_runtime_type(value, kind, *equals == sense).is_never() },
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
            _ if predicate_subject(predicate) != Some(reference) => {
                if let Some(subject) = predicate_subject(predicate)
                    && let Some(key) = provider.reference_member_key(reference, subject, self) {
                    return self.filter(ty, |checker, part| {
                        let member = match checker.native_table_read(part, NativeTableAccessKey::Literal(key)) {
                            Ok(member) => member,
                            Err(error) if checker.native_table_absent_read(part, NativeTableAccessKey::Literal(key), error) => TypeId::UNDEFINED,
                            // An invalid read keeps its original diagnostic; it
                            // cannot prove that this receiver is unreachable.
                            Err(_) => return true,
                        };
                        !checker.native_narrow_predicate(provider, member, predicate, sense, subject).is_never()
                    });
                }
                if self.native_predicate_possible(provider, predicate, sense) { ty } else { TypeId::NEVER }
            }
            NativePredicate::Truthy { .. } => self.lua_truthiness_type(ty, sense),
            NativePredicate::Equal { other, input, equals, .. } => { let other = self.native_flow_value(provider, *other, *input); self.native_equal_flow_type(ty, other, *equals == sense) },
            NativePredicate::RuntimeType { value, input, kind, equals, .. } => if provider.runtime_type_operation(*value, *input, self) { self.native_runtime_type(ty, kind, *equals == sense) } else { ty },
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
                NativePredicate::RuntimeType { value: operation, input, kind, equals, .. } => if provider.runtime_type_operation(*operation, *input, self) { self.native_runtime_type(value, kind, *equals == sense) } else { value },
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
        let result = self.native_flow_type(provider, at, projection.group, TypeId::UNKNOWN, initial.ty);
        let ty = if result.ty == initial.ty || self.native_pack_alternatives(NativeResultPack { ty: result.ty }).is_none() {
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

    /// Reassemble scalar reads from one surviving original result list. The
    /// ordinary list remains authoritative when the source relation was lost.
    pub fn native_flow_correlated_pack(
        &mut self,
        provider: &dyn NativeFlowProvider,
        ordinary: NativeResultPack,
        elements: &[Option<NativeFrozenReference>],
    ) -> NativeFlowPackResult {
        let fallback = |incomplete| NativeFlowPackResult { pack: ordinary, incomplete };
        let Some(ordinary_rows) = self.native_pack_alternatives(ordinary) else { return fallback(false) };
        let [ordinary_row] = ordinary_rows.as_slice() else { return fallback(false) };
        if ordinary_row.tail.is_some() || ordinary_row.fixed.len() != elements.len() {
            return fallback(false);
        }
        let projections: Vec<_> = elements.iter().map(|element| {
            element.and_then(|element| provider.pack_projection(element.reference).map(|projection| (element, projection)))
        }).collect();
        let mut participating = projections.iter().flatten();
        let Some((first, source)) = participating.next().copied() else { return fallback(false) };
        let Some((mut last, _)) = participating.next().copied() else { return fallback(false) };
        for &(element, projection) in projections.iter().flatten() {
            if element.reference.graph != source.group.graph || element.input.graph != source.group.graph
                || projection.group != source.group || projection.value != source.value || projection.input != source.input
            {
                return fallback(false);
            }
            last = element;
        }
        if first.input.graph != source.input.graph || !provider.contains_reference(source.group) {
            return fallback(false);
        }
        let (result, stored) = self.run_memoizable(|checker| {
            let initial = checker.native_flow_pack_value(provider, source.value, source.input);
            // UNKNOWN is the invalidated relation, never the original rows.
            let group = checker.native_flow_type(provider, last.input, source.group, TypeId::UNKNOWN, initial.ty);
            if group.incomplete { return fallback(true); }
            let Some(rows) = checker.native_pack_alternatives(NativeResultPack { ty: group.ty }) else { return fallback(false) };
            let views: Vec<_> = projections.iter().map(|projection| {
                projection.and_then(|(element, _)| provider.reference_written_contract(element.reference, checker))
            }).collect();
            let mut reconstructed = Vec::with_capacity(rows.len());
            for row in rows {
                let row = checker.native_result_pack(&[row]);
                let fixed = ordinary_row.fixed.iter().enumerate().map(|(index, ordinary)| {
                    let Some((_, projection)) = projections[index] else { return *ordinary };
                    let ty = checker.native_pack_slot(row, projection.index);
                    let ty = views[index].map_or(ty, |bound| checker.resolved_assignment_type(bound, ty));
                    super::native_packs::NativePackSlot { ty, optional: false }
                }).collect();
                reconstructed.push(super::native_packs::NativePackAlternative { fixed, tail: None });
            }
            NativeFlowPackResult { pack: checker.native_result_pack(&reconstructed), incomplete: false }
        });
        if stored.is_none() { fallback(true) } else { result }
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
        match kind {
            NativeRuntimeType::Table => return self.native_narrow_runtime_kind(ty, crate::types::NativeRuntimeKind::Table, sense),
            NativeRuntimeType::Function => return self.native_narrow_runtime_kind(ty, crate::types::NativeRuntimeKind::Function, sense),
            _ => {}
        }
        let name = match kind {
            NativeRuntimeType::Nil => known::undefined,
            NativeRuntimeType::Boolean => known::boolean,
            NativeRuntimeType::Number => known::number,
            NativeRuntimeType::String => known::string,
            NativeRuntimeType::Table | NativeRuntimeType::Function => unreachable!(),
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

    /// Source visitation can stop only after a completed reachability result.
    /// A provisional false during a loop/query cycle must not hide diagnostics.
    pub fn native_flow_definitely_unreachable(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
    ) -> bool {
        let (reachable, stored) = self.run_memoizable(|checker| checker.native_flow_reachable(provider, at));
        !reachable && stored.is_some()
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
    ) -> NativeEffectResult {
        let epoch = self.flow_memo.native_state.effect_epoch;
        let key = ((value, input), epoch);
        if value.snapshot != input.graph.snapshot
            || !provider.contains_graph(input.graph)
            || provider.node(input).is_none()
        {
            provider.flow_error(input, NativeFlowError::InvalidValue(value));
            return NativeEffectResult::Complete(NativeCallEffects::Unsupported("invalid frozen call context"));
        }
        if let Some(&cached) = self.flow_memo.native_state.call_effects.get(&key) {
            return NativeEffectResult::Complete(cached);
        }
        let query = super::Query::NativeFlowCallEffects(value, input, epoch);
        if let Some(raw) = self.provisional(query) {
            return NativeEffectResult::Incomplete { _effects: self.flow_memo.native_state.effect_records[raw as usize] };
        }
        if !self.enter(query) {
            // `enter` has tainted the dependent queries. An effect that needs
            // its own in-progress loop is unavailable, not proof of no effect.
            return NativeEffectResult::Pending;
        }
        let begun = self.begin_memoizable();
        let effects = provider.call_effects(value, input, self);
        let stored = self.end_memoizable(begun);
        match self.leave(query) {
            Ok(_) if stored.is_some() && !self.is_stack_low() => {
                self.flow_memo.native_state.call_effects.insert(key, effects);
            }
            Err(open) => {
                let raw = self.flow_memo.native_state.effect_records.len() as u64;
                self.flow_memo.native_state.effect_records.push(effects);
                self.cache_provisionally(query, raw, open);
                return NativeEffectResult::Incomplete { _effects: effects };
            }
            _ => return NativeEffectResult::Incomplete { _effects: effects },
        }
        NativeEffectResult::Complete(effects)
    }
}
