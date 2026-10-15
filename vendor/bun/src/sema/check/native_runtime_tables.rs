//! Original Lua allocation schemas and persistent structural value views.
use super::*;
use crate::types::{NativeRuntimeTableRef, NativeTableFieldOrigin};

impl<'p, 's> Checker<'p, 's> {
    pub(super) fn with_runtime_table_context<R>(&mut self, reference: NativeObjectReference, work: impl FnOnce(&mut Self) -> R) -> R {
        let contextual = std::mem::take(&mut self.native_function_contexts);
        let captured = std::mem::take(&mut self.native_capture_contexts);
        if let Some(context) = reference.runtime_context {
            self.native_function_contexts.extend(context.contextual);
            self.native_capture_contexts.extend(context.captured);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(self)));
        self.native_function_contexts = contextual;
        self.native_capture_contexts = captured;
        result.unwrap_or_else(|error| std::panic::resume_unwind(error))
    }

    pub(super) fn native_runtime_receiver_matches(&self, receiver: TypeId, expected: NativeObjectReference) -> bool {
        if self.is_union(receiver) { return self.parts(receiver).iter().any(|&part| self.native_runtime_receiver_matches(part, expected)) }
        self.native_runtime_table_schema(receiver).is_some_and(|schema| matches!(*self.data(schema), TypeData::NativeObject(reference) if reference.declaration == expected.declaration && reference.runtime_context == expected.runtime_context))
    }
    pub fn native_runtime_table_ref(&self, ty: TypeId) -> Option<NativeRuntimeTableRef> {
        match *self.data(ty) { TypeData::NativeRuntimeTable(reference) => Some(reference), _ => None }
    }

    pub fn native_runtime_table_schema(&self, ty: TypeId) -> Option<TypeId> {
        if let Some(reference) = self.native_runtime_table_ref(ty) { return Some(self.intern(TypeData::NativeObject(reference.schema))) }
        if let TypeData::NativeObject(reference) = *self.data(ty)
            && self.native_named_provider(reference.declaration).is_ok_and(|provider| provider.type_declaration_header(self, reference.declaration).kind == NativeTypeDeclarationKind::RuntimeTable)
        { return Some(ty) }
        None
    }

    /// Publish a value without requesting its prospective schema or seed. This
    /// is the bootstrap boundary for resolving a later store's frozen receiver.
    pub fn native_runtime_table_value(&mut self, declaration: NativeTypeDeclarationId, actual: TypeId) -> Result<TypeId, NativeNamedTypeError> {
        let provider = self.native_named_provider(declaration)?;
        let header = provider.type_declaration_header(self, declaration);
        if header.kind != NativeTypeDeclarationKind::RuntimeTable || provider.runtime_table_constructor(declaration).is_none() {
            return Err(NativeNamedTypeError::InvalidDeclaration(declaration));
        }
        let head = self.native_declared_head(declaration)?;
        let TypeData::NativeObject(mut schema) = *self.data(head) else { return Err(NativeNamedTypeError::InvalidDeclaration(declaration)) };
        let contextual = self.current_native_function_context();
        let captured = self.current_native_captured_function();
        if contextual.is_some() || captured.is_some() { schema.runtime_context = Some(NativeRuntimeTableContext { contextual, captured }); }
        let view = self.native_runtime_table_ref(actual).map_or(actual, |reference| reference.view);
        if !matches!(self.data(view), TypeData::NativeTable(_)) {
            return Err(NativeNamedTypeError::InvalidDeclaration(declaration));
        }
        let bounds = self.native_runtime_table_ref(actual).map_or(Some(view), |reference| reference.bounds);
        Ok(self.intern(TypeData::NativeRuntimeTable(NativeRuntimeTableRef { schema, view, bounds })))
    }

    fn native_runtime_table_seed(&mut self, reference: NativeObjectReference) -> Result<TypeId, NativeNamedTypeError> {
        let declaration = reference.declaration;
        let object = self.intern(TypeData::NativeObject(reference));
        let native_query = NativeTypeDeclarationQuery::RuntimeTableSeed(object);
        if let Some(&known) = self.native_named_cache.types.get(&native_query) { return Ok(known) }
        let provider = self.native_named_provider(declaration)?;
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) { return Err(NativeNamedTypeError::TooDeep(native_query)) }
        let result = self.with_runtime_table_context(reference, |checker| provider.runtime_table_seed(checker, declaration)).ok_or(NativeNamedTypeError::InvalidDeclaration(declaration));
        let left = self.leave(query);
        match left {
            Ok(stored) => if let Ok(value) = result { self.native_named_cache.ty(native_query, value, stored); },
            Err(open) => if let Ok(value) = result { self.cache_provisionally(query, u64::from(value.0), open); },
        }
        result
    }

    fn native_runtime_member_demand(&mut self, reference: NativeObjectReference, member: NativeTypeMemberId, indexer: bool) -> Result<TypeId, NativeNamedTypeError> {
        let provider = self.native_type_declaration_provider.ok_or(NativeNamedTypeError::InvalidMember(member))?;
        let object = self.intern(TypeData::NativeObject(reference));
        let native_query = NativeTypeDeclarationQuery::RuntimeTableMember(object, member, indexer);
        if let Some(&known) = self.native_named_cache.types.get(&native_query) { return Ok(known) }
        let query = Query::NativeNamed(native_query);
        if !self.enter(query) { return Err(NativeNamedTypeError::TooDeep(native_query)) }
        let value = self.with_runtime_table_context(reference, |checker| {
            if indexer { provider.type_indexer_key(checker, member) } else { provider.type_member_type(checker, member) }
        });
        match self.leave(query) {
            Ok(stored) => self.native_named_cache.ty(native_query, value, stored),
            Err(open) => self.cache_provisionally(query, u64::from(value.0), open),
        }
        Ok(value)
    }

    /// Constructor fields already anchor prospective reads. Discovering every
    /// later store first would demand its receiver's earlier RHS, which can
    /// itself read this same anchored field. Only a matching written member can
    /// replace an inferred constructor field under the schema worker's policy.
    pub(super) fn native_runtime_seed_read(&mut self, reference: NativeObjectReference, key: NativeTableAccessKey) -> Result<Option<TypeId>, NativeNamedTypeError> {
        let key = self.native_table_access(key).map_err(|error| NativeNamedTypeError::Table { declaration: reference.declaration, error })?;
        let NativeTableAccessKey::Literal(literal) = key else { return Ok(None) };
        let seed = self.native_runtime_table_seed(reference)?;
        let seed = self.instantiate(seed, reference.mapper);
        let TypeData::NativeTable(data) = self.data(seed) else { return Err(NativeNamedTypeError::InvalidDeclaration(reference.declaration)) };
        let Some(field) = data.fields.iter().find(|field| field.key == literal) else { return Ok(None) };
        let members = if field.origin == NativeTableFieldOrigin::Declared { Vec::new() }
            else { self.native_object_written_fixed_members(reference, literal)? };
        // Reuse the ordinary seed widening, optionality, mapper and written
        // overlay rules. No partial object schema is published by this read.
        let table = self.native_runtime_schema_resolved(reference, &members)?;
        self.native_table_read(table, key).map(Some)
            .map_err(|error| NativeNamedTypeError::Table { declaration: reference.declaration, error })
    }

    pub(super) fn native_runtime_schema_resolved(&mut self, reference: NativeObjectReference, members: &[NativeObjectMember]) -> Result<TypeId, NativeNamedTypeError> {
        let declaration = reference.declaration;
        let seed = self.native_runtime_table_seed(reference)?;
        let seed = self.instantiate(seed, reference.mapper);
        let TypeData::NativeTable(data) = self.data(seed) else { return Err(NativeNamedTypeError::InvalidDeclaration(declaration)) };
        let mut fields = data.fields.to_vec();
        let seed_keys: Vec<_> = fields.iter().map(|field| field.key).collect();
        let mut indexers = data.indexers.to_vec();
        let mut tails = data.sequence_tails.to_vec();
        for field in &mut fields {
            field.value = if field.origin == NativeTableFieldOrigin::Declared { self.regular(field.value) } else { self.widened(field.value) };
        }
        for indexer in &mut indexers {
            indexer.value = if indexer.origin == NativeTableFieldOrigin::Declared { self.regular(indexer.value) } else { self.widened(indexer.value) };
        }
        for tail in &mut tails {
            tail.value = if tail.origin == NativeTableFieldOrigin::Declared { self.regular(tail.value) } else { self.widened(tail.value) };
        }
        let provider = self.native_named_provider(declaration)?;
        for origin in members {
            let metadata = provider.type_member_header(self, origin.member);
            let declared = provider.type_member_origin(origin.member) == NativeTableFieldOrigin::Declared;
            // A computed store does not declare an open index signature. As
            // with TS checking JS expandos, that key domain must be written.
            if !declared && matches!(metadata.key, NativeTypeMemberKey::Indexer { .. }) { continue }
            if let NativeTypeMemberKey::Fixed(key) = metadata.key {
                let key = key.normalize().map_err(|error| NativeNamedTypeError::Table { declaration, error })?;
                if let Some(previous) = fields.iter_mut().find(|field| field.key == key)
                    && (previous.origin == NativeTableFieldOrigin::Declared || !declared && seed_keys.contains(&key))
                {
                    // Original constructor fields and written contracts anchor
                    // the member. Actual writes validate their own RHS at the
                    // original store; a bad future store cannot poison this
                    // contract or force a recursive RHS to type its own bound.
                    previous.readonly |= metadata.readonly;
                    continue;
                }
            }
            let raw = self.native_runtime_member_demand(reference, origin.member, false)?;
            let value = self.native_member_type(raw, origin.owner);
            let value = if declared { self.regular(value) } else { self.widened(value) };
            match metadata.key {
                NativeTypeMemberKey::Fixed(key) => {
                    let key = key.normalize().map_err(|error| NativeNamedTypeError::Table { declaration, error })?;
                    if let Some(previous) = fields.iter_mut().find(|field| field.key == key) {
                        if declared {
                            previous.value = value;
                            previous.origin = NativeTableFieldOrigin::Declared;
                        } else {
                            previous.value = self.union(&[previous.value, value]);
                        }
                        previous.optional |= metadata.optional;
                        previous.readonly |= metadata.readonly;
                    } else {
                        fields.push(NativeTableField { key, value, optional: metadata.optional, readonly: metadata.readonly,
                            origin: if declared { NativeTableFieldOrigin::Declared } else { NativeTableFieldOrigin::Inferred } });
                    }
                }
                NativeTypeMemberKey::Indexer { sparse } => {
                    let raw = self.native_runtime_member_demand(reference, origin.member, true)?;
                    let key = self.native_member_type(raw, origin.owner);
                    indexers.push(NativeTableIndexer { key, value, readonly: metadata.readonly, sparse,
                        origin: if declared { NativeTableFieldOrigin::Declared } else { NativeTableFieldOrigin::Inferred } });
                }
            }
        }
        let constructed = self.native_table(&fields, &indexers, NativeTableFlavor::Declared)
            .map_err(|error| NativeNamedTypeError::Table { declaration, error })?;
        if tails.is_empty() { return Ok(constructed) }
        let TypeData::NativeTable(table) = self.data(constructed) else { unreachable!() };
        let mut description = (**table).clone_in(self.arena);
        description.sequence_tails = self.list(&tails);
        Ok(self.intern(TypeData::NativeTable(self.boxed(description))))
    }

    pub(in crate::check) fn native_runtime_table_read(&mut self, reference: NativeRuntimeTableRef, key: NativeTableAccessKey) -> Result<TypeId, NativeTableError> {
        match self.native_table_read(reference.view, key) {
            Ok(value) => return Ok(value),
            Err(NativeTableError::MissingKey { .. } | NativeTableError::KeyType { .. }) => {},
            Err(error) => return Err(error),
        }
        let key = self.native_table_access(key)?;
        let members = self.native_object_members(reference.schema).map_err(|error| error.table_error(TypeId::ERROR))?;
        let provider = self.native_named_provider(reference.schema.declaration).map_err(|_| NativeTableError::NotTable { ty: TypeId::ERROR })?;
        // Original fixed keys and computed key domains do not demand future
        // stored values. Their schema presence cannot prove runtime presence.
        for member in members {
            let metadata = provider.type_member_header(self, member.member);
            match metadata.key {
                NativeTypeMemberKey::Fixed(other) => {
                    if matches!(key, NativeTableAccessKey::Literal(key) if other.normalize() == Ok(key)) { return Ok(TypeId::UNDEFINED) }
                }
                NativeTypeMemberKey::Indexer { .. } => {
                    if provider.type_member_origin(member.member) != NativeTableFieldOrigin::Declared { continue }
                    let raw = self.native_runtime_member_demand(reference.schema, member.member, true).map_err(|error| error.table_error(TypeId::ERROR))?;
                    let domain = self.native_member_type(raw, member.owner);
                    let observed = match key { NativeTableAccessKey::Literal(key) => self.native_table_key_type(key), NativeTableAccessKey::Type(ty) => ty };
                    if self.is_assignable(observed, domain) { return Ok(TypeId::UNDEFINED) }
                }
            }
        }
        self.native_table_read(reference.view, key)
    }

    pub(in crate::check) fn native_runtime_table_write(&mut self, reference: NativeRuntimeTableRef, key: NativeTableAccessKey, value: TypeId) -> Result<(), NativeTableError> {
        let schema = self.intern(TypeData::NativeObject(reference.schema));
        let schema = self.native_object_resolved(schema).map_err(|error| error.table_error(schema))?;
        self.native_table_write(schema, key, value)
    }

    pub(in crate::check) fn native_runtime_table_value_context(&mut self, reference: NativeRuntimeTableRef, key: NativeTableAccessKey) -> Option<TypeId> {
        let key = self.native_table_access(key).ok()?;
        if let NativeTableAccessKey::Literal(key) = key {
            if let Some(bounds) = reference.bounds
                && let TypeData::NativeTable(data) = self.data(bounds)
                && let Some(field) = data.fields.iter().find(|field| field.key == key && field.origin == NativeTableFieldOrigin::Declared)
            { return Some(field.value) }
            if let TypeData::NativeTable(data) = self.data(reference.view)
                && data.fields.iter().any(|field| field.key == key && field.origin == NativeTableFieldOrigin::Declared)
            {
                // The reached value may have been narrowed or deleted by an
                // earlier store. Its origin retains the written constructor
                // recipe, whose invariant bound supplies RHS context instead.
                let seed = self.native_runtime_table_seed(reference.schema).ok()?;
                let seed = self.instantiate(seed, reference.schema.mapper);
                if let TypeData::NativeTable(data) = self.data(seed)
                    && let Some(field) = data.fields.iter().find(|field| field.key == key && field.origin == NativeTableFieldOrigin::Declared)
                { return Some(field.value) }
            }
            let object = self.intern(TypeData::NativeObject(reference.schema));
            let query = Query::NativeNamed(NativeTypeDeclarationQuery::RuntimeTableWrittenContext(object, key));
            if !self.enter(query) { self.mark_tainted_from(0); return None }
            let (result, completed) = self.run_memoizable(|checker| checker.native_runtime_written_context(reference.schema, key));
            let ended = self.leave(query);
            if completed.is_none() || ended.is_err() { self.mark_tainted_from(0); return None }
            return result.ok().flatten();
        }
        None
    }

    fn native_runtime_written_context(&mut self, reference: NativeObjectReference, key: NativeTableKey) -> Result<Option<TypeId>, NativeNamedTypeError> {
        let provider = self.native_named_provider(reference.declaration)?;
        let mut values = Vec::new();
        for origin in self.native_object_written_fixed_members(reference, key)? {
            if self.native_object_member_effective_origin(&origin)? != Some(NativeTableFieldOrigin::Declared) { continue }
            let raw = if matches!(provider.type_member_header(self, origin.member).owner, NativeTypeMemberOwner::RuntimeStore(_)) {
                self.native_runtime_member_demand(origin.owner, origin.member, false)?
            } else {
                let Some(value) = self.native_object_member_demand(&origin)? else { continue };
                value
            };
            values.push(self.native_member_type(raw, origin.owner));
        }
        Ok(if values.is_empty() { None } else { Some(self.intersection(&values)) })
    }

    /// Flow transfers already hold an independently checked original store.
    /// Updating presence must not request a schema or another store's receiver.
    pub(in crate::check) fn native_runtime_table_flow_write(&mut self, table: TypeId, key: NativeTableKey, value: TypeId) -> TypeId {
        if self.is_union(table) { return self.map_type(table, |c, part| c.native_runtime_table_flow_write(part, key, value)) }
        let Some(reference) = self.native_runtime_table_ref(table) else { return table };
        let Ok(key) = key.normalize() else { return table };
        let TypeData::NativeTable(data) = self.data(reference.view) else { return table };
        let mut description = (**data).clone_in(self.arena);
        let mut fields = data.fields.to_vec();
        let optional = self.contains_undefined(value);
        let present = self.without_undefined(value);
        let bound = reference.bounds.and_then(|bounds| match self.data(bounds) {
            TypeData::NativeTable(data) => data.fields.iter().find(|field| field.key == key && field.origin == NativeTableFieldOrigin::Declared).map(|field| (field.value, field.readonly)),
            _ => None,
        });
        let reached = bound.map_or(present, |(bound, _)| self.resolved_assignment_type(bound, present));
        if let Some(field) = fields.iter_mut().find(|field| field.key == key) {
            // Validation uses the original schema. Narrow against the original
            // written constructor bound, never the previous reached value.
            // This also retains regular singleton literals without retaining
            // fresh literals beyond a written broad number/string bound.
            field.value = reached;
            field.optional = optional;
        } else {
            fields.push(NativeTableField { key, value: reached, optional, readonly: bound.is_some_and(|(_, readonly)| readonly),
                origin: if bound.is_some() { NativeTableFieldOrigin::Declared } else { NativeTableFieldOrigin::Inferred } });
        }
        description.fields = self.list(&fields);
        description.fresh = false;
        let view = self.intern(TypeData::NativeTable(self.boxed(description)));
        self.intern(TypeData::NativeRuntimeTable(NativeRuntimeTableRef { view, ..reference }))
    }

    pub(in crate::check) fn native_has_runtime_table(&self, ty: TypeId) -> bool {
        if self.is_union(ty) { return self.parts(ty).iter().any(|&part| self.native_has_runtime_table(part)) }
        self.native_runtime_table_ref(ty).is_some()
    }
    pub(in crate::check) fn native_runtime_table_absent_view(&mut self, ty: TypeId) -> TypeId {
        if self.is_union(ty) { return self.map_type(ty, |checker, part| checker.native_runtime_table_absent_view(part)) }
        let bounds = self.native_runtime_table_ref(ty).and_then(|reference| reference.bounds);
        let Some(schema) = self.native_runtime_table_schema(ty) else { return ty };
        let TypeData::NativeObject(schema) = *self.data(schema) else { unreachable!("runtime schema") };
        let view = self.native_table(&[], &[], NativeTableFlavor::Declared).expect("empty original value view");
        self.intern(TypeData::NativeRuntimeTable(NativeRuntimeTableRef { schema, view, bounds }))
    }
}
