//! Object identity provenance from immutable original Lua flow recipes.
//! These are allocation/copy tokens, never fabricated type or syntax IDs.
use super::{Checker, Query};
use super::native_flow::*;
use super::task::Stored;
use crate::util::FxHashMap;
use crate::types::NativeTableKey;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug)]
pub enum NativeAliasValue {
    Reference { reference: NativeReferenceKey, input: NativeFlowKey },
    Allocation,
    Call { callee: NativeValueKey, input: NativeFlowKey },
    Projection { value: NativeValueKey, input: NativeFlowKey, index: usize },
    NotTable,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub struct NativeAliasReturnValue { pub value: NativeValueKey, pub input: NativeFlowKey }
#[derive(Clone, Debug)]
pub struct NativeAliasReturn {
    pub at: NativeFlowKey,
    pub values: Vec<NativeAliasReturnValue>,
    pub expands_last: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeAliasRelation { Same, Different, Possible }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Origin {
    Environment(super::native_globals::NativeEnvironmentId),
    Allocation(NativeValueKey, NativeFlowKey),
    OpaqueReference(NativeReferenceKey, NativeFlowKey),
    OpaqueValue(NativeValueKey, NativeFlowKey, usize),
    CallAllocation(NativeValueKey, NativeFlowKey, Box<Origin>),
    CallOpaque(NativeValueKey, NativeFlowKey, Box<Origin>),
}
#[derive(Clone, Default)]
struct Origins { tokens: HashSet<Origin>, unknown: bool, non_table: bool, uncertain: bool }

/// Completed provenance facts belong to the checker that owns their query
/// dependencies. Flow epochs separate speculative effects from stable inputs.
#[derive(Default)]
pub(super) struct NativeAliasCache {
    references: FxHashMap<(NativeReferenceKey, NativeFlowKey, u64), Origins>,
    values: FxHashMap<(NativeValueKey, NativeFlowKey, usize, u64), Origins>,
    // These are interned query paths, not identities of invented source nodes.
    paths: Vec<Vec<NativeTableKey>>,
    path_ids: FxHashMap<Vec<NativeTableKey>, u32>,
    projected_references: FxHashMap<(NativeReferenceKey, NativeFlowKey, u32, u64), Origins>,
    projected_values: FxHashMap<(NativeValueKey, NativeFlowKey, usize, u32, u64), Origins>,
}
impl NativeAliasCache {
    fn reference(&mut self, key: (NativeReferenceKey, NativeFlowKey, u64), result: Origins, _: Stored) { self.references.insert(key, result); }
    fn value(&mut self, key: (NativeValueKey, NativeFlowKey, usize, u64), result: Origins, _: Stored) { self.values.insert(key, result); }
    fn path(&mut self, path: &[NativeTableKey]) -> u32 {
        if let Some(&id) = self.path_ids.get(path) { return id }
        let id = self.paths.len().try_into().expect("native alias path identities fit u32");
        self.paths.push(path.to_vec());
        self.path_ids.insert(path.to_vec(), id);
        id
    }
    fn projected_reference(&mut self, key: (NativeReferenceKey, NativeFlowKey, u32, u64), result: Origins, _: Stored) { self.projected_references.insert(key, result); }
    fn projected_value(&mut self, key: (NativeValueKey, NativeFlowKey, usize, u32, u64), result: Origins, _: Stored) { self.projected_values.insert(key, result); }
}
impl Origins {
    fn unknown() -> Self { Self { unknown: true, uncertain: true, ..Self::default() } }
    fn non_table() -> Self { Self { non_table: true, ..Self::default() } }
    fn one(token: Origin, unknown: bool) -> Self { Self { tokens: HashSet::from([token]), unknown, non_table: false, uncertain: false } }
    fn merge(&mut self, other: Self) { self.tokens.extend(other.tokens); self.unknown |= other.unknown; self.non_table |= other.non_table; self.uncertain |= other.uncertain; }
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
    pub fn native_flow_alias_relation(&mut self, provider: &dyn NativeFlowProvider, left: NativeReferenceKey, left_at: NativeFlowKey, right: NativeValueKey, right_at: NativeFlowKey) -> NativeAliasRelation {
        let left_contract = provider.reference_written_contract(left, self);
        if left_contract.is_some_and(|contract| self.native_alias_written_non_table(contract)) { return NativeAliasRelation::Different }
        let right_contract = if let NativeAliasValue::Reference { reference, .. } = provider.value_alias_source(right, right_at) { provider.reference_written_contract(reference, self) } else { None };
        if right_contract.is_some_and(|contract| self.native_alias_written_non_table(contract)) { return NativeAliasRelation::Different }
        if let (Some(left), Some(right)) = (left_contract, right_contract)
            && self.native_alias_written_disjoint(left, right, &mut HashSet::new())
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
        let key = (reference, at, self.flow_memo.native_state.effect_epoch);
        if let Some(result) = self.native_alias_cache.references.get(&key) { return result.clone() }
        let query = Query::NativeAlias(reference, at);
        if !self.enter(query) { return Origins::unknown() }
        let begun = self.begin_memoizable();
        let result = match provider.node(at) {
            Some(NativeFlowNode::Start) => {
                let declaration = provider.reference_declaration(reference);
                if let Some(declaration) = declaration
                    && declaration.owner != reference.graph
                    && let Some(creation) = provider.graph_creation(reference.graph)
                    // A captured local from another function activation needs
                    // that activation's allocation provenance. Syntax/typed
                    // capture facets alone cannot identify its physical object.
                    && provider.graph_creation(declaration.owner).is_none()
                    && !declaration.writes.iter().any(|write| write.owner != declaration.owner || write.statement_end > creation.position)
                    && let Some(outer) = provider.reference_in_graph(reference, creation.input.graph)
                { self.native_alias_reference(provider, outer, creation.input) }
                else { Origins::one(Origin::OpaqueReference(reference, at), true) }
            }
            Some(NativeFlowNode::Write { before, target, value, input }) => match if provider.reference_is_cell(reference) {
                // Heap stores cannot rebind a lexical cell. Resolve only the
                // cell's original assignments; classifying heap aliases here
                // recursively asks for the identity we are already tracing.
                provider.write_affects(reference, target, at)
            } else { provider.write_affects_with_types(reference, target, at, self) } {
                NativeWriteRelation::Exact => self.native_alias_value(provider, value, input, 0),
                NativeWriteRelation::PossibleExact => {
                    let mut previous = self.native_alias_reference(provider, reference, before);
                    previous.merge(self.native_alias_value(provider, value, input, 0));
                    previous
                }
                NativeWriteRelation::Ancestor => {
                    match provider.reference_member_path(target, reference, self).filter(|path| !path.is_empty()) {
                        Some(path) => self.native_alias_projected_value(provider, value, input, 0, &path),
                        None => Origins::unknown(),
                    }
                },
                NativeWriteRelation::PossibleAncestor => Origins::unknown(),
                _ => self.native_alias_reference(provider, reference, before),
            },
            Some(NativeFlowNode::Call { before, .. }) => {
                let stable_cell = provider.reference_declaration(reference).is_some_and(|declaration| declaration.writes.iter().all(|write| write.owner == declaration.owner));
                if stable_cell && !provider.call_invalidates_reference(reference, at, self) {
                    self.native_alias_reference(provider, reference, before)
                } else { Origins::one(Origin::OpaqueReference(reference, at), true) }
            }
            Some(NativeFlowNode::Merge { edges }) => {
                let mut origins = Origins::default();
                for edge in edges { origins.merge(self.native_alias_reference(provider, reference, edge)); }
                origins
            }
            // A static allocation site on a backedge can denote another
            // physical object. Never equate iterations through a cycle token.
            Some(NativeFlowNode::Loop { .. }) => Origins::unknown(),
            Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }
                | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. }) => self.native_alias_reference(provider, reference, before),
            Some(NativeFlowNode::Unreachable | NativeFlowNode::Return { .. }) => Origins::non_table(),
            _ => Origins::unknown(),
        };
        let memoizable = self.end_memoizable(begun);
        if let Ok(stored) = self.leave(query)
            && memoizable.is_some() && !result.uncertain && !self.is_stack_low()
        { self.native_alias_cache.reference(key, result.clone(), stored); }
        result
    }

    fn native_alias_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey, column: usize) -> Origins {
        if value.snapshot != input.graph.snapshot || !provider.contains_graph(input.graph) || provider.node(input).is_none() { return Origins::unknown() }
        let key = (value, input, column, self.flow_memo.native_state.effect_epoch);
        if let Some(result) = self.native_alias_cache.values.get(&key) { return result.clone() }
        let query = Query::NativeAliasValue(value, input, column);
        if !self.enter(query) { return Origins::unknown() }
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
        let result = if let Some(environment) = environment {
            Origins::one(Origin::Environment(environment), false)
        } else { match provider.value_alias_source(value, input) {
            NativeAliasValue::Reference { reference, input } => if column == 0 { self.native_alias_reference(provider, reference, input) } else { Origins::non_table() },
            NativeAliasValue::Allocation => if column == 0 { Origins::one(Origin::Allocation(value, input), false) } else { Origins::non_table() },
            NativeAliasValue::Projection { value, input, index } => match index.checked_add(column) {
                Some(index) => self.native_alias_value(provider, value, input, index),
                None => Origins::unknown(),
            },
            NativeAliasValue::Call { callee, input: callee_input } => self.native_alias_call(provider, value, input, column, callee, callee_input),
            NativeAliasValue::NotTable => Origins::non_table(),
            NativeAliasValue::Unknown => Origins::one(Origin::OpaqueValue(value, input, column), true),
        } };
        let memoizable = self.end_memoizable(begun);
        if let Ok(stored) = self.leave(query)
            && memoizable.is_some() && !result.uncertain && !self.is_stack_low()
        { self.native_alias_cache.value(key, result.clone(), stored); }
        result
    }

    fn native_alias_projected_value(&mut self, provider: &dyn NativeFlowProvider, value: NativeValueKey, input: NativeFlowKey, column: usize, path: &[NativeTableKey]) -> Origins {
        if path.is_empty() { return self.native_alias_value(provider, value, input, column) }
        if value.snapshot != input.graph.snapshot || !provider.contains_graph(input.graph) || provider.node(input).is_none() { return Origins::unknown() }
        let Some(path) = path.iter().map(|key| key.normalize().ok()).collect::<Option<Vec<_>>>() else { return Origins::unknown() };
        let path_id = self.native_alias_cache.path(&path);
        let epoch = self.flow_memo.native_state.effect_epoch;
        let key = (value, input, column, path_id, epoch);
        if let Some(result) = self.native_alias_cache.projected_values.get(&key) { return result.clone() }
        let query = Query::NativeAliasProjectedValue(value, input, column, path_id, epoch);
        if !self.enter(query) { return Origins::unknown() }
        let begun = self.begin_memoizable();
        let result = if column == 0 && let Some(field) = provider.value_alias_member_source(value, input, path[0], self) {
            self.native_alias_projected_value(provider, field.value, field.input, 0, &path[1..])
        } else {
            match provider.value_alias_source(value, input) {
                NativeAliasValue::Reference { reference, input } if column == 0 => self.native_alias_projected_reference(provider, reference, input, &path),
                NativeAliasValue::Projection { value, input, index } => match index.checked_add(column) {
                    Some(column) => self.native_alias_projected_value(provider, value, input, column, &path),
                    None => Origins::unknown(),
                },
                NativeAliasValue::NotTable => Origins::non_table(),
                NativeAliasValue::Reference { .. } | NativeAliasValue::Allocation if column != 0 => Origins::non_table(),
                _ => Origins::unknown(),
            }
        };
        let memoizable = self.end_memoizable(begun);
        if let Ok(stored) = self.leave(query)
            && memoizable.is_some() && !result.uncertain && !self.is_stack_low()
        { self.native_alias_cache.projected_value(key, result.clone(), stored); }
        result
    }

    fn native_alias_projected_reference(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey, path: &[NativeTableKey]) -> Origins {
        if path.is_empty() { return self.native_alias_reference(provider, reference, at) }
        if reference.graph != at.graph || !provider.contains_graph(at.graph) || !provider.contains_reference(reference) { return Origins::unknown() }
        let path_id = self.native_alias_cache.path(path);
        let epoch = self.flow_memo.native_state.effect_epoch;
        let key = (reference, at, path_id, epoch);
        if let Some(result) = self.native_alias_cache.projected_references.get(&key) { return result.clone() }
        let query = Query::NativeAliasProjectedReference(reference, at, path_id, epoch);
        if !self.enter(query) { return Origins::unknown() }
        let begun = self.begin_memoizable();
        let result = match provider.node(at) {
            Some(NativeFlowNode::Start) => {
                if let Some(declaration) = provider.reference_declaration(reference)
                    && declaration.owner != reference.graph
                    && let Some(creation) = provider.graph_creation(reference.graph)
                    && provider.graph_creation(declaration.owner).is_none()
                    && !declaration.writes.iter().any(|write| write.owner != declaration.owner || write.statement_end > creation.position)
                    && let Some(outer) = provider.reference_in_graph(reference, creation.input.graph)
                { self.native_alias_projected_reference(provider, outer, creation.input, path) }
                else { Origins::unknown() }
            },
            Some(NativeFlowNode::Write { before, target, value, input }) => {
                self.native_alias_projected_write(provider, reference, at, path, before, target, value, input)
            },
            Some(NativeFlowNode::Merge { edges }) => {
                let mut origins = Origins::default();
                for edge in edges { origins.merge(self.native_alias_projected_reference(provider, reference, edge, path)); }
                origins
            },
            Some(NativeFlowNode::Pass { before } | NativeFlowNode::Predicate { before, .. }
                | NativeFlowNode::IteratorStart { before, .. } | NativeFlowNode::Iteration { before, .. }) => self.native_alias_projected_reference(provider, reference, before, path),
            // Heap contents may change across a call even when the base cell
            // remains stable. Neither a loop site nor a captured activation is
            // an identity proof for the projected object.
            Some(NativeFlowNode::Call { .. } | NativeFlowNode::Loop { .. }) => Origins::unknown(),
            Some(NativeFlowNode::Unreachable | NativeFlowNode::Return { .. }) => Origins::non_table(),
            _ => Origins::unknown(),
        };
        let memoizable = self.end_memoizable(begun);
        if let Ok(stored) = self.leave(query)
            && memoizable.is_some() && !result.uncertain && !self.is_stack_low()
        { self.native_alias_cache.projected_reference(key, result.clone(), stored); }
        result
    }

    fn native_alias_projected_write(&mut self, provider: &dyn NativeFlowProvider, reference: NativeReferenceKey, at: NativeFlowKey, path: &[NativeTableKey], before: NativeFlowKey, target: NativeReferenceKey, value: NativeValueKey, input: NativeFlowKey) -> Origins {
        let cell_write = provider.reference_is_cell(target);
        if cell_write || !provider.reference_is_cell(reference) {
            let relation = if cell_write { provider.write_affects(reference, target, at) }
                else { provider.write_affects_with_types(reference, target, at, self) };
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
        let callee_type = provider.alias_value_type(callee, callee_input, self);
        let Some(signature) = self.native_implementation_signature(callee_type) else { return opaque() };
        let Some((function, _)) = self.sig_native_decl(signature) else { return opaque() };
        let Some(rows) = provider.function_alias_returns(function) else { return opaque() };
        let Some(exit) = provider.function_alias_exit(function) else { return opaque() };
        let query = Query::NativeAliasReturn(function, column);
        if !self.enter(query) { return Origins::unknown() }
        let mut result = Origins::default();
        for row in rows {
            let Some(NativeFlowNode::Return { before }) = provider.node(row.at) else { result.unknown = true; result.uncertain = true; continue };
            if !self.native_flow_reachable(provider, before) { continue; }
            let selected = if column < row.values.len() { Some((row.values[column], 0)) }
                else if row.expands_last { row.values.last().copied().map(|last| (last, column - row.values.len() + 1)) }
                else { None };
            let Some((selected, offset)) = selected else { result.non_table = true; continue };
            let origins = self.native_alias_value(provider, selected.value, selected.input, offset);
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
                } else {
                    result.tokens.insert(Origin::CallOpaque(call, input, Box::new(origin)));
                }
            }
        }
        if self.native_flow_reachable(provider, exit) { result.non_table = true; }
        let _ = self.leave(query);
        result
    }
}
