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
    /// A written Lua sequence contract (`T[]`), rather than a numeric dictionary.
    /// This is structural syntax information, independent of nominal class identity.
    pub is_array: bool,
    /// An original dense constructor row whose nil-capable slots would be
    /// erased from stored fields. This is a physical result row, not an array
    /// contract or a promise inferred from a sparse dictionary.
    pub constructor_sequence: Option<NativeResultPack>,
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
    PendingContext,
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
    /// Lua arrays have no JS members, but their sequence contract must survive
    /// structural materialization, inference and mapper substitution.
    pub fn native_array_table(&mut self, element: TypeId, readonly: bool) -> Result<TypeId, NativeTableError> {
        let table = self.native_table(&[], &[NativeTableIndexer {
            key: TypeId::NUMBER, value: element, readonly, sparse: false,
            origin: NativeTableFieldOrigin::Declared,
        }], NativeTableFlavor::Declared)?;
        let TypeData::NativeTable(table) = self.data(table) else { unreachable!() };
        let mut table = (**table).clone_in(self.arena);
        table.is_array = true;
        Ok(self.intern(TypeData::NativeTable(self.boxed(table))))
    }

    /// Admission to a Lua array is a sequence question, not vacuous numeric
    /// index-signature compatibility. Values still use the shared relation and
    /// inference workers after this metadata-only check.
    pub(super) fn native_array_source_is_sequence(&mut self, source: &NativeTableData<'_>) -> bool {
        if source.nominal_origin.is_some() || source.definition_origin.is_some() || source.environment_origin.is_some() {
            return false;
        }
        if source.is_array { return true }
        if !source.indexers.is_empty() { return false }
        let prefix = if let Some(pack) = source.constructor_sequence {
            let Some(rows) = self.native_pack_alternatives(pack) else { return false };
            let [row] = rows.as_slice() else { return false };
            if row.fixed.iter().any(|slot| slot.optional) { return false }
            row.fixed.len() as i64
        } else { 0 };
        let mut count = prefix;
        let mut maximum = prefix;
        for field in &source.fields {
            if field.optional && field.value.is_never() { continue }
            let NativeTableKey::Integer(index) = field.key else { return false };
            if index < 1 { return false }
            if index <= prefix { continue }
            if field.optional { return false }
            count += 1;
            maximum = maximum.max(index);
        }
        if count != maximum { return false }
        source.sequence_tails.iter().all(|tail| tail.start <= count + 1)
    }

    /// Assertions can compare a structural numeric facet without promising
    /// that the whole source is a sequence. Extra fixed properties do not
    /// remove that overlap; disjoint indexer domains still do.
    fn native_array_source_has_numeric_overlap(&mut self, source: &NativeTableData<'_>) -> bool {
        source.nominal_origin.is_none() && source.definition_origin.is_none()
            && source.environment_origin.is_none() && !source.indexers.is_empty()
            && source.indexers.iter().all(|indexer| self.native_table_key_domains_overlap(TypeId::NUMBER, indexer.key))
    }

    fn native_constructor_sequence(&mut self, fields: &[NativeTableField], indexers: &[NativeTableIndexer], tails: &[NativeTableSequenceTail], fresh: bool, expected: Option<TypeId>) -> Option<NativeResultPack> {
        if !indexers.is_empty() || tails.len() > 1
            || !fields.iter().any(|field| self.contains_undefined(field.value))
                && !tails.iter().any(|tail| self.contains_undefined(tail.value)) { return None }
        let mut entries = Vec::with_capacity(fields.len());
        for field in fields {
            let NativeTableKey::Integer(index) = field.key.normalize().ok()? else { return None };
            if field.optional || index < 1 { return None }
            let mut ty = field.value;
            if field.origin == NativeTableFieldOrigin::Declared { ty = self.regular(ty); }
            else if fresh {
                let context = expected.and_then(|expected| self.native_table_value_context(expected, NativeTableAccessKey::Literal(field.key)));
                ty = self.widen_literal_for_context(ty, context);
                ty = self.get_widened_type(ty);
            }
            entries.push((index, ty));
        }
        entries.sort_by_key(|(index, _)| *index);
        if entries.iter().enumerate().any(|(offset, (index, _))| *index != offset as i64 + 1) { return None }
        let tail = if let Some(tail) = tails.first() {
            if tail.start != entries.len() as i64 + 1 { return None }
            let mut ty = tail.value;
            if tail.origin == NativeTableFieldOrigin::Declared { ty = self.regular(ty); }
            else if fresh { ty = self.get_widened_type(ty); }
            Some(ty)
        } else { None };
        Some(self.native_result_pack(&[NativePackAlternative {
            fixed: entries.into_iter().map(|(_, ty)| NativePackSlot { ty, optional: false }).collect(), tail,
        }]))
    }

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
        let constructor_sequence = data.constructor_sequence;
        let result = self.native_table_from_description_with_policy(&fields, &data.indexers, &tails, data.fresh, data.collision_candidates)?;
        if constructor_sequence.is_none() { return Ok(result) }
        let TypeData::NativeTable(result) = self.data(result) else { unreachable!() };
        let mut result = (**result).clone_in(self.arena);
        result.constructor_sequence = constructor_sequence;
        Ok(self.intern(TypeData::NativeTable(self.boxed(result))))
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
            if !self.native_table_valid_indexer_key(indexer, fresh) {
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
            is_array: false,
            constructor_sequence: None,
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
        let constructor_sequence = if collision_candidates {
            self.native_constructor_sequence(fields, indexers, sequence_tails, fresh, expected)
        } else { None };
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
            if !self.native_table_valid_indexer_key(&indexer, fresh) {
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
            is_array: false,
            constructor_sequence,
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
        if let TypeData::Union(parts) = self.data(ty) {
            return parts.iter().all(|&part| self.native_table_valid_key_type(part));
        }
        if let TypeData::Intersection(parts) = self.data(ty) {
            return parts.iter().any(|&part| self.native_table_valid_key_type(part));
        }
        self.parts(ty).iter().all(|&part| {
            let flags = self.flags(part);
            flags & (tf::ANY | tf::STRING_LIKE | tf::NUMBER_LIKE | tf::BOOLEAN | tf::BOOLEAN_LITERAL | tf::OBJECT | tf::TYPE_PARAMETER | tf::NEVER) != 0
                && !part.is_undefined() && !part.is_null()
        })
    }

    /// As in Luanalysis v1, a written key type may include nil (`table<nil | string, V>`): the
    /// table then accepts lookups whose key may be nil, which Lua answers with nil. A table
    /// constructor's keys (`{ [key] = value }`) are values, and a nil one is an error.
    fn native_table_valid_indexer_key(&self, indexer: &NativeTableIndexer, fresh: bool) -> bool {
        if self.native_table_valid_key_type(indexer.key) { return true }
        // A written `void` key type (a generic table instantiated with void) indexes nothing,
        // as v1 accepts it.
        if !fresh && indexer.key == TypeId::VOID { return true }
        let nil = |part: &TypeId| part.is_undefined() || part.is_null();
        !fresh && self.native_table_key_has_nil(indexer.key)
            && self.parts(indexer.key).iter().any(|part| !nil(part))
            && self.parts(indexer.key).iter().all(|part| nil(part) || self.native_table_valid_key_type(*part))
    }

    fn native_table_key_has_nil(&self, ty: TypeId) -> bool {
        self.parts(ty).iter().any(|part| part.is_undefined() || part.is_null())
    }

    /// The key a lookup with a possibly nil key reads or writes, when `table` declares nil keys.
    fn native_table_nil_key_access(&mut self, table: TypeId, key: NativeTableAccessKey) -> Option<TypeId> {
        let NativeTableAccessKey::Type(ty) = key else { return None };
        if !self.native_table_key_has_nil(ty) { return None }
        let TypeData::NativeTable(data) = self.data(table) else { return None };
        if !data.indexers.iter().any(|indexer| self.native_table_key_has_nil(indexer.key)) {
            return None;
        }
        let present: Vec<TypeId> = self.parts(ty).iter().copied().filter(|part| !part.is_undefined() && !part.is_null()).collect();
        Some(self.union(&present))
    }

    /// Instantiation keeps exact types even when a generic key becomes invalid. Inspection and
    /// operations report the invalid schema instead of weakening it or rebuilding a recipe.
    pub fn native_table_description_error(&self, ty: TypeId) -> Option<NativeTableError> {
        let TypeData::NativeTable(table) = self.data(ty) else { return None };
        if let Some(tail) = table.sequence_tails.iter().find(|tail| tail.start < 1) {
            return Some(NativeTableError::InvalidSequenceStart { start: tail.start });
        }
        table.indexers.iter().find_map(|indexer| {
            (!self.native_table_valid_indexer_key(indexer, table.fresh)).then_some(NativeTableError::InvalidKey {
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

    /// Assignment needs evidence of a shared domain; an unresolved target
    /// parameter is a possible overlap for inference, not evidence of one.
    fn native_table_known_key_domains_overlap(&mut self, source: TypeId, target: TypeId) -> bool {
        let source = self.base_constraint_or_type(source);
        let target = self.base_constraint_or_type(target);
        if self.is_union(source) { return self.parts(source).iter().any(|&part| self.native_table_known_key_domains_overlap(part, target)) }
        if self.is_union(target) { return self.parts(target).iter().any(|&part| self.native_table_known_key_domains_overlap(source, part)) }
        self.is_assignable(source, target) || self.is_assignable(target, source)
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

    /// The language service's members of a Lua value, as `properties_of_type` lists a
    /// TypeScript type's: each string key a read of `table` accepts, with the type that read
    /// returns. Names come from every table representation; a read decides which remain, so a
    /// union keeps the keys all its constituents accept.
    pub fn native_completion_members(&mut self, table: TypeId) -> Vec<(Atom, TypeId)> {
        let mut names = Vec::new();
        self.native_completion_names(table, &mut names, 0);
        let mut seen = std::collections::HashSet::new();
        names.retain(|name| seen.insert(*name));
        names.into_iter().filter_map(|name| {
            let key = NativeTableAccessKey::Literal(NativeTableKey::String(name));
            self.native_table_read(table, key).ok().map(|value| (name, value))
        }).collect()
    }

    /// `keyof` of a Lua table: the type of each key it has, as Lua indexes it. A named field's
    /// key is its string literal and another literal key its own literal type, an index
    /// signature gives its key type and a sequence `number`. `None`: `ty` is not a Lua table, so
    /// TypeScript's `keyof` applies.
    pub(super) fn native_key_type(&mut self, ty: TypeId) -> Option<TypeId> {
        let ty = self.native_runtime_carrier(ty);
        let resolved = match *self.data(ty) {
            TypeData::NativeArray { .. } => return Some(TypeId::NUMBER),
            TypeData::NativeTable(_) => ty,
            TypeData::NativeObject(_) | TypeData::NativeRuntimeTable(_) | TypeData::NativeDefinition(_) => {
                self.native_object_schema_resolved(ty).unwrap_or(TypeId::ERROR)
            }
            _ => return None,
        };
        let TypeData::NativeTable(data) = self.data(resolved) else {
            // A runtime table's view or a definition's table that is itself another representation.
            return if resolved == ty || self.is_error_type(resolved) { Some(TypeId::NEVER) } else { self.native_key_type(resolved).or(Some(TypeId::NEVER)) };
        };
        let mut keys: Vec<TypeId> = Vec::new();
        if let Some(environment) = data.environment_origin {
            for (name, _) in self.native_global_environment_properties(environment) { keys.push(self.string_literal(name, false)); }
        }
        let (fields, indexers, sequence) = (data.fields.to_vec(), data.indexers.to_vec(), !data.sequence_tails.is_empty() || data.constructor_sequence.is_some());
        keys.extend(fields.iter().map(|field| self.native_table_key_type(field.key)));
        keys.extend(indexers.iter().map(|indexer| indexer.key));
        if sequence { keys.push(TypeId::NUMBER); }
        Some(self.union(&keys))
    }

    /// Whether `ty` is a Lua table representation, or a union or intersection with one, so that
    /// a type's index into it reads it as Lua does.
    pub(super) fn is_native_table_type(&self, ty: TypeId) -> bool {
        match self.data(ty) {
            TypeData::NativeTable(_) | TypeData::NativeObject(_) | TypeData::NativeRuntimeTable(_)
            | TypeData::NativeDefinition(_) | TypeData::NativeArray { .. } => true,
            TypeData::NativeRuntimeTop { carrier, .. } => *carrier != ty && self.is_native_table_type(*carrier),
            TypeData::Union(parts) | TypeData::Intersection(parts) => parts.iter().any(|&part| self.is_native_table_type(part)),
            _ => false,
        }
    }

    fn native_completion_names(&mut self, ty: TypeId, names: &mut Vec<Atom>, depth: usize) {
        if depth > 16 { return }
        let ty = self.native_runtime_carrier(ty);
        if self.is_union(ty) || self.is_intersection(ty) {
            for part in self.constituents(ty).to_vec() { self.native_completion_names(part, names, depth + 1); }
            return;
        }
        if let Some(provider) = self.native_globals_provider
            && let Some(environment) = provider.primitive_member_environment()
        {
            let apparent = self.base_constraint_of(ty).unwrap_or(ty);
            if self.flags(apparent) & crate::types::tf::STRING_LIKE != 0 {
                if let Some(binding) = provider.primitive_member_binding(environment, super::native_globals::NativePrimitiveMemberKind::String)
                    && let Ok(namespace) = self.native_global_binding_type(binding)
                    && namespace != ty
                {
                    self.native_completion_names(namespace, names, depth + 1);
                }
                return;
            }
        }
        if let Some(reference) = self.native_runtime_table_ref(ty) {
            self.native_completion_names(reference.view, names, depth + 1);
            names.extend(self.native_object_member_names(reference.schema));
            return;
        }
        if let Some(reference) = self.native_definition_ref(ty)
            && reference.view != ty
        {
            self.native_completion_names(reference.view, names, depth + 1);
            return;
        }
        match self.data(ty) {
            TypeData::NativeObject(reference) => {
                let reference = *reference;
                names.extend(self.native_object_member_names(reference));
                // A published allocation schema retains its constructor keys as
                // well as later stores. The seed belongs to this exact original
                // runtime declaration; resolving the full schema here would
                // unnecessarily demand every prospective stored value.
                if self.native_runtime_table_schema(ty).is_some()
                    && let Ok(seed) = self.native_runtime_table_seed(reference)
                { self.native_completion_names(seed, names, depth + 1); }
            }
            TypeData::NativeTable(data) => {
                if let Some(environment) = data.environment_origin {
                    names.extend(self.native_global_environment_properties(environment).into_iter().map(|(name, _)| name));
                    return;
                }
                names.extend(data.fields.iter().filter_map(|field| match field.key {
                    NativeTableKey::String(name) => Some(name),
                    _ => None,
                }));
                if let Some(origin) = data.nominal_origin { names.extend(self.native_object_member_names(origin)); }
            }
            _ => {
                if let Some(base) = self.base_constraint_of(ty)
                    && base != ty
                {
                    return self.native_completion_names(base, names, depth + 1);
                }
                if self.is_object_type(ty) && let Some(members) = self.members(ty) {
                    names.extend(members.shape().props.iter().map(|prop| prop.name));
                }
            }
        }
    }

    pub fn native_table_read(&mut self, table: TypeId, key: NativeTableAccessKey) -> Result<TypeId, NativeTableError> {
        // Lua dot/index reads and colon calls share the initial primitive
        // namespace and preserve its original member/signature capability.
        if let Ok(NativeTableAccessKey::Literal(NativeTableKey::String(name))) = self.native_table_access(key)
            && let Some(member) = self.native_primitive_property(table, name)
        { return member; }
        let table = self.native_runtime_carrier(table);
        // A generic table at a generic key of its own, `object[key]` where `key: K` and
        // `K: keyof T`, is `T[K]`, as a TypeScript element access is.
        if let NativeTableAccessKey::Type(key_type) = key
            && self.is_generic_object_type(table)
            && self.is_generic_index_type(key_type)
        {
            let keys = self.keyof(table);
            if self.is_assignable(key_type, keys)
                && let Some(value) = self.indexed_access_type(table, key_type)
            { return Ok(value) }
        }
        if let Some(reference) = self.native_runtime_table_ref(table) { return self.native_runtime_table_read(reference, key) }
        if let Some(reference) = self.native_definition_ref(table) { return self.native_definition_member_read(reference, table, key) }
        if let TypeData::NativeObject(reference) = *self.data(table) {
            return self.native_named_read(reference, key);
        }
        if let Some(error) = self.native_table_description_error(table) { return Err(error) }
        if let Some(present) = self.native_table_nil_key_access(table, key) {
            if present.is_never() { return Ok(TypeId::UNDEFINED) }
            let value = self.native_table_read(table, NativeTableAccessKey::Type(present))?;
            return Ok(self.optional(value));
        }
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
        if self.is_intersection(table) { return self.native_intersection_read(table, key) }
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
        if !values.is_empty() && let NativeTableAccessKey::Type(_) = key {
            let fallback = if data.collision_candidates || !data.sequence_tails.is_empty() {
                self.union_reduced(&values)
            } else { self.intersection(&values) };
            values.clear();
            values.push(fallback);
            for field in &data.fields {
                if self.native_table_key_domains_overlap(self.native_table_key_type(field.key), key_type) {
                    values.push(if field.optional { self.optional(field.value) } else { field.value });
                }
            }
            return Ok(self.union_reduced(&values));
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

    /// A member of an intersection, as TypeScript reads one: from the parts that declare it, as
    /// what they all give, and only when none does, from every part, so that an index signature
    /// applies only to names no part declares. `Statics & { [string]: Value | nil }` keeps each
    /// of its statics' types and gives any other name `Value | nil`.
    fn native_intersection_read(&mut self, table: TypeId, key: NativeTableAccessKey) -> Result<TypeId, NativeTableError> {
        let parts = self.constituents(table).to_vec();
        if let NativeTableAccessKey::Literal(literal) = key {
            let mut declared = Vec::new();
            for &part in &parts {
                if self.native_declares_member(part, literal) && let Ok(value) = self.native_table_read(part, key) { declared.push(value); }
            }
            if !declared.is_empty() { return Ok(self.intersection(&declared)) }
        }
        let mut values = Vec::new();
        let mut failure = None;
        for &part in &parts {
            match self.native_table_read(part, key) {
                Ok(value) => values.push(value),
                Err(error) => { failure.get_or_insert(error); }
            }
        }
        if values.is_empty() { return Err(failure.unwrap_or(NativeTableError::MissingKey { key })) }
        let fallback = self.intersection(&values);
        if let NativeTableAccessKey::Type(key_type) = key {
            let mut values = vec![fallback];
            for literal in self.native_intersection_possible_fields(&parts, key_type) {
                values.push(self.native_intersection_read(table, NativeTableAccessKey::Literal(literal))?);
            }
            return Ok(self.union_reduced(&values));
        }
        Ok(fallback)
    }

    fn native_intersection_possible_fields(&mut self, parts: &[TypeId], key: TypeId) -> Vec<NativeTableKey> {
        let mut keys = Vec::new();
        for &part in parts {
            let part = self.base_constraint_or_type(part);
            let Some(table) = self.native_table_object_description(part) else { continue };
            for field in &table.fields {
                if !keys.contains(&field.key)
                    && self.native_table_key_domains_overlap(self.native_table_key_type(field.key), key)
                { keys.push(field.key); }
            }
        }
        keys
    }

    fn native_intersection_write(&mut self, table: TypeId, key: NativeTableAccessKey, value: TypeId) -> Result<(), NativeTableError> {
        let parts = self.constituents(table).to_vec();
        if let NativeTableAccessKey::Literal(literal) = key {
            let mut declared = false;
            for &part in &parts {
                if self.native_declares_member(part, literal) {
                    self.native_table_write(part, key, value)?;
                    declared = true;
                }
            }
            if declared { return Ok(()) }
        }
        let mut matched = false;
        let mut failure = None;
        for &part in &parts {
            match self.native_table_write(part, key, value) {
                Ok(()) => matched = true,
                Err(error @ (NativeTableError::MissingKey { .. } | NativeTableError::KeyType { .. })) => { failure.get_or_insert(error); },
                Err(error) => return Err(error),
            }
        }
        if !matched { return Err(failure.unwrap_or(NativeTableError::MissingKey { key })) }
        if let NativeTableAccessKey::Type(key_type) = key {
            for literal in self.native_intersection_possible_fields(&parts, key_type) {
                self.native_intersection_write(table, NativeTableAccessKey::Literal(literal), value)?;
            }
        }
        Ok(())
    }

    /// Whether `ty` declares the member `key` itself, rather than admitting it by an index
    /// signature. A part of another kind declares what it can be read for.
    fn native_declares_member(&mut self, ty: TypeId, key: NativeTableKey) -> bool {
        match *self.data(ty) {
            TypeData::NativeTable(_) => {
                let TypeData::NativeTable(data) = self.data(ty) else { unreachable!() };
                data.fields.iter().any(|field| field.key == key)
            }
            TypeData::NativeObject(reference) => self.native_object_declares(reference, key),
            _ => self.native_table_read(ty, NativeTableAccessKey::Literal(key)).is_ok(),
        }
    }

    /// A physical Lua table contributes nil when a different union constituent
    /// supplies the queried key. This never turns an invalid receiver into a table.
    pub(super) fn native_table_absent_read(&mut self, table: TypeId, key: NativeTableAccessKey, error: NativeTableError) -> bool {
        // A broad key might also select one of this constituent's declared
        // fields. A rejected lookup cannot prove those possible values are nil.
        if !matches!(error, NativeTableError::MissingKey { .. } | NativeTableError::KeyType { .. }) { return false }
        let table = self.native_runtime_carrier(table);
        if matches!(key, NativeTableAccessKey::Type(TypeId::NUMBER))
            && let Some(reference) = self.native_runtime_table_ref(table)
            && let TypeData::NativeTable(data) = self.data(reference.view)
        {
            // A reached sparse numeric facet on another branch admits this
            // truly empty numeric continuation as nil. Inspect only its
            // existing view; future store recipes cannot supply values.
            return data.indexers.is_empty() && data.sequence_tails.is_empty()
                && !data.fields.iter().any(|field| matches!(field.key, NativeTableKey::Integer(_) | NativeTableKey::Float(_)));
        }
        if !matches!(key, NativeTableAccessKey::Literal(_)) { return false }
        if matches!(self.data(table), TypeData::NativeRuntimeTable(_) | TypeData::NativeTable(_) | TypeData::NativeObject(_) | TypeData::NativeDefinition(_)) { return true }
        let base = self.base_constraint_or_type(table);
        base != table && self.native_table_absent_read(base, key, error)
    }

    pub fn native_table_value_context(&mut self, expected: TypeId, key: NativeTableAccessKey) -> Option<TypeId> {
        if let Some(reference) = self.native_runtime_table_ref(expected) { return self.native_runtime_table_value_context(reference, key) }
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
        if let TypeData::NativeObject(reference) = *self.data(expected) { return self.native_named_context_value(reference, key) }
        self.native_table_read(expected, key).ok()
    }

    pub fn native_table_write(&mut self, table: TypeId, key: NativeTableAccessKey, value: TypeId) -> Result<(), NativeTableError> {
        let table = self.native_runtime_carrier(table);
        if let Some(reference) = self.native_runtime_table_ref(table) { return self.native_runtime_table_write(reference, key, value) }
        if let Some(reference) = self.native_definition_ref(table) { return self.native_definition_write(reference, key, value) }
        let table = self.native_object_resolved(table).map_err(|error| error.table_error(table))?;
        if let Some(error) = self.native_table_description_error(table) { return Err(error) }
        if let Some(present) = self.native_table_nil_key_access(table, key) {
            if present.is_never() { return Ok(()) }
            return self.native_table_write(table, NativeTableAccessKey::Type(present), value);
        }
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
        if self.is_intersection(table) { return self.native_intersection_write(table, key, value) }
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
            if self.contains_undefined(value) && !field.optional && !self.contains_undefined(field.value)
                && (self.native_strict_nil || !self.is_assignable(TypeId::UNDEFINED, field.value)) {
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
        if matched && let NativeTableAccessKey::Type(_) = key {
            for field in &data.fields {
                if !self.native_table_key_domains_overlap(self.native_table_key_type(field.key), key_type) { continue }
                matched = true;
                if field.readonly { return Err(NativeTableError::ReadOnly { key }) }
                if self.contains_undefined(value) && !field.optional && !self.contains_undefined(field.value)
                    && (self.native_strict_nil || !self.is_assignable(TypeId::UNDEFINED, field.value)) {
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
        if identity && source.is_array != target.is_array
            || target.is_array && !self.native_array_source_is_sequence(source)
                && !(r.relation == Relation::Comparable && self.native_array_source_has_numeric_overlap(source)) {
            return Ternary::FALSE;
        }
        if r.relation == Relation::Comparable && source.is_array
            && !self.native_array_source_is_sequence(target)
            && !self.native_array_source_has_numeric_overlap(target)
        { return Ternary::FALSE }
        if let Some(target_origin) = target.nominal_origin {
            if source.nominal_origin.is_none_or(|source_origin| !self.native_nominal_reference_compatible(source_origin, target_origin, identity)) {
                return Ternary::FALSE;
            }
        } else if identity && source.nominal_origin.is_some() { return Ternary::FALSE }
        if source.indexers.iter().any(|indexer| !self.native_table_valid_indexer_key(indexer, source.fresh))
            || target.indexers.iter().any(|indexer| !self.native_table_valid_indexer_key(indexer, target.fresh))
        {
            return Ternary::FALSE;
        }
        if identity && (source.fields.len() != target.fields.len() || source.indexers.len() != target.indexers.len()
            || source.sequence_tails.len() != target.sequence_tails.len()
            || (!source.indexers.is_empty() || !source.sequence_tails.is_empty()) && source.collision_candidates != target.collision_candidates)
        {
            return Ternary::FALSE;
        }
        if !target.indexers.is_empty() {
            // Dictionary contracts are partial key-domain views. A nonempty
            // source must provide a compatible domain, rather than satisfy an
            // unrelated indexer vacuously. Other domains retain record width.
            // Opaque instances can expose members without becoming iterable tables.
            if source.nominal_origin.is_some_and(|origin| self.native_opaque_runtime_object(origin))
                && !target.nominal_origin.is_some_and(|origin| self.native_opaque_runtime_object(origin))
            { return Ternary::FALSE }
            if !identity {
                let mut keys = target.indexers.iter().map(|indexer| indexer.key).collect::<Vec<_>>();
                keys.extend(target.fields.iter().map(|field| self.native_table_key_type(field.key)));
                let domain = self.union(&keys);
                let mut present = false;
                let mut compatible = false;
                for field in &source.fields {
                    if field.value.is_never() { continue }
                    present = true;
                    let key = self.native_table_key_type(field.key);
                    compatible |= if r.relation == Relation::Comparable {
                        self.native_table_key_domains_overlap(key, domain)
                    } else { self.native_table_known_key_domains_overlap(key, domain) };
                }
                for indexer in &source.indexers {
                    if indexer.value.is_never() { continue }
                    present = true;
                    compatible |= if r.relation == Relation::Comparable {
                        self.native_table_key_domains_overlap(indexer.key, domain)
                    } else { self.native_table_known_key_domains_overlap(indexer.key, domain) };
                }
                for tail in &source.sequence_tails {
                    if tail.value.is_never() { continue }
                    present = true;
                    compatible |= self.native_sequence_type_overlaps(tail.start, domain);
                }
                if present && !compatible { return Ternary::FALSE }
            }
        }
        // Lua structural contracts permit additional source keys, including on a
        // constructor. Each requested dictionary domain checks its shared values.
        // Freshness controls contextual literal widening and constructor collisions;
        // target members and indexer values are checked below.
        let mut result = Ternary::TRUE;
        if target.is_array && let Some(pack) = source.constructor_sequence {
            let expected = self.native_result_pack(&[NativePackAlternative { fixed: Vec::new(), tail: Some(target.indexers[0].value) }]);
            let related = self.is_related_to_ex::<REPORT>(r, pack.ty, expected.ty, REC_BOTH, state);
            if !related.holds() { return related }
            result &= related;
        }
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
                if !identity && (field.optional || self.contains_undefined(field.value)) {
                    let key = self.native_table_key_type(field.key);
                    for indexer in &source.indexers {
                        if self.native_table_key_domains_overlap(key, indexer.key) { values.push(self.optional(indexer.value)); }
                    }
                }
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
            for actual in &source.indexers {
                let matches = if identity {
                    actual.readonly == indexer.readonly && actual.sparse == indexer.sparse
                        && self.is_related_to_ex::<REPORT>(r, actual.key, indexer.key, REC_BOTH, state).holds()
                } else {
                    self.native_table_key_domains_overlap(actual.key, indexer.key)
                };
                if !matches { continue }
                matched = true;
                // Index signatures restrict present entries. Sparse lookup presence does not add
                // a nil entry; constructor nil values delete entries. A declared nil union stays.
                let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                let related = self.is_related_to_ex::<REPORT>(r, value, expected, REC_BOTH, state);
                if !related.holds() { return Ternary::FALSE }
                result &= related;
            }
            if identity && !matched { return Ternary::FALSE }
            // An explicitly open any contract is the existing gradual escape;
            // it controls this indexer view without refining it from fixed keys.
            if !identity && source.indexers.iter().any(|actual| {
                actual.origin == NativeTableFieldOrigin::Declared
                    && self.is_any(actual.key) && self.is_any(actual.value)
            }) { continue }
            for actual in &source.fields {
                let key = self.native_table_key_type(actual.key);
                if !self.native_table_key_domains_overlap(key, indexer.key) { continue }
                // The target's exact contract was checked above. Its fallback
                // applies only to keys with no explicit target field.
                if target.fields.iter().any(|field| field.key == actual.key) { continue }
                let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                let related = self.is_related_to_ex::<REPORT>(r, value, expected, REC_BOTH, state);
                if !related.holds() { return Ternary::FALSE }
                result &= related;
            }
            if !identity {
                for actual in &source.sequence_tails {
                    if !self.native_sequence_type_overlaps(actual.start, indexer.key) { continue }
                    let value = if source.collision_candidates { self.without_undefined(actual.value) } else { actual.value };
                    let related = self.is_related_to_ex::<REPORT>(r, value, expected, REC_BOTH, state);
                    if !related.holds() { return related }
                    result &= related;
                }
            }
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
            is_array: false,
            constructor_sequence: None,
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
        let constructor_sequence = table.constructor_sequence.map(|pack| NativeResultPack { ty: self.get_widened_type(pack.ty) });
        self.intern(TypeData::NativeTable(self.boxed(NativeTableData {
            is_array: table.is_array,
            constructor_sequence,
            environment_origin: table.environment_origin,
            definition_origin,
            nominal_origin: table.nominal_origin,
            fields: self.list(&fields), indexers: self.list(&indexers),
            sequence_tails: self.list(&sequence_tails), fresh: false,
            collision_candidates: table.collision_candidates,
        })))
    }
}
