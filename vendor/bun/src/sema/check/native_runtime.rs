//! Lua runtime top types. Structural carriers supply existing table/call
//! operations without erasing the distinction between functions and tables.
use super::Checker;
use crate::types::{NativeRuntimeKind, NativeResultPack, NativeTableIndexer, NativeTableFlavor, SigParam, TypeData, TypeId};

impl<'p, 's> Checker<'p, 's> {
    pub fn native_runtime_top(&mut self, kind: NativeRuntimeKind) -> TypeId {
        let index = match kind { NativeRuntimeKind::Table => 0, NativeRuntimeKind::Function => 1 };
        if let Some(known) = self.native_runtime_tops[index] { return known }
        let carrier = match kind {
            NativeRuntimeKind::Table => self.native_table(&[], &[NativeTableIndexer {
                key: TypeId::ANY, value: TypeId::ANY, readonly: false, sparse: false,
                origin: crate::types::NativeTableFieldOrigin::Declared,
            }], NativeTableFlavor::Declared).expect("any is a valid table key domain"),
            NativeRuntimeKind::Function => {
                let pack = self.native_result_pack(&[super::native_packs::NativePackAlternative { fixed: Vec::new(), tail: Some(TypeId::ANY) }]);
                let param = SigParam {
                    name: self.atoms().intern(b"..."), ty: self.native_pack_type(pack),
                    optional: false, rest: true, is_required_rest: false, declaration: None,
                };
                self.native_pack_function(&[param], &[], NativeResultPack { ty: TypeId::ANY })
            }
        };
        let ty = self.intern(TypeData::NativeRuntimeTop { kind, carrier });
        self.native_runtime_tops[index] = Some(ty);
        ty
    }

    pub(super) fn native_runtime_kind(&mut self, ty: TypeId) -> Option<NativeRuntimeKind> {
        if !self.is_object_type(ty) { return None }
        match self.data(ty) {
            TypeData::NativeRuntimeTop { kind, .. } => Some(*kind),
            // A named table can also have call signatures. Its origin takes
            // precedence over structural callability in the Lua runtime.
            TypeData::NativeDefinition(_) | TypeData::NativeObject(_) | TypeData::NativeTable(_) => Some(NativeRuntimeKind::Table),
            _ => {
                let members = self.structured_members(ty)?;
                Some(if members.call_signatures().is_empty() && members.construct_signatures().is_empty() {
                    NativeRuntimeKind::Table
                } else { NativeRuntimeKind::Function })
            }
        }
    }

    /// This gate runs inside recursive relations, including member/union and
    /// generic-constraint checks. Composite types retain the shared traversal.
    pub(super) fn native_runtime_types_compatible(&mut self, source: TypeId, target: TypeId) -> bool {
        let (source_top, target_top) = match (self.data(source), self.data(target)) {
            (TypeData::NativeRuntimeTop { kind: s, .. }, TypeData::NativeRuntimeTop { kind: t, .. }) => return s == t,
            (TypeData::NativeRuntimeTop { kind, .. }, _) => (Some(*kind), None),
            (_, TypeData::NativeRuntimeTop { kind, .. }) => (None, Some(*kind)),
            _ => return true,
        };
        let (other, expected) = if let Some(kind) = target_top { (source, kind) } else { (target, source_top.unwrap()) };
        if self.is_any(other) || other == TypeId::UNKNOWN || other.is_never() { return true }
        if self.is_union(other) || self.is_intersection(other) || self.is_instantiable(other) { return true }
        self.native_runtime_kind(other).is_some_and(|kind| kind == expected)
    }

    pub(super) fn native_runtime_carrier(&self, ty: TypeId) -> TypeId {
        if let TypeData::NativeRuntimeTop { carrier, .. } = self.data(ty) { *carrier } else { ty }
    }

    /// A runtime top accepts every value in its category, including named
    /// tables with call signatures. Structural carriers impose no additional
    /// TypeScript object requirements on these category comparisons.
    pub(super) fn native_runtime_top_target_relation(&mut self, source: TypeId, target: TypeId, relation: super::relate::Relation) -> Option<bool> {
        let TypeData::NativeRuntimeTop { kind, .. } = *self.data(target) else { return None };
        if self.is_union(source) || self.is_intersection(source) || self.is_instantiable(source) { return None }
        let actual = self.native_runtime_kind(source)?;
        Some(actual == kind && relation != super::relate::Relation::Identity)
    }

    pub(super) fn native_narrow_runtime_kind(&mut self, ty: TypeId, kind: NativeRuntimeKind, sense: bool) -> TypeId {
        if self.is_any(ty) { return ty }
        if ty == TypeId::UNKNOWN { return if sense { self.native_runtime_top(kind) } else { ty } }
        self.map_type(ty, |checker, part| {
            let apparent = checker.base_constraint_of(part).unwrap_or(part);
            if let Some(actual) = checker.native_runtime_kind(apparent) {
                return if (actual == kind) == sense { part } else { TypeId::NEVER }
            }
            if checker.flags(part) & crate::types::tf::TYPE_PARAMETER != 0 {
                if sense {
                    let narrowed = checker.native_runtime_top(kind);
                    return checker.intersection(&[part, narrowed]);
                }
                return part;
            }
            if apparent == TypeId::UNKNOWN || !sense { part } else { TypeId::NEVER }
        })
    }
}
