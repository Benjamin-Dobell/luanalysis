//! Immutable Lua table shapes and key domains, independent of JS property-name coercion.

use super::*;
use super::relate::{Relater, Relation, Ternary, REC_BOTH};
use super::native_packs::{NativePackAlternative, NativePackSlot};
use crate::types::List;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTableKey {
    String(Atom),
    Integer(i64),
    Float(u64),
    Boolean(bool),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTableAccessKey {
    Literal(NativeTableKey),
    Type(TypeId),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTableFieldOrigin {
    Inferred,
    Declared,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeTableField {
    pub key: NativeTableKey,
    pub value: TypeId,
    pub optional: bool,
    pub readonly: bool,
    pub origin: NativeTableFieldOrigin,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeTableIndexer {
    pub key: TypeId,
    pub value: TypeId,
    pub readonly: bool,
    pub sparse: bool,
    pub origin: NativeTableFieldOrigin,
}

/// Open sequence entries have integer keys at or above `start`; they do not imply a broad Lua
/// numeric indexer and cannot overwrite the types of an earlier prefix or fractional key.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeTableSequenceTail {
    pub start: i64,
    pub value: TypeId,
    pub readonly: bool,
    pub origin: NativeTableFieldOrigin,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct NativeTableData<'s> {
    /// A selected original Lua environment, with member contracts demanded lazily.
    pub environment_origin: Option<super::native_globals::NativeEnvironmentId>,
    pub definition_origin: Option<NativeDefinitionRef>,
    /// The original class declaration carried by a resolved native object. Ordinary table
    /// construction never supplies an origin; it cannot infer one from equal fields.
    pub nominal_origin: Option<NativeObjectReference>,
    pub fields: List<'s, NativeTableField>,
    pub indexers: List<'s, NativeTableIndexer>,
    pub sequence_tails: List<'s, NativeTableSequenceTail>,
    pub fresh: bool,
    /// Inferred constructor stores are collision candidates even after freshness is widened.
    pub collision_candidates: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeTableFlavor {
    Declared,
    Literal,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum NativeTableError {
    InvalidKey { key: NativeTableAccessKey },
    NotTable { ty: TypeId },
    MissingKey { key: NativeTableAccessKey },
    KeyType { key: TypeId, expected: TypeId },
    ValueType { value: TypeId, expected: TypeId },
    ReadOnly { key: NativeTableAccessKey },
    RequiredDeletion { key: NativeTableKey },
    DuplicateDeclaredField { key: NativeTableKey },
    InvalidSequenceStart { start: i64 },
    SequenceTooComplex { count: usize, alternatives: usize },
    UnresolvedSequence,
}

impl NativeTableKey {
    pub fn normalize(self) -> Result<Self, NativeTableError> {
        let NativeTableKey::Float(bits) = self else { return Ok(self) };
        let value = f64::from_bits(bits);
        if value.is_nan() {
            return Err(NativeTableError::InvalidKey { key: NativeTableAccessKey::Literal(self) });
        }
        // 2^63 is excluded: its f64 value cannot represent i64::MAX.
        if value >= i64::MIN as f64 && value < -(i64::MIN as f64) && value.fract() == 0.0 {
            Ok(NativeTableKey::Integer(value as i64))
        } else {
            Ok(self)
        }
    }
}

impl<'p, 's> Checker<'p, 's> {
    pub(super) fn native_sequence_contains(start: i64, key: NativeTableKey) -> bool {
        matches!(key, NativeTableKey::Integer(index) if index >= start)
    }

    fn native_sequence_access_matches(&mut self, start: i64, key: NativeTableAccessKey) -> bool {
        match key {
            NativeTableAccessKey::Literal(key) => Self::native_sequence_contains(start, key),
            NativeTableAccessKey::Type(ty) => self.is_any(ty) || self.is_assignable(ty, TypeId::NUMBER),
        }
    }

    pub(super) fn native_sequence_type_overlaps(&mut self, start: i64, ty: TypeId) -> bool {
        let key = self.native_table_access(NativeTableAccessKey::Type(ty));
        key.is_ok_and(|key| self.native_sequence_access_matches(start, key))
    }

    pub fn native_table_with_sequence(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        flavor: NativeTableFlavor,
        start: i64,
        pack: NativeResultPack,
        expected: Option<TypeId>,
    ) -> Result<TypeId, NativeTableError> {
        self.native_table_with_sequence_and_tails(fields, indexers, &[], flavor, start, pack, expected)
    }

    /// A final Lua sequence expression expands complete physical result rows. A missing result
    /// performs no store; an explicit nil candidate deletes an entry through the constructor.
    pub fn native_table_with_sequence_and_tails(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        sequence_tails: &[NativeTableSequenceTail],
        flavor: NativeTableFlavor,
        start: i64,
        pack: NativeResultPack,
        expected: Option<TypeId>,
    ) -> Result<TypeId, NativeTableError> {
        self.native_table_with_sequence_and_policy(fields, indexers, sequence_tails, flavor, flavor == NativeTableFlavor::Literal, start, pack, expected)
    }

    pub fn native_table_with_sequence_and_policy(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        sequence_tails: &[NativeTableSequenceTail],
        flavor: NativeTableFlavor,
        collision_candidates: bool,
        start: i64,
        pack: NativeResultPack,
        expected: Option<TypeId>,
    ) -> Result<TypeId, NativeTableError> {
        self.native_table_with_sequence_and_policy_origin(fields, indexers, sequence_tails, flavor, collision_candidates, start, pack, NativeTableFieldOrigin::Inferred, expected)
    }

    pub fn native_table_with_sequence_and_policy_origin(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        sequence_tails: &[NativeTableSequenceTail],
        flavor: NativeTableFlavor,
        collision_candidates: bool,
        start: i64,
        pack: NativeResultPack,
        origin: NativeTableFieldOrigin,
        expected: Option<TypeId>,
    ) -> Result<TypeId, NativeTableError> {
        if start < 1 { return Err(NativeTableError::InvalidSequenceStart { start }) }
        let alternatives = match self.native_pack_alternatives(pack) {
            Some(rows) => rows,
            None if self.is_any(pack.ty) => vec![NativePackAlternative { fixed: Vec::new(), tail: Some(pack.ty) }],
            None => return Err(NativeTableError::UnresolvedSequence),
        };
        let width = alternatives.iter().map(|row| row.fixed.len()).max().unwrap_or(0);
        let open_start = alternatives.iter().filter(|row| row.tail.is_some())
            .map(|row| start.checked_add(row.fixed.len() as i64)).collect::<Option<Vec<_>>>()
            .ok_or(NativeTableError::InvalidSequenceStart { start })?
            .into_iter().min();
        let rows = alternatives.iter().try_fold(0usize, |rows, row| {
            let minimum = row.fixed.iter().position(|slot| slot.optional).unwrap_or(row.fixed.len());
            rows.checked_add(row.fixed.len() - minimum + 1)
        });
        if width >= 10_000 || rows.and_then(|rows| rows.checked_mul(width.max(1))).is_none_or(|slots| slots >= 100_000) {
            return Err(NativeTableError::SequenceTooComplex { count: width, alternatives: rows.unwrap_or(usize::MAX) });
        }
        let mut tables = Vec::new();
        for row in &alternatives {
            let minimum = row.fixed.iter().position(|slot| slot.optional).unwrap_or(row.fixed.len());
            for present in minimum..=row.fixed.len() {
                let mut actual_fields = fields.to_vec();
                for (offset, slot) in row.fixed[..present].iter().enumerate() {
                    let index = start.checked_add(offset as i64).ok_or(NativeTableError::InvalidSequenceStart { start })?;
                    actual_fields.push(NativeTableField {
                        key: NativeTableKey::Integer(index), value: slot.ty, optional: false, readonly: false,
                        origin,
                    });
                }
                let mut tails = sequence_tails.to_vec();
                if present == row.fixed.len() && let Some(value) = row.tail {
                    let start = start.checked_add(present as i64).ok_or(NativeTableError::InvalidSequenceStart { start })?;
                    tails.push(NativeTableSequenceTail { start, value, readonly: false, origin });
                }
                let table = self.native_table_with_context_and_sequence_policy(&actual_fields, indexers, &tails, flavor, collision_candidates, expected)?;
                tables.push(self.native_table_sequence_presence(table, start, width, open_start, origin)?);
            }
        }
        Ok(self.union(&tables))
    }

    /// Absence is a fact in each whole row, rather than an invented nil store. These keys are
    /// possible sequence positions in another row; unrelated absent closed-shape keys still fail.
    fn native_table_sequence_presence(&mut self, table: TypeId, start: i64, width: usize, open_start: Option<i64>, origin: NativeTableFieldOrigin) -> Result<TypeId, NativeTableError> {
        let TypeData::NativeTable(data) = self.data(table) else { return Err(NativeTableError::NotTable { ty: table }) };
        let mut fields = data.fields.to_vec();
        let mut tails = data.sequence_tails.to_vec();
        let mut missing = |c: &mut Self, index: i64| {
            let key = NativeTableKey::Integer(index);
            if fields.iter().any(|field| field.key == key)
                || tails.iter().any(|tail| Self::native_sequence_contains(tail.start, key))
                || data.indexers.iter().any(|indexer| c.is_assignable(c.native_table_key_type(key), indexer.key)) { return }
            fields.push(NativeTableField { key, value: TypeId::NEVER, optional: true, readonly: false, origin });
        };
        for offset in 0..width {
            let index = start.checked_add(offset as i64).ok_or(NativeTableError::InvalidSequenceStart { start })?;
            missing(self, index);
        }
        if let Some(open) = open_start {
            match tails.iter().map(|tail| tail.start).min() {
                None => tails.push(NativeTableSequenceTail { start: open, value: TypeId::NEVER, readonly: false, origin }),
                Some(current) if current > open => {
                    let gap = usize::try_from(current - open).map_err(|_| NativeTableError::SequenceTooComplex { count: usize::MAX, alternatives: 1 })?;
                    if gap >= 10_000 { return Err(NativeTableError::SequenceTooComplex { count: gap, alternatives: 1 }) }
                    for index in open..current { missing(self, index); }
                }
                _ => {}
            }
        }
        self.native_table_from_description_with_policy(&fields, &data.indexers, &tails, data.fresh, data.collision_candidates)
    }

    /// Context remains a union of whole numeric rows, including gaps and optional presence.
    /// Selecting a field context and independently unioning positions would lose correlations.
    pub fn native_table_sequence_context(&mut self, expected: TypeId, start: i64) -> Option<NativeResultPack> {
        if start < 1 { return None }
        if self.is_any(expected) { return Some(NativeResultPack { ty: expected }) }
        let expected = self.base_constraint_or_type(expected);
        let parts = self.parts(expected).to_vec();
        let mut alternatives = Vec::new();
        for &part in &parts {
            let TypeData::NativeTable(table) = self.data(part) else { continue };
            let end = table.fields.iter().filter_map(|field| match field.key {
                NativeTableKey::Integer(index) if index >= start => index.checked_add(1), _ => None,
            }).chain(table.sequence_tails.iter().map(|tail| tail.start.max(start))).max().unwrap_or(start);
            let count = usize::try_from(end.checked_sub(start)?).ok()?;
            if count >= 10_000 || count.checked_mul(parts.len().max(1)).is_none_or(|slots| slots >= 100_000) { return None }
            let mut fixed = Vec::with_capacity(count);
            for index in start..end {
                let literal = NativeTableKey::Integer(index);
                let field = table.fields.iter().find(|field| field.key == literal);
                let mut indexed = Vec::new();
                for tail in &table.sequence_tails { if Self::native_sequence_contains(tail.start, literal) { indexed.push(tail.value); } }
                let key = self.native_table_key_type(literal);
                for indexer in &table.indexers { if self.is_assignable(key, indexer.key) { indexed.push(indexer.value); } }
                let ty = match field {
                    Some(field) => field.value,
                    None if indexed.is_empty() => TypeId::UNDEFINED,
                    None if table.collision_candidates => self.union(&indexed),
                    None => self.intersection(&indexed),
                };
                fixed.push(NativePackSlot { ty, optional: field.is_none_or(|field| field.optional || self.contains_undefined(field.value)) });
            }
            let mut values: Vec<_> = table.sequence_tails.iter().map(|tail| tail.value).collect();
            for indexer in &table.indexers { if self.is_assignable(TypeId::NUMBER, indexer.key) { values.push(indexer.value); } }
            let tail = if values.is_empty() { None } else if table.collision_candidates { Some(self.union(&values)) } else { Some(self.intersection(&values)) };
            alternatives.push(NativePackAlternative { fixed, tail });
        }
        (!alternatives.is_empty()).then(|| self.native_result_pack(&alternatives))
    }

    /// Import a checked immutable description without re-running constructor inference. In
    /// particular, contextual singleton literals and generic substitutions must survive export.
    pub fn native_table_from_description(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        fresh: bool,
    ) -> Result<TypeId, NativeTableError> {
        self.native_table_from_description_with_sequence_tails(fields, indexers, &[], fresh)
    }

    pub fn native_table_from_description_with_sequence_tails(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        sequence_tails: &[NativeTableSequenceTail],
        fresh: bool,
    ) -> Result<TypeId, NativeTableError> {
        self.native_table_from_description_with_policy(fields, indexers, sequence_tails, fresh, fresh)
    }

    pub fn native_table_from_description_with_policy(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        sequence_tails: &[NativeTableSequenceTail],
        fresh: bool,
        collision_candidates: bool,
    ) -> Result<TypeId, NativeTableError> {
        let mut fields = fields.to_vec();
        for field in &mut fields {
            field.key = field.key.normalize()?;
            if field.origin == NativeTableFieldOrigin::Declared { field.value = self.regular(field.value); }
        }
        fields.sort_by(|a, b| self.native_table_compare_keys(a.key, b.key));
        for pair in fields.windows(2) {
            if pair[0].key == pair[1].key {
                return Err(NativeTableError::DuplicateDeclaredField { key: pair[0].key });
            }
        }
        for indexer in indexers {
            if !self.native_table_valid_key_type(indexer.key) {
                return Err(NativeTableError::InvalidKey { key: NativeTableAccessKey::Type(indexer.key) });
            }
        }
        let mut indexers = indexers.to_vec();
        for indexer in &mut indexers {
            if indexer.origin == NativeTableFieldOrigin::Declared { indexer.value = self.regular(indexer.value); }
        }
        indexers.sort_by_key(|indexer| indexer.key.0);
        let mut sequence_tails = sequence_tails.to_vec();
        for tail in &mut sequence_tails {
            if tail.start < 1 { return Err(NativeTableError::InvalidSequenceStart { start: tail.start }) }
            if tail.origin == NativeTableFieldOrigin::Declared { tail.value = self.regular(tail.value); }
        }
        sequence_tails.sort_by_key(|tail| (tail.start, tail.value.0, tail.readonly));
        Ok(self.intern(TypeData::NativeTable(self.boxed(NativeTableData {
            environment_origin: None,
            definition_origin: None,
            nominal_origin: None,
            fields: self.list(&fields), indexers: self.list(&indexers),
            sequence_tails: self.list(&sequence_tails), fresh, collision_candidates,
        }))))
    }

    pub fn native_table(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        flavor: NativeTableFlavor,
    ) -> Result<TypeId, NativeTableError> {
        self.native_table_with_context(fields, indexers, flavor, None)
    }

    pub fn native_table_with_context(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        flavor: NativeTableFlavor,
        expected: Option<TypeId>,
    ) -> Result<TypeId, NativeTableError> {
        self.native_table_with_context_and_sequence_tails(fields, indexers, &[], flavor, expected)
    }

    pub fn native_table_with_context_and_sequence_tails(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        sequence_tails: &[NativeTableSequenceTail],
        flavor: NativeTableFlavor,
        expected: Option<TypeId>,
    ) -> Result<TypeId, NativeTableError> {
        self.native_table_with_context_and_sequence_policy(fields, indexers, sequence_tails, flavor, flavor == NativeTableFlavor::Literal, expected)
    }

    pub fn native_table_with_context_and_sequence_policy(
        &mut self,
        fields: &[NativeTableField],
        indexers: &[NativeTableIndexer],
        sequence_tails: &[NativeTableSequenceTail],
        flavor: NativeTableFlavor,
        collision_candidates: bool,
        expected: Option<TypeId>,
    ) -> Result<TypeId, NativeTableError> {
        let fresh = flavor == NativeTableFlavor::Literal;
        let mut merged: Vec<NativeTableField> = Vec::with_capacity(fields.len());
        for &field in fields {
            let key = field.key.normalize()?;
            let mut field = NativeTableField { key, ..field };
            if field.origin == NativeTableFieldOrigin::Declared { field.value = self.regular(field.value); }
            if collision_candidates {
                field.optional |= self.contains_undefined(field.value);
                field.value = self.without_undefined(field.value);
            }
            if fresh && field.origin == NativeTableFieldOrigin::Inferred {
                let contextual = expected.and_then(|expected| self.native_table_value_context(expected, NativeTableAccessKey::Literal(key)));
                field.value = self.widen_literal_for_context(field.value, contextual);
                field.value = self.get_widened_type(field.value);
            }
            if let Some(previous) = merged.iter_mut().find(|previous| previous.key == key) {
                if !collision_candidates {
                    return Err(NativeTableError::DuplicateDeclaredField { key });
                }
                previous.value = self.union_reduced(&[previous.value, field.value]);
                previous.optional |= field.optional;
                previous.readonly &= field.readonly;
                if field.origin == NativeTableFieldOrigin::Declared { previous.origin = field.origin; }
            } else {
                merged.push(field);
            }
        }
        // Nil-valued constructor fields are absent, including every candidate of a collision.
        if collision_candidates {
            merged.retain(|field| !field.value.is_never());
        }
        let mut domains: Vec<NativeTableIndexer> = Vec::with_capacity(indexers.len());
        for &indexer in indexers {
            if !self.native_table_valid_key_type(indexer.key) {
                return Err(NativeTableError::InvalidKey { key: NativeTableAccessKey::Type(indexer.key) });
            }
            let mut indexer = indexer;
            if indexer.origin == NativeTableFieldOrigin::Declared { indexer.value = self.regular(indexer.value); }
            if fresh {
                indexer.sparse = true;
                if indexer.origin == NativeTableFieldOrigin::Inferred {
                    let contextual = expected.and_then(|expected| self.native_table_value_context(expected, NativeTableAccessKey::Type(indexer.key)));
                    indexer.value = self.widen_literal_for_context(indexer.value, contextual);
                    indexer.value = self.get_widened_type(indexer.value);
                }
            }
            if let Some(previous) = domains.iter_mut().find(|previous| previous.key == indexer.key) {
                previous.value = if collision_candidates {
                    self.union_reduced(&[previous.value, indexer.value])
                } else {
                    self.intersection(&[previous.value, indexer.value])
                };
                previous.sparse |= indexer.sparse;
                previous.readonly = if collision_candidates { previous.readonly && indexer.readonly } else { previous.readonly || indexer.readonly };
                if indexer.origin == NativeTableFieldOrigin::Declared { previous.origin = indexer.origin; }
            } else {
                domains.push(indexer);
            }
        }
        let mut tails: Vec<NativeTableSequenceTail> = Vec::with_capacity(sequence_tails.len());
        for &tail in sequence_tails {
            if tail.start < 1 { return Err(NativeTableError::InvalidSequenceStart { start: tail.start }) }
            let mut tail = tail;
            if tail.origin == NativeTableFieldOrigin::Declared { tail.value = self.regular(tail.value); }
            if fresh && tail.origin == NativeTableFieldOrigin::Inferred {
                let contextual = expected.and_then(|expected| self.native_table_sequence_context(expected, tail.start))
                    .and_then(|pack| self.native_pack_alternatives(pack))
                    .map(|rows| self.union(&rows.iter().flat_map(|row| row.fixed.iter().map(|slot| slot.ty).chain(row.tail)).collect::<Vec<_>>()));
                tail.value = self.widen_literal_for_context(tail.value, contextual);
                tail.value = self.get_widened_type(tail.value);
            }
            if let Some(previous) = tails.iter_mut().find(|previous| previous.start == tail.start) {
                previous.value = if collision_candidates { self.union_reduced(&[previous.value, tail.value]) }
                    else { self.intersection(&[previous.value, tail.value]) };
                previous.readonly = if collision_candidates { previous.readonly && tail.readonly } else { previous.readonly || tail.readonly };
                if tail.origin == NativeTableFieldOrigin::Declared { previous.origin = tail.origin; }
            } else { tails.push(tail); }
        }
        if collision_candidates {
            for field in &mut merged {
                let key_type = self.native_table_key_type(field.key);
                for indexer in &domains {
                    if self.native_table_inference_key_matches(key_type, indexer.key) {
                        field.optional |= self.contains_undefined(indexer.value);
                        let present = self.without_undefined(indexer.value);
                        field.value = self.union_reduced(&[field.value, present]);
                    }
                }
                for tail in &tails {
                    if Self::native_sequence_contains(tail.start, field.key) {
                        field.optional |= self.contains_undefined(tail.value);
                        let present = self.without_undefined(tail.value);
                        field.value = self.union_reduced(&[field.value, present]);
                    }
                }
            }
            // Deletion candidates affect collision presence above, but nil is never a stored
            // entry. Sparse metadata preserves missing reads after this type is widened/stored.
            for indexer in &mut domains { indexer.value = self.without_undefined(indexer.value); }
            for tail in &mut tails { tail.value = self.without_undefined(tail.value); }
        }
        merged.sort_by(|a, b| self.native_table_compare_keys(a.key, b.key));
        domains.sort_by_key(|indexer| indexer.key.0);
        tails.sort_by_key(|tail| (tail.start, tail.value.0, tail.readonly));
        Ok(self.intern(TypeData::NativeTable(self.boxed(NativeTableData {
            environment_origin: None,
            definition_origin: None,
            nominal_origin: None,
            fields: self.list(&merged),
            indexers: self.list(&domains),
            sequence_tails: self.list(&tails),
            fresh,
            collision_candidates,
        }))))
    }

    fn native_table_compare_keys(&self, a: NativeTableKey, b: NativeTableKey) -> std::cmp::Ordering {
        use NativeTableKey::*;
        let rank = |key| match key { String(_) => 0, Integer(_) => 1, Float(_) => 2, Boolean(_) => 3 };
        rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
            (String(a), String(b)) => self.atoms().bytes(a).cmp(self.atoms().bytes(b)),
            (Integer(a), Integer(b)) => a.cmp(&b),
            (Float(a), Float(b)) => f64::from_bits(a).total_cmp(&f64::from_bits(b)),
            (Boolean(a), Boolean(b)) => a.cmp(&b),
            _ => std::cmp::Ordering::Equal,
        })
    }

    pub(super) fn native_table_valid_key_type(&self, ty: TypeId) -> bool {
        if let TypeData::Intersection(parts) = self.data(ty) {
            return parts.iter().any(|&part| self.native_table_valid_key_type(part));
        }
        self.parts(ty).iter().all(|&part| {
            let flags = self.flags(part);
            flags & (tf::ANY | tf::STRING_LIKE | tf::NUMBER_LIKE | tf::BOOLEAN | tf::BOOLEAN_LITERAL | tf::OBJECT | tf::TYPE_PARAMETER | tf::NEVER) != 0
                && !part.is_undefined() && !part.is_null()
        })
    }

    /// Instantiation keeps exact types even when a generic key becomes invalid. Inspection and
    /// operations report the invalid schema instead of weakening it or rebuilding a recipe.
    pub fn native_table_description_error(&self, ty: TypeId) -> Option<NativeTableError> {
        let TypeData::NativeTable(table) = self.data(ty) else { return None };
        if let Some(tail) = table.sequence_tails.iter().find(|tail| tail.start < 1) {
            return Some(NativeTableError::InvalidSequenceStart { start: tail.start });
        }
        table.indexers.iter().find_map(|indexer| {
            (!self.native_table_valid_key_type(indexer.key)).then_some(NativeTableError::InvalidKey {
                key: NativeTableAccessKey::Type(indexer.key),
            })
        })
    }

    pub(super) fn native_table_key_type(&self, key: NativeTableKey) -> TypeId {
        match key {
            NativeTableKey::String(value) => self.string_literal(value, false),
            NativeTableKey::Integer(value) => self.number_literal(value as f64, false),
            NativeTableKey::Float(value) => self.number_literal(f64::from_bits(value), false),
            NativeTableKey::Boolean(value) => if value { TypeId::TRUE } else { TypeId::FALSE },
        }
    }

    pub(super) fn native_table_inference_key_matches(&mut self, source: TypeId, target: TypeId) -> bool {
        let domain = if self.could_contain_type_variables(target) {
            let constraint = self.base_constraint_or_type(target);
            if constraint == target || !self.native_table_valid_key_type(constraint) { return true }
            constraint
        } else { target };
        self.is_assignable(source, domain) || self.is_assignable(domain, source)
    }

    /// Key domains intersect constituent by constituent; neither complete union needs to be a
    /// subtype of the other. Constraint reduction and atomic overlap reuse the native workers.
    pub(super) fn native_table_key_domains_overlap(&mut self, source: TypeId, target: TypeId) -> bool {
        let source = self.base_constraint_or_type(source);
        let target = self.base_constraint_or_type(target);
        if self.is_union(source) { return self.parts(source).iter().any(|&part| self.native_table_key_domains_overlap(part, target)) }
        if self.is_union(target) { return self.parts(target).iter().any(|&part| self.native_table_key_domains_overlap(source, part)) }
        self.native_table_inference_key_matches(source, target)
    }

    fn native_table_access_type(&self, key: NativeTableAccessKey) -> TypeId {
        match key {
            NativeTableAccessKey::Literal(key) => self.native_table_key_type(key),
            NativeTableAccessKey::Type(ty) => ty,
        }
    }

    pub(super) fn native_table_access(&self, key: NativeTableAccessKey) -> Result<NativeTableAccessKey, NativeTableError> {
        match key {
            NativeTableAccessKey::Literal(key) => key.normalize().map(NativeTableAccessKey::Literal),
            NativeTableAccessKey::Type(ty) => {
                if !self.native_table_valid_key_type(ty) {
                    return Err(NativeTableError::InvalidKey { key });
                }
                let literal = match *self.data(ty) {
                    TypeData::StringLit { value, .. } => Some(NativeTableKey::String(value)),
                    TypeData::NumberLit { bits, .. } => Some(NativeTableKey::Float(bits)),
                    TypeData::BoolLit { value, .. } => Some(NativeTableKey::Boolean(value)),
                    _ => None,
                };
                match literal {
                    Some(literal) => literal.normalize().map(NativeTableAccessKey::Literal),
                    None => Ok(key),
                }
            }
        }
    }

    pub fn native_table_read(&mut self, table: TypeId, key: NativeTableAccessKey) -> Result<TypeId, NativeTableError> {
        // Lua dot/index reads and colon calls share the initial primitive
        // namespace and preserve its original member/signature capability.
        if let Ok(NativeTableAccessKey::Literal(NativeTableKey::String(name))) = self.native_table_access(key)
            && let Some(member) = self.native_primitive_property(table, name)
        { return member.ok_or(NativeTableError::MissingKey { key }); }
        let table = self.native_runtime_carrier(table);
        if let Some(reference) = self.native_definition_ref(table) { return self.native_definition_member_read(reference, table, key) }
        if let TypeData::NativeObject(reference) = *self.data(table) {
            return self.native_named_read(reference, key);
        }
        if let Some(error) = self.native_table_description_error(table) { return Err(error) }
        let key = self.native_table_access(key)?;
        if let NativeTableAccessKey::Type(ty) = key && self.is_union(ty) {
            let mut values = Vec::new();
            for &part in self.parts(ty) { values.push(self.native_table_read(table, NativeTableAccessKey::Type(part))?); }
            return Ok(self.union_reduced(&values));
        }
        if self.is_any(table) { return Ok(TypeId::ANY) }
        if self.is_union(table) {
            let mut values = Vec::new();
            let mut present = false;
            let mut missing = None;
            for &part in self.parts(table) {
                match self.native_table_read(part, key) {
                    Ok(value) => { values.push(value); present = true; }
                    Err(error) if self.native_table_absent_read(part, key, error) => {
                        values.push(TypeId::UNDEFINED);
                        missing.get_or_insert(error);
                    }
                    Err(error) => return Err(error),
                }
            }
            if !present { return Err(missing.expect("a union has constituents")) }
            return Ok(self.union_reduced(&values));
        }
        let base = self.base_constraint_or_type(table);
        if base != table { let base = self.type_with_this_argument(base, table); return self.native_table_read(base, key) }
        if self.is_array_or_tuple(table) { return Err(NativeTableError::NotTable { ty: table }) }
        let TypeData::NativeTable(data) = self.data(table) else {
            if let NativeTableAccessKey::Literal(NativeTableKey::String(name)) = key
                && self.is_object_type(table)
            {
                return self.type_of_property(table, name).ok_or(NativeTableError::MissingKey { key });
            }
            return Err(NativeTableError::NotTable { ty: table });
        };
        if let Some(environment) = data.environment_origin {
            return Ok(self.native_global_environment_member(environment, key));
        }
        if let NativeTableAccessKey::Literal(literal) = key
            && let Some(field) = data.fields.iter().find(|field| field.key == literal)
        {
            return Ok(if field.optional { self.optional(field.value) } else { field.value });
        }
        let key_type = self.native_table_access_type(key);
        let mut values = Vec::new();
        for indexer in &data.indexers {
            if self.is_assignable(key_type, indexer.key)
                || data.collision_candidates && self.is_assignable(indexer.key, key_type)
            {
                values.push(if indexer.sparse { self.optional(indexer.value) } else { indexer.value });
            }
        }
        for tail in &data.sequence_tails {
            if self.native_sequence_access_matches(tail.start, key) {
                values.push(self.optional(tail.value));
            }
        }
        if !values.is_empty() && (!data.sequence_tails.is_empty() || data.collision_candidates)
            && let NativeTableAccessKey::Type(_) = key {
            for field in &data.fields {
                if self.is_assignable(self.native_table_key_type(field.key), key_type) {
                    values.push(if field.optional { self.optional(field.value) } else { field.value });
                }
            }
        }
        if values.is_empty() {
            if !data.indexers.is_empty() {
                let expected = self.union(&data.indexers.iter().map(|indexer| indexer.key).collect::<Vec<_>>());
                return Err(NativeTableError::KeyType { key: key_type, expected });
            }
            return Err(NativeTableError::MissingKey { key });
        }
        Ok(if data.collision_candidates || !data.sequence_tails.is_empty() && matches!(key, NativeTableAccessKey::Type(_)) {
            self.union_reduced(&values)
        } else { self.intersection(&values) })
    }

    /// A physical Lua table contributes nil when a different union constituent
    /// supplies the queried key. This never turns an invalid receiver into a table.
    pub(super) fn native_table_absent_read(&mut self, table: TypeId, key: NativeTableAccessKey, error: NativeTableError) -> bool {
        // A broad key might also select one of this constituent's declared
        // fields. A rejected lookup cannot prove those possible values are nil.
        if !matches!(key, NativeTableAccessKey::Literal(_)) { return false }
        if !matches!(error, NativeTableError::MissingKey { .. } | NativeTableError::KeyType { .. }) { return false }
        let table = self.native_runtime_carrier(table);
        if matches!(self.data(table), TypeData::NativeTable(_) | TypeData::NativeObject(_) | TypeData::NativeDefinition(_)) { return true }
        let base = self.base_constraint_or_type(table);
        base != table && self.native_table_absent_read(base, key, error)
    }

    pub fn native_table_value_context(&mut self, expected: TypeId, key: NativeTableAccessKey) -> Option<TypeId> {
        if let Some(reference) = self.native_definition_ref(expected) { return self.native_definition_value_context(reference, key) }
        if self.is_union(expected) {
            // Contextual object properties draw information from declarations;
            // unlike a runtime read, an absent constituent supplies no context.
            let mut values = Vec::new();
            for &part in self.parts(expected) { values.extend(self.native_table_value_context(part, key)); }
            return if values.is_empty() { None } else { Some(self.union(&values)) };
        }
        if let TypeData::Intersection(parts) = self.data(expected) {
            let mut values = Vec::new();
            let mut declared = Vec::new();
            for &part in parts.iter() {
                let field = match (self.data(part), key) {
                    (TypeData::NativeTable(table), NativeTableAccessKey::Literal(key)) => table.fields.iter()
                        .find(|field| field.key == key).map(|field| if field.optional { self.optional(field.value) } else { field.value }),
                    (_, NativeTableAccessKey::Literal(NativeTableKey::String(name))) => self.type_of_property(part, name),
                    _ => None,
                };
                declared.extend(field);
                if field.is_none() { values.extend(self.native_table_value_context(part, key)); }
            }
            if !declared.is_empty() { values = declared; }
            for value in &mut values { if self.is_any(*value) { *value = TypeId::UNKNOWN; } }
            return if values.is_empty() { None } else { Some(self.intersection(&values)) };
        }
        self.native_table_read(expected, key).ok()
    }

    pub fn native_table_write(&mut self, table: TypeId, key: NativeTableAccessKey, value: TypeId) -> Result<(), NativeTableError> {
        let table = self.native_runtime_carrier(table);
        if let Some(reference) = self.native_definition_ref(table) { return self.native_definition_write(reference, key, value) }
        let table = self.native_object_resolved(table).map_err(|_| NativeTableError::NotTable { ty: table })?;
        if let Some(error) = self.native_table_description_error(table) { return Err(error) }
        let key = self.native_table_access(key)?;
        if let NativeTableAccessKey::Type(ty) = key && self.is_union(ty) {
            for &part in self.parts(ty) { self.native_table_write(table, NativeTableAccessKey::Type(part), value)?; }
            return Ok(());
        }
        if self.is_any(table) { return Ok(()) }
        if self.is_union(table) {
            for &part in self.parts(table) { self.native_table_write(part, key, value)?; }
            return Ok(());
        }
        let base = self.base_constraint_or_type(table);
        if base != table { let base = self.type_with_this_argument(base, table); return self.native_table_write(base, key, value) }
        if self.is_array_or_tuple(table) { return Err(NativeTableError::NotTable { ty: table }) }
        let TypeData::NativeTable(data) = self.data(table) else {
            if let NativeTableAccessKey::Literal(NativeTableKey::String(name)) = key
                && let Some(expected) = self.write_type_of_property(table, name)
            {
                return if self.is_assignable(value, expected) { Ok(()) } else { Err(NativeTableError::ValueType { value, expected }) };
            }
            return Err(NativeTableError::NotTable { ty: table });
        };
        if let Some(environment) = data.environment_origin {
            let expected = self.native_global_environment_member(environment, key);
            return if self.is_assignable(value, expected) { Ok(()) } else { Err(NativeTableError::ValueType { value, expected }) };
        }
        for tail in &data.sequence_tails {
            if tail.readonly && self.native_sequence_access_matches(tail.start, key) {
                return Err(NativeTableError::ReadOnly { key });
            }
        }
        if let NativeTableAccessKey::Literal(literal) = key
            && let Some(field) = data.fields.iter().find(|field| field.key == literal)
        {
            if field.readonly { return Err(NativeTableError::ReadOnly { key }) }
            if self.contains_undefined(value) && !field.optional && !self.contains_undefined(field.value) {
                return Err(NativeTableError::RequiredDeletion { key: literal });
            }
            let present = self.without_undefined(value);
            return if self.is_assignable(present, field.value) { Ok(()) } else { Err(NativeTableError::ValueType { value, expected: field.value }) };
        }
        let key_type = self.native_table_access_type(key);
        let mut matched = false;
        for indexer in &data.indexers {
            if !self.is_assignable(key_type, indexer.key) { continue }
            matched = true;
            if indexer.readonly { return Err(NativeTableError::ReadOnly { key }) }
            let present = self.without_undefined(value);
            if !self.is_assignable(present, indexer.value) {
                return Err(NativeTableError::ValueType { value, expected: indexer.value });
            }
        }
        for tail in &data.sequence_tails {
            if !self.native_sequence_access_matches(tail.start, key) { continue }
            matched = true;
            let present = self.without_undefined(value);
            if !self.is_assignable(present, tail.value) {
                return Err(NativeTableError::ValueType { value, expected: tail.value });
            }
        }
        if !data.sequence_tails.is_empty() && let NativeTableAccessKey::Type(_) = key {
            for field in &data.fields {
                if !self.is_assignable(self.native_table_key_type(field.key), key_type) { continue }
                matched = true;
                if field.readonly { return Err(NativeTableError::ReadOnly { key }) }
                if self.contains_undefined(value) && !field.optional && !self.contains_undefined(field.value) {
                    return Err(NativeTableError::RequiredDeletion { key: field.key });
                }
                let present = self.without_undefined(value);
                if !self.is_assignable(present, field.value) {
                    return Err(NativeTableError::ValueType { value, expected: field.value });
                }
            }
        }
        if matched { Ok(()) } else if !data.indexers.is_empty() {
            let expected = self.union(&data.indexers.iter().map(|indexer| indexer.key).collect::<Vec<_>>());
            Err(NativeTableError::KeyType { key: key_type, expected })
        } else { Err(NativeTableError::MissingKey { key }) }
    }

    /// Lua key selection stays native; the selected values use the ordinary relation worker and
    /// its recursion, inference-marker, diagnostic and relation caches.
    pub(super) fn native_tables_related<const REPORT: bool>(
        &mut self,
        r: &mut Relater,
        source: &NativeTableData<'_>,
        target: &NativeTableData<'_>,
        state: u8,
    ) -> Ternary {
        if r.relation == Relation::Identity && (source.environment_origin.is_some() || target.environment_origin.is_some()) {
            return if source.environment_origin == target.environment_origin { Ternary::TRUE } else { Ternary::FALSE };
        }
        let mut target_description;
        let target = if let Some(environment) = target.environment_origin {
            target_description = target.clone_in(self.arena);
            let fields = self.native_global_environment_fields(environment);
            target_description.fields = self.list(&fields);
            &target_description
        } else { target };
        let mut source_description;
        let source = if let Some(environment) = source.environment_origin
            && target.indexers.iter().any(|indexer| !self.is_any(indexer.value)
                && !source.indexers.iter().any(|actual| self.is_assignable(indexer.key, actual.key)))
        {
            source_description = source.clone_in(self.arena);
            let fields = self.native_global_environment_fields(environment);
            source_description.fields = self.list(&fields);
            &source_description
        } else { source };
        let identity = r.relation == Relation::Identity;
        if let Some(target_origin) = target.nominal_origin {
            if source.nominal_origin.is_none_or(|source_origin| !self.native_nominal_reference_compatible(source_origin, target_origin, identity)) {
                return Ternary::FALSE;
            }
        } else if identity && source.nominal_origin.is_some() { return Ternary::FALSE }
        if source.indexers.iter().chain(target.indexers.iter()).any(|indexer| !self.native_table_valid_key_type(indexer.key)) {
            return Ternary::FALSE;
        }
        if identity && (source.fields.len() != target.fields.len() || source.indexers.len() != target.indexers.len()
            || source.sequence_tails.len() != target.sequence_tails.len()
            || (!source.indexers.is_empty() || !source.sequence_tails.is_empty()) && source.collision_candidates != target.collision_candidates)
        {
            return Ternary::FALSE;
        }
        if source.fresh && !identity {
            for field in &source.fields {
                if field.optional && field.value.is_never() { continue }
                if target.fields.iter().any(|other| other.key == field.key) { continue }
                let key = self.native_table_key_type(field.key);
                if !target.indexers.iter().any(|indexer| self.is_assignable(key, indexer.key))
                    && !target.sequence_tails.iter().any(|tail| Self::native_sequence_contains(tail.start, field.key))
                {
                    return Ternary::FALSE;
                }
            }
            for indexer in &source.indexers {
                if indexer.value.is_never() { continue }
                if !target.indexers.iter().any(|other| self.is_assignable(indexer.key, other.key)) {
                    return Ternary::FALSE;
                }
            }
            for tail in &source.sequence_tails {
                if tail.value.is_never() { continue }
                if !target.indexers.iter().any(|indexer| self.is_assignable(TypeId::NUMBER, indexer.key))
                    && !target.sequence_tails.iter().any(|other| tail.start >= other.start)
                { return Ternary::FALSE }
            }
        }
        let mut result = Ternary::TRUE;
        for field in &target.fields {
            if let Some(environment) = source.environment_origin {
                let actual = self.native_global_environment_member(environment, NativeTableAccessKey::Literal(field.key));
                let expected = if field.optional { self.optional(field.value) } else { field.value };
                let related = self.is_related_to_ex::<REPORT>(r, actual, expected, REC_BOTH, state);
                if !related.holds() { return related }
                result &= related;
                continue;
            }
            let Some(actual) = source.fields.iter().find(|actual| actual.key == field.key) else {
                let mut values = Vec::new();
                for tail in &source.sequence_tails {
                    if Self::native_sequence_contains(tail.start, field.key) { values.push(self.optional(tail.value)); }
                }
                if !values.is_empty() && !identity {
                    let actual = if source.collision_candidates { self.union(&values) } else { self.intersection(&values) };
                    let expected = if field.optional { self.optional(field.value) } else { field.value };
                    let related = self.is_related_to_ex::<REPORT>(r, actual, expected, REC_BOTH, state);
                    if !related.holds() { return related }
                    result &= related;
                    continue;
                }
                if field.optional || self.contains_undefined(field.value) && !identity { continue }
                return Ternary::FALSE;
            };
            if identity && (actual.optional != field.optional || actual.readonly != field.readonly) {
                return Ternary::FALSE;
            }
            let actual_type = if actual.optional { self.optional(actual.value) } else { actual.value };
            let expected_type = if field.optional { self.optional(field.value) } else { field.value };
            let related = self.is_related_to_ex::<REPORT>(r, actual_type, expected_type, REC_BOTH, state);
            if !related.holds() { return Ternary::FALSE }
            result &= related;
        }
        for indexer in &target.indexers {
            let expected = indexer.value;
            let mut matched = false;
            let mut covering_domain = false;
            for actual in &source.indexers {
                let matches = if identity {
                    actual.readonly == indexer.readonly && actual.sparse == indexer.sparse
                        && self.is_related_to_ex::<REPORT>(r, actual.key, indexer.key, REC_BOTH, state).holds()
                } else {
                    self.is_assignable(actual.key, indexer.key) || self.is_assignable(indexer.key, actual.key)
                };
                if !matches { continue }
                matched = true;
                // Index signatures restrict present entries. Sparse lookup presence does not add
                // a nil entry; constructor nil values delete entries. A declared nil union stays.
                let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                let related = self.is_related_to_ex::<REPORT>(r, value, expected, REC_BOTH, state);
                if !related.holds() { return Ternary::FALSE }
                result &= related;
                covering_domain |= self.is_assignable(indexer.key, actual.key);
            }
            if identity && !matched { return Ternary::FALSE }
            // TypeScript's typeRelatedToIndexInfo uses an applicable explicit
            // source index signature before inferring an index from properties.
            // A covering Lua domain follows that same rule; merely overlapping
            // domains still need their individual field/tail checks below.
            if !identity && covering_domain { continue }
            for actual in &source.fields {
                let key = self.native_table_key_type(actual.key);
                if !self.is_assignable(key, indexer.key) { continue }
                matched = true;
                let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                let related = self.is_related_to_ex::<REPORT>(r, value, expected, REC_BOTH, state);
                if !related.holds() { return Ternary::FALSE }
                result &= related;
            }
            if !identity {
                for actual in &source.sequence_tails {
                    if !self.native_sequence_type_overlaps(actual.start, indexer.key) { continue }
                    matched = true;
                    let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                    let related = self.is_related_to_ex::<REPORT>(r, value, expected, REC_BOTH, state);
                    if !related.holds() { return related }
                    result &= related;
                }
            }
            // Closed shapes have an inferable index, as ordinary TS object shapes do. A dictionary
            // with exclusively different domains does not acquire a new domain from its shape.
            if !matched && (source.indexers.iter().any(|indexer| !indexer.value.is_never())
                || source.sequence_tails.iter().any(|tail| !tail.value.is_never())) { return Ternary::FALSE }
        }
        for tail in &target.sequence_tails {
            for actual in &source.sequence_tails {
                if identity && (actual.start != tail.start || actual.readonly != tail.readonly) { continue }
                let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                let related = self.is_related_to_ex::<REPORT>(r, value, tail.value, REC_BOTH, state);
                if !related.holds() { return related }
                result &= related;
            }
            if identity {
                if !source.sequence_tails.iter().any(|actual| actual.start == tail.start && actual.readonly == tail.readonly) { return Ternary::FALSE }
                continue;
            }
            for actual in &source.indexers {
                if !self.native_sequence_type_overlaps(tail.start, actual.key) { continue }
                let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                let related = self.is_related_to_ex::<REPORT>(r, value, tail.value, REC_BOTH, state);
                if !related.holds() { return related }
                result &= related;
            }
            for actual in &source.fields {
                if !Self::native_sequence_contains(tail.start, actual.key)
                    || target.fields.iter().any(|field| field.key == actual.key) { continue }
                let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                let related = self.is_related_to_ex::<REPORT>(r, value, tail.value, REC_BOTH, state);
                if !related.holds() { return related }
                result &= related;
            }
        }
        result
    }

    /// Old/native class namespaces expose genuine byte-string names. They can participate in
    /// native table relations without manufacturing JS names for numeric or boolean Lua keys.
    pub(super) fn native_table_object_description(&mut self, ty: TypeId) -> Option<NativeTableData<'s>> {
        let ty = self.native_runtime_carrier(ty);
        let ty = self.native_object_schema_resolved(ty).ok()?;
        if let TypeData::NativeTable(table) = self.data(ty) { return Some((**table).clone_in(self.arena)) }
        if self.is_array_or_tuple(ty) { return None }
        let members = self.structured_members(ty)?;
        let mut fields = Vec::new();
        for prop in &members.shape().props {
            fields.push(NativeTableField {
                key: NativeTableKey::String(prop.name),
                value: self.type_of_prop(prop, members.mapper),
                optional: prop.flags.contains(PropFlags::OPTIONAL),
                readonly: prop.flags.contains(PropFlags::READONLY),
                origin: if self.is_fresh_object_literal_type(ty) { NativeTableFieldOrigin::Inferred } else { NativeTableFieldOrigin::Declared },
            });
        }
        let indexers = members.index_infos().iter().map(|indexer| NativeTableIndexer {
            key: self.instantiate(indexer.key, members.mapper),
            value: self.instantiate(indexer.value, members.mapper),
            readonly: indexer.readonly,
            sparse: false,
            origin: if self.is_fresh_object_literal_type(ty) { NativeTableFieldOrigin::Inferred } else { NativeTableFieldOrigin::Declared },
        }).collect::<Vec<_>>();
        Some(NativeTableData {
            environment_origin: None,
            definition_origin: None,
            nominal_origin: None,
            fields: self.list(&fields), indexers: self.list(&indexers),
            sequence_tails: self.list(&[]),
            fresh: self.is_fresh_object_literal_type(ty),
            collision_candidates: self.is_fresh_object_literal_type(ty),
        })
    }

    pub(super) fn native_table_widened(&mut self, table: &NativeTableData<'p>) -> TypeId {
        let fields = table.fields.iter().map(|field| NativeTableField {
            value: if field.origin == NativeTableFieldOrigin::Declared { self.regular(field.value) } else { self.get_widened_type(field.value) }, ..*field
        }).collect::<Vec<_>>();
        let indexers = table.indexers.iter().map(|indexer| NativeTableIndexer {
            key: self.get_widened_type(indexer.key),
            value: if indexer.origin == NativeTableFieldOrigin::Declared { self.regular(indexer.value) } else { self.get_widened_type(indexer.value) }, ..*indexer
        }).collect::<Vec<_>>();
        let sequence_tails = table.sequence_tails.iter().map(|tail| NativeTableSequenceTail {
            value: if tail.origin == NativeTableFieldOrigin::Declared { self.regular(tail.value) } else { self.get_widened_type(tail.value) }, ..*tail
        }).collect::<Vec<_>>();
        let definition_origin = table.definition_origin.map(|reference| NativeDefinitionRef { view: self.get_widened_type(reference.view), ..reference });
        self.intern(TypeData::NativeTable(self.boxed(NativeTableData {
            environment_origin: table.environment_origin,
            definition_origin,
            nominal_origin: table.nominal_origin,
            fields: self.list(&fields), indexers: self.list(&indexers),
            sequence_tails: self.list(&sequence_tails), fresh: false,
            collision_candidates: table.collision_candidates,
        })))
    }
}
