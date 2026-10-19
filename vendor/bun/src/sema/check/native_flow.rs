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

/// What an original callee names lexically, for ordinary standard-operation
/// classification (Luanalysis v1 semantics: intrinsics are recognised by name,
/// not traced through heap provenance).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeLexicalStandard {
    /// Leave the classification to the flow-sensitive provenance walk.
    Unknown,
    /// It cannot name a standard intrinsic.
    Ordinary,
    /// It names this original global environment member, which no source
    /// in the project replaces.
    Global(NativeReferenceKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeWriteRelation {
    Exact,
    /// Replacing an ancestor invalidates the descendant reference facts.
    Ancestor,
    /// A single frozen child of the queried receiver. The provider supplies its key;
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
    Userdata,
    Thread,
}

/// Runtime categories represented by selected original nominal declarations,
/// rather than by structural table or function carriers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeOpaqueRuntimeKind { Userdata, Thread }

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
    /// An original call and its arguments, frozen at their evaluation input.
    /// The applicable signature decides which argument the predicate targets.
    Call {
        value: NativeValueKey,
        input: NativeFlowKey,
        arguments: Vec<Option<NativeReferenceKey>>,
        /// Whether a true condition means the selected guard returned false.
        negated: bool,
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

/// Passive observation around an actual source receiver read. It owns no
/// checker query or memo scope and cannot change the source computation's
/// recursion, provisional capabilities, diagnostics, or contextual lifetime.
#[must_use]
pub struct NativeEnvironmentRead {
    key: (NativeValueKey, NativeFlowKey),
    generation: u64,
    counters: (u64, u64),
    frames: usize,
    frame_serial: Option<u64>,
    taints: u64,
    cuts: u64,
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
    /// Complete keys of a directly evaluated literal prove this raw field
    /// absent at construction. Aliases/paths to literals, dynamic keys, and
    /// structural types cannot supply this fact.
    fn value_alias_member_absent(&self, _: NativeValueKey, _: NativeFlowKey, _: NativeTableKey, _: &Checker<'_, '_>) -> bool { false }
    /// Original receiver evaluation for a heap store, frozen before its RHS.
    fn write_alias_receiver(&self, _: NativeFlowKey) -> Option<super::native_aliases::NativeAliasReturnValue> { None }
    /// Original field/index receiver, frozen before an index key is evaluated.
    fn read_alias_receiver(&self, _: NativeFlowKey) -> Option<super::native_aliases::NativeAliasReturnValue> { None }
    fn function_alias_returns(&self, _: crate::types::NativeFunctionId) -> Option<Vec<super::native_aliases::NativeAliasReturn>> { None }
    fn function_alias_exit(&self, _: crate::types::NativeFunctionId) -> Option<NativeFlowKey> { None }
    /// Value capabilities require the assigned runtime value rather than the
    /// declaration-shape reduction used by ordinary TS variable flow.
    fn reference_tracks_runtime_value(&self, _: NativeReferenceKey) -> bool { false }
    fn reference_follows_creation(&self, _: NativeReferenceKey) -> bool { false }
    fn runtime_type_operation(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut Checker<'_, '_>) -> bool { true }
    /// The truthiness of an original literal value (`nil`, `false`, `true`, a
    /// number, string, table or function literal); `None` for anything else.
    /// Reachability is structural: it never asks for operand types.
    fn value_literal_truthiness(&self, _: NativeValueKey) -> Option<bool> { None }
    /// What an original callee value names lexically; see `NativeLexicalStandard`.
    fn value_lexical_standard(&self, _: NativeValueKey, _: NativeFlowKey) -> NativeLexicalStandard { NativeLexicalStandard::Unknown }
    /// No source in the project may mutate the module loader (`require`,
    /// `package` loader fields), so ordinary module checking needs no walk.
    fn loader_never_mutated(&self, _: NativeGraphKey) -> bool { false }
    fn write_member_key(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: NativeFlowKey, _: &mut Checker<'_, '_>) -> Option<NativeTableKey> { None }
    /// The original computed child key and its frozen evaluation input. The
    /// existing descendant relation, not this recipe, selects the receiver.
    fn write_index_value(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: NativeFlowKey) -> Option<(NativeValueKey, NativeFlowKey)> { None }
    /// Whether the store at `at` appends to its receiver's sequence (`list[#list + 1] = value`).
    fn write_index_appends(&self, _: NativeFlowKey) -> bool { false }
    /// Pure allocation/store scope metadata. A contextual or written receiver
    /// contract is independently excluded by the native value carrier.
    fn runtime_table_store_in_scope(&self, _: super::native_named::NativeTypeDeclarationId, _: NativeFlowKey) -> bool { false }
    /// A tested fixed child of this original receiver, independent of write metadata.
    fn reference_member_key(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: &mut Checker<'_, '_>) -> Option<NativeTableKey> { None }
    /// An original fixed descendant path. This metadata does not evaluate a
    /// property or infer the identity of an intermediate receiver.
    fn reference_member_path(&self, reference: NativeReferenceKey, member: NativeReferenceKey, checker: &mut Checker<'_, '_>) -> Option<Vec<NativeTableKey>> {
        self.reference_member_key(reference, member, checker).map(|key| vec![key])
    }
    /// Existing canonical proper prefixes of one original member address.
    /// This supplies no runtime alias relationship or inferred receiver type.
    fn reference_proper_prefixes(&self, _: NativeReferenceKey) -> Option<Vec<NativeReferenceKey>> { None }
    /// Original immutable predicate recipe for a verified assertion argument.
    fn assertion_predicate(&self, _: NativeValueKey, _: NativeFlowKey) -> Option<NativePredicate> { None }
    /// Canonicalize original predicate addresses using their frozen receiver
    /// evaluations. This is a flow-only demand; structural reachability never
    /// asks for runtime receiver identities. None retains the immutable recipe.
    /// Pure source-presence gate: ordinary predicates require no typed query
    /// frame, cloned tree, or memo entry.
    fn predicate_needs_types(&self, _: NativeFlowKey, _: NativeReferenceKey, _: &NativePredicate) -> bool { false }
    fn predicate_with_types(&self, _: NativeFlowKey, _: NativePredicate, _: &mut Checker<'_, '_>) -> Option<NativePredicate> { None }

    fn call_type_predicate(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut Checker<'_, '_>) -> Option<super::native_signatures::NativeTypePredicate> { None }
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
    /// `write_affects_with_types` for a consumer that treats `Descendant`,
    /// `PossibleDescendant` and `Unaffected` alike. A provider may omit proofs
    /// that could only distinguish those relations.
    fn write_affects_ignoring_descendants(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, checker: &mut Checker<'_, '_>) -> NativeWriteRelation {
        self.write_affects_with_types(reference, target, at, checker)
    }
    fn call_invalidates_reference(&self, _: NativeReferenceKey, _: NativeFlowKey, _: &mut Checker<'_, '_>) -> bool { false }
    /// Original explicit loader-mutating call metadata, without effect inference.
    fn known_standard_loader_write(&self, _: NativeFlowKey) -> bool { false }
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
type LoopKey = (NativeFlowKey, NativeReferenceKey, TypeId, TypeId, NativeFlowRole);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum NativeFlowRole {
    Reference,
    IteratorFrame,
}

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
    environment_origins: FxHashMap<ValueContext, super::native_globals::NativeEnvironmentId>,
    environment_reads: FxHashMap<ValueContext, u64>,
    next_environment_read: u64,
    pack_types: FxHashMap<ValueContext, NativeResultPack>,
    call_effects: FxHashMap<(ValueContext, u64), NativeCallEffects>,
    call_type_predicates: FxHashMap<(ValueContext, u64), Option<super::native_signatures::NativeTypePredicate>>,
    typed_predicates: FxHashMap<(NativeFlowKey, u64), NativePredicate>,
    pub(super) signature_type_predicates: FxHashMap<SigId, Option<super::decl::Predicate>>,
    pub(super) contextual_owners: FxHashMap<(NativeFunctionId, NativeFlowKey, u64), Option<TypeId>>,
    // Provisional query handles name actual effect records, never TypeIds.
    effect_records: Vec<NativeCallEffects>,
    pub(super) effect_epoch: u64,
    next_effect_epoch: u64,
    reachable: FxHashMap<NativeFlowKey, bool>,
    latest_reachable: Option<(NativeFlowKey, bool)>,
    disabled: FxHashMap<NativeGraphKey, ()>,
    capture_graphs: Vec<NativeGraphKey>,
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
    role: NativeFlowRole,
    traversal: WalkState<NativeFlowKey>,
}

struct NativeDomain<'a> {
    provider: &'a dyn NativeFlowProvider,
    graph: NativeGraphKey,
}

enum NativeTransfer {
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

fn predicate_subject(provider: &dyn NativeFlowProvider, predicate: &NativePredicate, checker: &mut Checker<'_, '_>) -> Option<NativeReferenceKey> {
    match predicate {
        NativePredicate::Truthy { subject, .. } => *subject,
        NativePredicate::Call { value, input, arguments, .. } => {
            let predicate = checker.native_flow_call_type_predicate(provider, *value, *input)?;
            arguments.get(predicate.parameter).copied().flatten()
        }
        NativePredicate::Equal { subject, .. } | NativePredicate::RuntimeType { subject, .. } => Some(*subject),
        _ => None,
    }
}

fn predicate_mentions(provider: &dyn NativeFlowProvider, predicate: &NativePredicate, reference: NativeReferenceKey, pack: bool, checker: &mut Checker<'_, '_>) -> bool {
    match predicate {
        NativePredicate::And(left, right) | NativePredicate::Or(left, right) => predicate_mentions(provider, left, reference, pack, checker) || predicate_mentions(provider, right, reference, pack, checker),
        NativePredicate::Not(inner) => predicate_mentions(provider, inner, reference, pack, checker),
        NativePredicate::Call { arguments, .. } if !arguments.iter().flatten().any(|&subject| {
            if pack { provider.pack_projection(subject).is_some_and(|projection| projection.group == reference) || provider.iterator_projection(subject).is_some_and(|projection| projection.frame == reference) }
            else { subject == reference || provider.reference_member_key(reference, subject, checker).is_some() }
        }) => false,

        _ => predicate_subject(provider, predicate, checker).is_some_and(|subject| if pack { provider.pack_projection(subject).is_some_and(|projection| projection.group == reference) || provider.iterator_projection(subject).is_some_and(|projection| projection.frame == reference) } else { subject == reference || provider.reference_member_key(reference, subject, checker).is_some() }),
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
    /// A store at a computed key whose type is one literal (`t[K.NAME] = v` with `K.NAME` the
    /// string 'name') stores that field, as TypeScript reads a late-bound assignment.
    fn literal_store_key(&self, checker: &mut Checker<'_, '_>, walk: &NativeWalk, at: NativeFlowKey, target: NativeReferenceKey) -> Option<NativeTableKey> {
        let (key, input) = self.provider.write_index_value(walk.reference, target, at)?;
        let (ty, completed) = checker.run_memoizable(|checker| checker.native_flow_value(self.provider, key, input));
        completed?;
        // Numeric keys keep the sparse numeric inference that builds sequences.
        match checker.native_table_access(super::native_tables::NativeTableAccessKey::Type(ty)) {
            Ok(super::native_tables::NativeTableAccessKey::Literal(key @ NativeTableKey::String(_))) => Some(key),
            _ => None,
        }
    }
    fn numeric_store_transfer(&self, checker: &mut Checker<'_, '_>, walk: &NativeWalk, at: NativeFlowKey, target: NativeReferenceKey, value: NativeValueKey, input: NativeFlowKey, previous: FlowType) -> Option<FlowType> {
        if !checker.native_has_runtime_table(previous.ty) { return None; }
        let (key, key_input) = self.provider.write_index_value(walk.reference, target, at)?;
        let (candidates, completed) = checker.run_memoizable(|checker| {
            let key = checker.native_flow_value(self.provider, key, key_input);
            checker.native_runtime_numeric_store_candidates(self.provider, at, previous.ty, key)
        });
        if completed.is_none() || matches!(candidates, Err(super::native_tables::NativeTableError::PendingContext)) {
            return Some(FlowType::new(previous.ty, true));
        }
        let Ok(candidates) = candidates else { return Some(previous); };
        if candidates.is_empty() { return Some(previous); }
        let append = self.provider.write_index_appends(at);
        let (ty, completed) = checker.run_memoizable(|checker| {
            let value = checker.native_flow_value(self.provider, value, input);
            checker.native_runtime_table_flow_numeric_write(previous.ty, &candidates, value, append)
        });
        Some(FlowType::new(ty, previous.incomplete || completed.is_none()))
    }
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
            NativePredicate::Call { value, input, arguments, .. } => {
                if !self.edge(at, *input) { return false; }
                if value.snapshot != input.graph.snapshot {
                    self.provider.flow_error(at, NativeFlowError::InvalidValue(*value));
                    return false;
                }
                for subject in arguments.iter().flatten() {
                    if subject.graph != self.graph || !self.provider.contains_reference(*subject) {
                        self.provider.flow_error(at, NativeFlowError::InvalidReference(*subject));
                        return false;
                    }
                }
                return true;
            }
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
        let predicate = if matches!(predicate, NativePredicate::NumericIteration { .. } | NativePredicate::GenericIteration { .. } | NativePredicate::Unsupported(_)) {
            predicate
        } else {
            match checker.native_flow_predicate_with_types(self.provider, at, walk.reference, &predicate) {
                Ok(Some(typed)) => {
                    if !self.predicate_valid(at, &typed) { return FlowStep::Value(TypeId::ERROR) }
                    typed
                },
                Ok(None) => predicate,
                // An unfinished receiver identity cannot publish a completed
                // reference walk, even when its raw source path was unchanged.
                Err(()) => return FlowStep::Follow { before, transfer: Some(NativeTransfer::IncompleteCall), count_passed: false },
            }
        };
        if let NativePredicate::GenericIteration { frame, .. } = predicate {
            return FlowStep::Follow { before, transfer: (walk.reference == frame).then_some(NativeTransfer::GenericIteration(sense)), count_passed: true };
        }
        let applies = predicate_mentions(self.provider, &predicate, walk.reference, false, checker);
        let projection = predicate_mentions(self.provider, &predicate, walk.reference, true, checker);
        let transfer = if applies { Some(NativeTransfer::Predicate(predicate, sense)) }
            else { projection.then_some(NativeTransfer::PackPredicate { predicate, sense, group: walk.reference }) };
        FlowStep::Follow { before, transfer, count_passed: true }
    }

    /// Operand types refine TypeScript's structural reachability. A return type
    /// re-requested by the refinement is cut (`on_reentry`); the structural
    /// answer, a reachable predecessor, then stands.
    fn reach_refinement(&self, checker: &mut Checker<'_, '_>, before: NativeFlowKey, refine: impl FnOnce(&mut Checker<'_, '_>) -> ReachStep<NativeFlowKey>) -> ReachStep<NativeFlowKey> {
        let cuts = checker.reach_cuts;
        checker.reach_refinements.push(checker.stack.len());
        let step = refine(checker);
        checker.reach_refinements.pop();
        if checker.reach_cuts != cuts { ReachStep::Follow(before) } else { step }
    }

    /// Reachability is structural, as in TypeScript: only literal conditions
    /// (Lua truthiness), literal numeric ranges and explicit nonreturning or
    /// assertion contracts make a branch unreachable. Operand types are never
    /// requested: type-based dead-code findings belong to separate inspections.
    fn predicate_reach_step(&self, checker: &mut Checker<'_, '_>, at: NativeFlowKey, before: NativeFlowKey, predicate: NativePredicate, sense: bool) -> ReachStep<NativeFlowKey> {
        if !self.edge(at, before) || !self.predicate_valid(at, &predicate) { return ReachStep::Value(false) }
        match predicate {
            NativePredicate::NumericIteration { start, limit, step, stage } => match checker.native_literal_numeric_entry(start, limit, step, stage) {
                NumericEntry::Impossible => ReachStep::Value(false),
                NumericEntry::Known(enters) if enters != sense => ReachStep::Value(false),
                NumericEntry::Known(_) | NumericEntry::Unknown => ReachStep::Follow(before),
            },
            NativePredicate::Unsupported(feature) => {
                self.provider.flow_error(at, NativeFlowError::Unsupported(feature));
                ReachStep::Value(false)
            }
            predicate => if self.literal_predicate_possible(&predicate, sense) { ReachStep::Follow(before) } else { ReachStep::Value(false) },
        }
    }

    fn literal_predicate_possible(&self, predicate: &NativePredicate, sense: bool) -> bool {
        match predicate {
            NativePredicate::And(left, right) => if sense { self.literal_predicate_possible(left, true) && self.literal_predicate_possible(right, true) }
                else { self.literal_predicate_possible(left, false) || self.literal_predicate_possible(right, false) },
            NativePredicate::Or(left, right) => if sense { self.literal_predicate_possible(left, true) || self.literal_predicate_possible(right, true) }
                else { self.literal_predicate_possible(left, false) && self.literal_predicate_possible(right, false) },
            NativePredicate::Not(inner) => self.literal_predicate_possible(inner, !sense),
            NativePredicate::Truthy { value, .. } | NativePredicate::Call { value, .. } => self.provider.value_literal_truthiness(*value).is_none_or(|truthy| truthy == sense),
            _ => true,
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
            NativeFlowNode::Start => match self.auto_at_start(checker, walk, at) {
                Some(result) => FlowStep::Complete(result),
                None => FlowStep::Initial,
            },
            NativeFlowNode::Unreachable | NativeFlowNode::Return { .. } => {
                FlowStep::Value(self.unreachable_branch(checker, walk))
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
                    if edges.is_empty() {
                        FlowStep::Value(self.unreachable_branch(checker, walk))
                    } else { FlowStep::Loop { edges } }
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
                    NativeEffectResult::Complete(NativeCallEffects::None) => FlowStep::Follow { before, transfer: None, count_passed: false },
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
            NativeTransfer::PossibleWrite { at, target, value, input, relation } => {
                let assigned = match relation {
                    NativeWriteRelation::PossibleExact => {
                        // A computed key writes this field only if it can be this field's key,
                        // and exactly this field if its type is that one literal.
                        if let Some(read) = self.provider.write_member_key(walk.reference, walk.reference, at, checker)
                            && let Some((key, key_input)) = self.provider.write_index_value(walk.reference, target, at) {
                            let (written, completed) = checker.run_memoizable(|checker| checker.native_flow_value(self.provider, key, key_input));
                            if completed.is_some() {
                                let read_type = checker.native_table_key_type(read);
                                if !checker.is_assignable(read_type, written) { return previous; }
                                if let Ok(super::native_tables::NativeTableAccessKey::Literal(literal)) = checker.native_table_access(super::native_tables::NativeTableAccessKey::Type(written))
                                    && read.normalize().ok() == Some(literal) {
                                    return FlowType::new(checker.native_flow_value(self.provider, value, input), previous.incomplete);
                                }
                            }
                        }
                        checker.native_flow_value(self.provider, value, input)
                    }
                    NativeWriteRelation::PossibleDescendant => {
                        if checker.native_definition_flow_target(previous.ty)
                            && let Some(key) = self.provider.write_member_key(walk.reference, target, at, checker) {
                            let value = checker.native_flow_value(self.provider, value, input);
                            checker.native_definition_flow_write(previous.ty, key, value)
                        } else if checker.native_definition_flow_target(previous.ty)
                            && let Some(key) = self.literal_store_key(checker, walk, at, target) {
                            // The literal key is this field: the store is exact.
                            let value = checker.native_flow_value(self.provider, value, input);
                            return FlowType::new(checker.native_definition_flow_write(previous.ty, key, value), previous.incomplete);
                        } else if let Some(result) = self.numeric_store_transfer(checker, walk, at, target, value, input, previous) {
                            // Every numeric slot remains sparse, which already
                            // includes the unaffected continuation of this
                            // possible receiver/key write.
                            return result;
                        } else { previous.ty }
                    }
                    NativeWriteRelation::PossibleAncestor => checker.native_runtime_table_absent_view(walk.initial),
                    _ => unreachable!("only possible writes enter this transfer"),
                };
                checker.union_or_evolving_with(&[previous.ty, assigned], false)
            }
            NativeTransfer::IncompleteCall => unreachable!(),
            NativeTransfer::DefinitionWrite { at, target, value, input } => {
                // A store below a fixed member (`t.a.b = v` for `t`) writes the table it holds.
                if checker.native_definition_flow_target(previous.ty)
                    && let Some(path) = self.provider.reference_member_path(walk.reference, target, checker)
                    && path.len() > 1 {
                    let value = checker.native_flow_value(self.provider, value, input);
                    checker.native_definition_flow_write_path(previous.ty, &path, value)
                } else if checker.native_definition_flow_target(previous.ty)
                    && let Some(key) = self.provider.write_member_key(walk.reference, target, at, checker)
                        .or_else(|| self.literal_store_key(checker, walk, at, target)) {
                    let value = checker.native_flow_value(self.provider, value, input);
                    checker.native_definition_flow_write(previous.ty, key, value)
                } else if let Some(result) = self.numeric_store_transfer(checker, walk, at, target, value, input, previous) {
                    return result;
                } else { previous.ty }
            }
            NativeTransfer::Iteration(at, stage) => checker.native_iterator_step(self.provider, at, stage, NativeResultPack { ty: previous.ty }).ty,
            NativeTransfer::GenericIteration(sense) => checker.native_iterator_filter(NativeResultPack { ty: previous.ty }, sense).ty,
            NativeTransfer::PackPredicate { predicate, sense, group } => {
                let scope = checker.begin_scope();
                let (ty, completed) = checker.run_memoizable(|checker| checker.native_narrow_pack(self.provider, NativeResultPack { ty: previous.ty }, &predicate, sense, group).ty);
                let settled = checker.end_scope(scope).is_ok() && completed.is_some();
                return FlowType::new(ty, previous.incomplete || !settled);
            }
            NativeTransfer::Nothing => previous.ty,
            NativeTransfer::Predicate(_, _) if previous.ty.is_never() => previous.ty,
            NativeTransfer::Predicate(ref predicate, sense) => {
                let scope = checker.begin_scope();
                let (ty, completed) = checker.run_memoizable(|checker| checker.native_narrow_predicate(self.provider, previous.ty, predicate, sense, walk.reference, walk.declared));
                let settled = checker.end_scope(scope).is_ok() && completed.is_some();
                return FlowType::new(ty, previous.incomplete || !settled);
            },
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
    fn unreachable_branch(&self, checker: &mut Checker<'_, '_>, walk: &NativeWalk) -> TypeId {
        if walk.role == NativeFlowRole::IteratorFrame {
            // No reachable hidden-state rows can flow around this dead edge.
            // UNKNOWN is the protocol bound, not an iterator/state/control row.
            return checker.native_result_pack(&[]).ty;
        }
        // Like TS Flow::Unreachable, an entirely dead continuation is checked
        // against the declaration. Per-reference predicate Never stays intact.
        checker.convert_auto_to_any(walk.declared)
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
            .get(&(node, walk.reference, walk.declared, initial, walk.role))
            .copied()
    }
    fn loop_active(
        &self,
        checker: &Checker<'_, '_>,
        walk: &NativeWalk,
        node: NativeFlowKey,
        initial: TypeId,
    ) -> Option<(TypeId, usize)> {
        let key = (node, walk.reference, walk.declared, initial, walk.role);
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
            key: (node, walk.reference, walk.declared, initial, walk.role),
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
            .insert((node, walk.reference, walk.declared, initial, walk.role), ty);
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
            if walk.role == NativeFlowRole::IteratorFrame {
                checker.native_result_pack(&[]).ty
            } else { walk.declared }
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
        let origins = std::mem::take(&mut checker.flow_memo.native_state.environment_origins);
        let reads = std::mem::take(&mut checker.flow_memo.native_state.environment_reads);
        let calls = std::mem::take(&mut checker.flow_memo.native_state.call_effects);
        let packs = std::mem::take(&mut checker.flow_memo.native_state.pack_types);
        let epoch = checker.flow_memo.native_state.effect_epoch;
        checker.flow_memo.native_state.fresh_effect_epoch();
        let result = work(checker);
        checker.flow_memo.native_state.value_types = cache;
        checker.flow_memo.native_state.environment_origins = origins;
        checker.flow_memo.native_state.environment_reads = reads;
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
                // Only an explicit nonreturning or assertion contract is a
                // structural effect; its classification is a refinement too.
                self.reach_refinement(checker, before, |checker| match checker.native_flow_call_effects(self.provider, value, input) {
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
                })
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
            NativePredicate::Truthy { value, input, .. } | NativePredicate::Call { value, input, .. } => { let value = self.native_flow_value(provider, *value, *input); !self.lua_truthiness_type(value, sense).is_never() },
            NativePredicate::RuntimeType { value, input, kind, equals, .. } => { if !provider.runtime_type_operation(*value, *input, self) { return true; } let value = self.native_flow_value(provider, *value, *input); !self.native_runtime_type(value, kind, *equals == sense).is_never() },
            _ => true,
        }
    }

    fn native_narrow_predicate(&mut self, provider: &dyn NativeFlowProvider, ty: TypeId, predicate: &NativePredicate, sense: bool, reference: NativeReferenceKey, declared: TypeId) -> TypeId {
        match predicate {
            NativePredicate::Not(inner) => self.native_narrow_predicate(provider, ty, inner, !sense, reference, declared),
            NativePredicate::And(left, right) | NativePredicate::Or(left, right) => {
                let conjunction = matches!(predicate, NativePredicate::And(..));
                if conjunction == sense {
                    let left = self.native_narrow_predicate(provider, ty, left, sense, reference, declared);
                    self.native_narrow_predicate(provider, left, right, sense, reference, declared)
                } else {
                    let bypass = self.native_narrow_predicate(provider, ty, left, sense, reference, declared);
                    let evaluated = self.native_narrow_predicate(provider, ty, left, !sense, reference, declared);
                    let evaluated = self.native_narrow_predicate(provider, evaluated, right, sense, reference, declared);
                    self.union_reduced(&[bypass, evaluated])
                }
            }
            _ if predicate_subject(provider, predicate, self) != Some(reference) => {
                if let Some(subject) = predicate_subject(provider, predicate, self)
                    && let Some(key) = provider.reference_member_key(reference, subject, self) {
                    // Like getDiscriminantPropertyAccess, a member predicate
                    // filters parent alternatives only for an original/current
                    // union. A lone record is not Never merely because one of
                    // its members has an impossible branch. Keep the original
                    // union after an earlier predicate selected one constituent.
                    if !self.is_union(declared) && !self.is_union(ty) { return ty; }
                    // A member no alternative has may be stored through another reference to
                    // the table: its absence everywhere is no discriminant, and proves nothing.
                    let parts = self.parts(ty).to_vec();
                    if parts.iter().all(|&part| matches!(self.native_table_read(part, NativeTableAccessKey::Literal(key)),
                        Err(error) if self.native_table_absent_read(part, NativeTableAccessKey::Literal(key), error))) { return ty; }
                    return self.filter(ty, |checker, part| {
                        let member = match checker.native_table_read(part, NativeTableAccessKey::Literal(key)) {
                            Ok(member) => member,
                            Err(error) if checker.native_table_absent_read(part, NativeTableAccessKey::Literal(key), error) => TypeId::UNDEFINED,
                            // An invalid read keeps its original diagnostic; it
                            // cannot prove that this receiver is unreachable.
                            Err(_) => return true,
                        };
                        !checker.native_narrow_predicate(provider, member, predicate, sense, subject, member).is_never()
                    });
                }
                // Reachability checks the whole predicate separately. An
                // impossible operand about a different reference does not
                // erase this reference's type inside an otherwise related
                // compound condition (for example, object.flag && false).
                ty
            }
            NativePredicate::Call { value, input, negated, .. } => {
                match self.native_flow_call_type_predicate(provider, *value, *input) {
                    Some(predicate) => self.narrowed_to(ty, predicate.ty, sense != *negated, super::flow::CheckDerived::No),
                    None => ty,
                }
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
        let index = predicate_subject(provider, predicate, self).and_then(|reference| {
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
                NativePredicate::Call { value: call, input, negated, .. } => {
                    match self.native_flow_call_type_predicate(provider, *call, *input) {
                        Some(predicate) => self.narrowed_to(value, predicate.ty, sense != *negated, super::flow::CheckDerived::No),
                        None => value,
                    }
                }
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
                // A part is possible if the slot can hold it. A type parameter's slot holds only
                // what relates to it: nil is not a possible `R`, as TypeScript does not compare
                // undefined with an unconstrained type parameter.
                let possible = if checker.could_contain_type_variables(part) || checker.could_contain_type_variables(projected) {
                    checker.is_assignable(part, projected) || checker.is_assignable(projected, part)
                } else { checker.are_comparable(part, projected) };
                if possible { part } else { TypeId::NEVER }
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

    /// Whether a numeric `for` is entered, from original numeric literals
    /// alone (structural reachability): a non-literal bound leaves both paths
    /// open, and a literal zero step raises before either continuation.
    fn native_literal_numeric_entry(&self, start: NativeNumericInput, limit: NativeNumericInput, step: Option<NativeNumericInput>, stage: NativeNumericIterationStage) -> NumericEntry {
        let step = step.map_or(Some(NativeNumericConstant::Integer(1)), |step| step.literal);
        if matches!(step, Some(NativeNumericConstant::Integer(0)))
            || matches!(step, Some(NativeNumericConstant::Float(value)) if value == 0.0)
        { return NumericEntry::Impossible }
        if stage == NativeNumericIterationStage::Next { return NumericEntry::Unknown }
        let (Some(start), Some(limit), Some(step)) = (start.literal, limit.literal, step) else { return NumericEntry::Unknown };
        numeric_first_entry(Some(start), Some(limit), Some(step))
    }

    pub(super) fn native_runtime_type(&mut self, ty: TypeId, kind: &NativeRuntimeType, sense: bool) -> TypeId {
        match kind {
            // Lua has one nil value. JS `typeof x !== "undefined"` retains
            // null, including a synthetic T & null branch for type parameters.
            // Exclude both native nullish carriers through the shared worker.
            NativeRuntimeType::Nil if !sense => return self.non_nullable(ty),
            NativeRuntimeType::Table => return self.native_narrow_runtime_kind(ty, crate::types::NativeRuntimeKind::Table, sense),
            NativeRuntimeType::Function => return self.native_narrow_runtime_kind(ty, crate::types::NativeRuntimeKind::Function, sense),
            NativeRuntimeType::Userdata => return self.native_narrow_opaque_runtime_kind(ty, NativeOpaqueRuntimeKind::Userdata, sense),
            NativeRuntimeType::Thread => return self.native_narrow_opaque_runtime_kind(ty, NativeOpaqueRuntimeKind::Thread, sense),
            _ => {}
        }
        let name = match kind {
            NativeRuntimeType::Nil => known::undefined,
            NativeRuntimeType::Boolean => known::boolean,
            NativeRuntimeType::Number => known::number,
            NativeRuntimeType::String => known::string,
            NativeRuntimeType::Table | NativeRuntimeType::Function | NativeRuntimeType::Userdata | NativeRuntimeType::Thread => unreachable!(),
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
        self.native_flow_type_for_role(provider, at, reference, declared, initial, NativeFlowRole::Reference)
    }

    /// Hidden generic-for state has a correlated pack protocol rather than an
    /// ordinary declaration type. Its dead edges carry no reachable rows; the
    /// original IteratorStart and call workers still determine every live row.
    pub(super) fn native_flow_iterator_frame(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        reference: NativeReferenceKey,
    ) -> NativeFlowResult {
        self.native_flow_type_for_role(provider, at, reference, TypeId::UNKNOWN, TypeId::NEVER, NativeFlowRole::IteratorFrame)
    }

    fn native_flow_type_for_role(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        reference: NativeReferenceKey,
        declared: TypeId,
        initial: TypeId,
        role: NativeFlowRole,
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
                role,
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

    /// A passive observer may retain facts only from an entirely settled
    /// context. Inherited provisional frames must not become stable facts just
    /// because a nested source read itself raises no additional counter.
    fn environment_read_is_clean(&self) -> bool {
        self.lowest_taint == usize::MAX && !self.relation_too_complex && self.reliability == 0
            && !self.frames.iter().any(|frame| frame.tainted || frame.circular || frame.incomplete_flow)
    }

    /// Observe an existing source read without entering a query, opening a
    /// memo scope, or changing any state that controls the source computation.
    pub fn begin_environment_read(&mut self, value: NativeValueKey, input: NativeFlowKey) -> Option<NativeEnvironmentRead> {
        let key = (value, input);
        if value.snapshot != input.graph.snapshot || self.flow_memo.native_state.environment_origins.contains_key(&key) { return None; }
        if let Some(generation) = self.flow_memo.native_state.environment_reads.get_mut(&key) {
            // Reentry invalidates only the observation, never normal checking.
            *generation = 0;
            return None;
        }
        if !self.environment_read_is_clean() { return None; }
        let generation = self.flow_memo.native_state.next_environment_read.checked_add(1)
            .expect("native environment observation generation overflow");
        self.flow_memo.native_state.next_environment_read = generation;
        self.flow_memo.native_state.environment_reads.insert(key, generation);
        Some(NativeEnvironmentRead {
            key, generation, counters: self.non_cacheable_mark(), frames: self.frames.len(),
            frame_serial: self.frames.last().map(|frame| frame.serial), taints: self.taints, cuts: self.cuts(),
        })
    }

    pub fn end_environment_read(&mut self, read: NativeEnvironmentRead, origin: Option<super::native_globals::NativeEnvironmentId>, complete: bool) {
        let generation = self.flow_memo.native_state.environment_reads.remove(&read.key);
        if complete && generation == Some(read.generation)
            && self.non_cacheable_mark() == read.counters && self.taints == read.taints && self.cuts() == read.cuts
            && self.frames.len() == read.frames && self.frames.last().map(|frame| frame.serial) == read.frame_serial
            && self.environment_read_is_clean() && !self.is_stack_low()
            && let Some(origin) = origin
            && self.native_globals_provider.is_some_and(|provider| provider.contains_global_environment(origin))
        {
            self.flow_memo.native_state.environment_origins.insert(read.key, origin);
        }
    }

    pub fn cached_environment_origin(&self, value: NativeValueKey, input: NativeFlowKey) -> Option<super::native_globals::NativeEnvironmentId> {
        self.flow_memo.native_state.environment_origins.get(&(value, input)).copied()
    }

    /// The value of an original recipe at its frozen input, through the same
    /// cache flow walks use (stored only when the computation was stable).
    pub fn native_flow_value_cached(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey) -> TypeId {
        self.native_flow_value(provider, value, input)
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
        // A value computed while an enclosing query is unfinished depends on it. As tsgo's
        // provisional caches, it is kept for that query's lifetime: values whose recipes read one
        // another (`local b = a * a; local c = b * b`) are each computed once per frame, rather
        // than once per path to them.
        let query = super::Query::NativeFlowValue(value, input);
        if let Some(raw) = self.provisional(query) {
            return TypeId(raw as u32);
        }
        if !self.enter(query) {
            // `enter` has tainted the dependent queries; the value depends on itself.
            return TypeId::ANY;
        }
        let context = self.native_query_context();
        let (ty, stored) =
            self.run_memoizable(|checker| provider.value_type(value, input, checker));
        match self.leave(query) {
            Ok(_) if stored.is_some() && !self.is_stack_low() => {
                self.flow_memo.native_state.value_types.insert(key, ty);
            }
            Err(_) if self.native_complete_without_cache(context, stored.is_some()) => {}
            Err(open) => self.cache_provisionally(query, u64::from(ty.0), open),
            _ => {}
        }
        ty
    }

    fn native_flow_predicate_with_types(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        reference: NativeReferenceKey,
        predicate: &NativePredicate,
    ) -> Result<Option<NativePredicate>, ()> {
        if !provider.predicate_needs_types(at, reference, predicate) { return Ok(None); }
        let epoch = self.flow_memo.native_state.effect_epoch;
        let key = (at, epoch);
        if let Some(cached) = self.flow_memo.native_state.typed_predicates.get(&key) { return Ok(Some(cached.clone())); }
        let query = super::Query::NativeFlowPredicate(at, epoch);
        if !self.enter(query) { return Err(()); }
        let begun = self.begin_memoizable();
        let typed = provider.predicate_with_types(at, predicate.clone(), self);
        let stored = self.end_memoizable(begun);
        if self.leave(query).is_ok() && stored.is_some() && !self.is_stack_low() {
            // Ordinary lexical predicates keep no additional cache entries.
            if let Some(ref typed) = typed {
                self.flow_memo.native_state.typed_predicates.insert(key, typed.clone());
            }
            Ok(typed)
        } else { Err(()) }
    }

    fn native_flow_call_type_predicate(
        &mut self,
        provider: &dyn NativeFlowProvider,
        value: NativeValueKey,
        input: NativeFlowKey,
    ) -> Option<super::native_signatures::NativeTypePredicate> {
        let epoch = self.flow_memo.native_state.effect_epoch;
        let key = ((value, input), epoch);
        if value.snapshot != input.graph.snapshot || !provider.contains_graph(input.graph) || provider.node(input).is_none() {
            provider.flow_error(input, NativeFlowError::InvalidValue(value));
            return None;
        }
        if let Some(&cached) = self.flow_memo.native_state.call_type_predicates.get(&key) { return cached; }
        let query = super::Query::NativeFlowCallTypePredicate(value, input, epoch);
        if !self.enter(query) { return None; }
        let begun = self.begin_memoizable();
        let predicate = provider.call_type_predicate(value, input, self);
        let stored = self.end_memoizable(begun);
        if self.leave(query).is_ok() && stored.is_some() && !self.is_stack_low() {
            self.flow_memo.native_state.call_type_predicates.insert(key, predicate);
            predicate
        } else { None }
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
        let context = self.native_query_context();
        let begun = self.begin_memoizable();
        let effects = provider.call_effects(value, input, self);
        let stored = self.end_memoizable(begun);
        match self.leave(query) {
            Ok(_) if stored.is_some() && !self.is_stack_low() => {
                self.flow_memo.native_state.call_effects.insert(key, effects);
            }
            Err(_) if self.native_complete_without_cache(context, stored.is_some()) => {
                return NativeEffectResult::Complete(effects);
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
