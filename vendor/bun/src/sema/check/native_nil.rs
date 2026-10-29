//! Luanalysis v1's optional nil assignment compatibility. Runtime nil and its
//! union members remain physical facts even when contracts accept nil.
use super::Checker;
use super::native_packs::{NativePackAlternative, NativePackSlot};
use super::relate::Relation;
use crate::types::{NativeResultPack, SigParam, TypeId};

impl<'p, 's> Checker<'p, 's> {
    /// Set before any queries; a checker's relation caches belong to one policy.
    pub fn set_native_strict_nil(&mut self, strict: bool) {
        self.native_strict_nil = strict;
    }

    /// Set before any queries. Exact: an optional Lua parameter may be absent, but an argument
    /// is a value, so nil satisfies it only when its own type admits nil. A frontend marks the
    /// absence with `missing` rather than `undefined`, which reads as nil everywhere else.
    /// Otherwise, as EmmyLua Analyzer reads it, absence and nil are one.
    pub fn set_native_exact_optionals(&mut self, exact: bool) {
        self.native_exact_optionals = exact;
    }

    pub fn native_exact_optionals(&self) -> bool {
        self.native_exact_optionals
    }

    /// The type an argument must have for a parameter of type `expected`: an optional one's
    /// absence, `missing`, is no value an argument can be.
    pub(super) fn native_argument_parameter_type(&mut self, expected: TypeId) -> TypeId {
        if !self.native_exact_optionals || !self.parts(expected).contains(&TypeId::MISSING) {
            return expected;
        }
        self.filter(expected, |_, member| member != TypeId::MISSING)
    }

    /// The values an expansion must supply for `params` from `position`, whose tuple is
    /// `expected`: each optional parameter's value is a value of its own type, or absent, so the
    /// expansion is one of the rows of present values, not a tuple whose optional element is nil.
    pub(super) fn native_argument_sequence_parameter_type(&mut self, params: &[SigParam], position: usize, expected: TypeId) -> TypeId {
        let remaining = params.get(position..).unwrap_or_default();
        if !self.native_exact_optionals
            || remaining.iter().any(|param| param.rest)
            || !remaining.iter().any(|param| self.parts(param.ty).contains(&TypeId::MISSING))
        {
            return expected;
        }
        let required = self.min_argument_count(params).saturating_sub(position).min(remaining.len());
        let mut slots = Vec::with_capacity(remaining.len());
        for param in remaining {
            let ty = self.native_argument_parameter_type(param.ty);
            slots.push(NativePackSlot { ty, optional: false });
        }
        let rows: Vec<_> = (required..=slots.len())
            .map(|count| NativePackAlternative { fixed: slots[..count].to_vec(), tail: None })
            .collect();
        self.native_result_pack(&rows).ty
    }

    /// An expansion against [`Self::native_argument_sequence_parameter_type`]'s rows: each row
    /// whose last values may be absent is also each shorter row without them. An optional slot's
    /// tuple element carries the nil its absence reads as; present, its value is the rest.
    pub(super) fn native_argument_sequence_rows(&mut self, actual: TypeId) -> TypeId {
        let Some(rows) = self.native_pack_alternatives(NativeResultPack { ty: actual }) else { return actual };
        if !rows.iter().any(|row| row.tail.is_none() && row.fixed.last().is_some_and(|slot| slot.optional)) {
            return actual;
        }
        let mut expanded = Vec::new();
        for row in rows {
            if row.tail.is_some() {
                expanded.push(row);
                continue;
            }
            let present = row.fixed.iter().rposition(|slot| !slot.optional).map_or(0, |index| index + 1);
            let fixed: Vec<_> = row.fixed.iter().map(|slot| NativePackSlot {
                ty: if slot.optional { self.filter(slot.ty, |_, member| !member.is_undefined()) } else { slot.ty },
                optional: false,
            }).collect();
            expanded.extend((present..=fixed.len()).map(|count| NativePackAlternative { fixed: fixed[..count].to_vec(), tail: None }));
        }
        self.native_result_pack(&expanded).ty
    }

    pub fn native_contains_nil(&mut self, ty: TypeId) -> bool {
        self.native_is_assignable_with_strict_nil(TypeId::UNDEFINED, ty)
    }

    pub(super) fn native_nil_assignment_compatible(&self, source: TypeId, target: TypeId, relation: Relation) -> bool {
        !self.native_strict_nil && relation == Relation::Assignable
            && source.is_undefined() && !target.is_never()
            && !self.is_union(target) && !self.is_intersection(target)
    }

    /// Physical exclusion and narrowing must not treat nil as every type merely
    /// because an assignment contract permits it. A separate relation key keeps
    /// strict physical comparisons apart from permissive contract comparisons.
    pub(super) fn native_is_assignable_with_strict_nil(&mut self, source: TypeId, target: TypeId) -> bool {
        let relation = if self.native_strict_nil { Relation::Assignable } else { Relation::NativeStrictAssignable };
        self.related(source, target, relation)
    }
}
