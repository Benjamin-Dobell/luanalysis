//! Lua runtime top types. Structural carriers supply existing table/call
//! operations without erasing the distinction between functions and tables.
use super::Checker;
use super::native_flow::NativeOpaqueRuntimeKind;
use super::native_named::{NativeNamedTypeError, NativeTypeDeclarationKind, NativeTypeDeclarationQuery};
use super::Query;
use crate::types::NativeTypeDeclarationId;
use crate::types::{NativeRuntimeKind, NativeResultPack, NativeTableIndexer, NativeTableFlavor, SigParam, TypeData, TypeId};

impl<'p, 's> Checker<'p, 's> {
    fn native_opaque_runtime_root(&self, kind: NativeOpaqueRuntimeKind) -> Option<NativeTypeDeclarationId> {
        let declaration = self.native_type_declaration_provider?.runtime_type_declaration(kind)?;
        let provider = self.native_named_provider(declaration).ok()?;
        let header = provider.type_declaration_header(self, declaration);
        (header.kind == NativeTypeDeclarationKind::Class
            && header.parameters.is_empty() && header.outer_parameters.is_empty()).then_some(declaration)
    }

    /// Declaration identity and ancestry only. Runtime classification must not
    /// expand a class's members, method contracts or inferred implementations.
    pub(super) fn native_opaque_runtime_declaration(&mut self, declaration: NativeTypeDeclarationId) -> Result<Option<NativeOpaqueRuntimeKind>, NativeNamedTypeError> {
        let roots = [NativeOpaqueRuntimeKind::Userdata, NativeOpaqueRuntimeKind::Thread]
            .map(|kind| (kind, self.native_opaque_runtime_root(kind)));
        if roots.iter().all(|(_, root)| root.is_none()) { return Ok(None) }
        if let Some((kind, _)) = roots.iter().find(|(_, root)| *root == Some(declaration)) { return Ok(Some(*kind)) }
        let query_kind = NativeTypeDeclarationQuery::RuntimeDomain(declaration);
        let query = Query::NativeNamed(query_kind);
        if !self.enter(query) {
            return Err(if self.found_cycle { NativeNamedTypeError::CircularBase(declaration) } else { NativeNamedTypeError::TooDeep(query_kind) });
        }
        let result = (|| {
            let mut selected = None;
            for (index, base) in self.native_named_bases(declaration)?.into_iter().enumerate() {
                let TypeData::NativeObject(reference) = *self.data(base) else { continue };
                let Some(kind) = self.native_opaque_runtime_declaration(reference.declaration)? else { continue };
                if let Some(previous) = selected && previous != kind {
                    let expected = self.native_type_reference(self.native_opaque_runtime_root(previous).unwrap(), &[])?;
                    let actual = self.native_type_reference(self.native_opaque_runtime_root(kind).unwrap(), &[])?;
                    return Err(NativeNamedTypeError::IncompatibleBase { declaration, index, base, expected, actual });
                }
                selected = Some(kind);
            }
            Ok(selected)
        })();
        let _ = self.leave(query);
        if let Err(error) = result { self.report_native_named(error); }
        result
    }

    fn native_opaque_runtime_kind(&mut self, ty: TypeId) -> Result<Option<NativeOpaqueRuntimeKind>, NativeNamedTypeError> {
        if self.is_intersection(ty) {
            for part in self.parts(ty).to_vec() {
                if let Some(kind) = self.native_opaque_runtime_kind(part)? { return Ok(Some(kind)) }
            }
            return Ok(None);
        }
        // A class namespace remains a physical table even when its instance
        // schema has opaque ancestry. Materialization preserves that facet.
        if matches!(self.data(ty), TypeData::NativeDefinition(_) | TypeData::NativeRuntimeTable(_)) { return Ok(None) }
        if matches!(self.data(ty), TypeData::NativeTable(table) if table.definition_origin.is_some()) { return Ok(None) }
        let Some(reference) = self.native_nominal_origin(ty) else { return Ok(None) };
        self.native_opaque_runtime_declaration(reference.declaration)
    }

    pub(super) fn native_narrow_opaque_runtime_kind(&mut self, ty: TypeId, kind: NativeOpaqueRuntimeKind, sense: bool) -> TypeId {
        let Some(root) = self.native_opaque_runtime_root(kind) else { return ty };
        let Ok(root) = self.native_type_reference(root, &[]) else { return ty };
        if self.is_any(ty) { return ty }
        if ty == TypeId::UNKNOWN { return if sense { root } else { ty } }
        self.map_type(ty, |checker, part| {
            let apparent = checker.base_constraint_of(part).unwrap_or(part);
            match checker.native_opaque_runtime_kind(apparent) {
                Ok(Some(actual)) => return if (actual == kind) == sense { part } else { TypeId::NEVER },
                Err(_) => return part,
                Ok(None) => {},
            }
            if checker.flags(part) & crate::types::tf::TYPE_PARAMETER != 0 {
                return if sense { checker.intersection(&[part, root]) } else { part };
            }
            if apparent == TypeId::UNKNOWN || !sense { part } else { TypeId::NEVER }
        })
    }

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
        match self.native_opaque_runtime_kind(ty) {
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) => {},
        }
        match self.data(ty) {
            TypeData::NativeRuntimeTop { kind, .. } => Some(*kind),
            // A named table can also have call signatures. Its origin takes
            // precedence over structural callability in the Lua runtime.
            TypeData::NativeRuntimeTable(_) | TypeData::NativeDefinition(_) | TypeData::NativeObject(_) | TypeData::NativeTable(_) => Some(NativeRuntimeKind::Table),
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
        // Definition namespaces and instances can share a nominal owner, but
        // an actual Lua table cannot acquire its owner's userdata/thread kind.
        // Leave composite and instantiable types to the existing relation walk.
        let (Ok(source_domain), Ok(target_domain)) = (self.native_opaque_runtime_kind(source), self.native_opaque_runtime_kind(target)) else { return false };
        if let Some(expected) = target_domain {
            if !self.is_any(source) && source != TypeId::UNKNOWN && !source.is_never()
                && !self.is_union(source) && !self.is_intersection(source) && !self.is_instantiable(source)
                && source_domain != Some(expected)
            { return false }
        }
        if source_domain.is_some()
            && matches!(self.data(target), TypeData::NativeDefinition(_) | TypeData::NativeRuntimeTable(_))
        { return false }
        if source_domain.is_some()
            && matches!(self.data(target), TypeData::NativeTable(table) if table.definition_origin.is_some())
        { return false }
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
            match checker.native_opaque_runtime_kind(apparent) {
                Ok(Some(_)) => return if sense { TypeId::NEVER } else { part },
                Err(_) => return part,
                Ok(None) => {},
            }
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
