//! Luanalysis v1's optional nil assignment compatibility. Runtime nil and its
//! union members remain physical facts even when contracts accept nil.
use super::Checker;
use super::relate::Relation;
use crate::types::TypeId;

impl<'p, 's> Checker<'p, 's> {
    /// Set before any queries; a checker's relation caches belong to one policy.
    pub fn set_native_strict_nil(&mut self, strict: bool) {
        self.native_strict_nil = strict;
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
