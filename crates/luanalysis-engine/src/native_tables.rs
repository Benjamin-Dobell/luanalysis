//! Owned Lua table descriptions and key queries at the native checker boundary.
//!
//! The checker owns key normalization, constructor collisions, indexer lookup,
//! assignability, freshness, and deletion rules. This module only translates
//! owned descriptions to and from types in the current checker session.

use super::{Checker, SessionQueries, Type, TypeHandles, ValuePack, export_type, import_type};
use bun_sema::check::native_tables::NativeTableError;
use bun_sema::types::{
    NativeTableAccessKey, NativeTableField, NativeTableFlavor, NativeTableIndexer, NativeTableKey,
    NativeTableSequenceTail, TypeData, TypeId,
};

/// An exact Lua runtime key. String keys retain their bytes, and integer keys
/// retain Lua's full integer precision independently of numeric value types.
#[derive(Debug, Clone, PartialEq)]
pub enum LuaKey {
    String(Vec<u8>),
    Integer(i64),
    Number(f64),
    Boolean(bool),
}

/// A literal address or the type of a dynamic source key expression.
#[derive(Debug, Clone, PartialEq)]
pub enum TableKey {
    Literal(LuaKey),
    Type(Type),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableField {
    pub key: LuaKey,
    pub value: Type,
    pub optional: bool,
    pub readonly: bool,
}

impl TableField {
    pub fn new(key: LuaKey, value: Type) -> Self {
        Self {
            key,
            value,
            optional: false,
            readonly: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableIndexer {
    pub key: Type,
    pub value: Type,
    pub readonly: bool,
    /// Inferred dynamic constructor entries may be absent. Declared indexers
    /// retain their declared value type on reads, matching the TS default.
    pub sparse: bool,
}

impl TableIndexer {
    pub fn new(key: Type, value: Type) -> Self {
        Self {
            key,
            value,
            readonly: false,
            sparse: false,
        }
    }
}

/// Open integer sequence entries at or above `start`; keys below it and
/// fractional numeric keys are not included in this domain.
#[derive(Debug, Clone, PartialEq)]
pub struct TableSequenceTail {
    pub start: i64,
    pub value: Type,
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableType {
    pub fields: Vec<TableField>,
    pub indexers: Vec<TableIndexer>,
    pub sequence_tails: Vec<TableSequenceTail>,
    /// Literal construction may combine overlapping candidate values by union.
    /// This origin survives storage independently of excess-property freshness.
    pub collision_candidates: bool,
    /// An original constructor value, eligible for the checker's fresh-literal
    /// relation. Stored and declared table descriptions use `false`.
    pub fresh: bool,
}

/// A structured native table failure. Source adapters choose the original key
/// or value span; neither a diagnostic location nor a backend type ID crosses
/// this boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum TableError {
    InvalidKey { key: TableKey },
    NotTable { ty: Type },
    MissingKey { key: TableKey },
    KeyType { key: Type, expected: Type },
    ValueType { value: Type, expected: Type },
    ReadOnly { key: TableKey },
    RequiredDeletion { key: LuaKey },
    DuplicateDeclaredField { key: LuaKey },
    InvalidSequenceStart { start: i64 },
    SequenceTooComplex { count: usize, alternatives: usize },
    UnresolvedSequence,
}

pub(super) fn import_table(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    table: &TableType,
) -> TypeId {
    let (fields, indexers) = import_descriptors(checker, handles, table);
    let tails = import_sequence_tails(checker, handles, table);
    checker
        .native_table_from_description_with_policy(
            &fields,
            &indexers,
            &tails,
            table.fresh,
            table.collision_candidates,
        )
        .map_err(|error| export_error(checker, handles, error))
        .unwrap_or_else(|error| std::panic::panic_any(error))
}

fn try_import_table(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    table: &TableType,
) -> Result<TypeId, TableError> {
    try_import_table_with_context(checker, handles, table, None)
}

fn try_import_table_with_context(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    table: &TableType,
    expected: Option<&Type>,
) -> Result<TypeId, TableError> {
    let (fields, indexers) = import_descriptors(checker, handles, table);
    let tails = import_sequence_tails(checker, handles, table);
    let flavor = if table.fresh {
        NativeTableFlavor::Literal
    } else {
        NativeTableFlavor::Declared
    };
    let expected = expected.map(|ty| import_type(checker, handles, ty));
    checker
        .native_table_with_context_and_sequence_policy(
            &fields,
            &indexers,
            &tails,
            flavor,
            table.collision_candidates || table.fresh,
            expected,
        )
        .map_err(|error| export_error(checker, handles, error))
}

fn import_descriptors(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    table: &TableType,
) -> (Vec<NativeTableField>, Vec<NativeTableIndexer>) {
    let fields = table
        .fields
        .iter()
        .map(|field| NativeTableField {
            key: import_literal_key(checker, &field.key),
            value: import_type(checker, handles, &field.value),
            optional: field.optional,
            readonly: field.readonly,
        })
        .collect();
    let indexers = table
        .indexers
        .iter()
        .map(|indexer| NativeTableIndexer {
            key: import_type(checker, handles, &indexer.key),
            value: import_type(checker, handles, &indexer.value),
            readonly: indexer.readonly,
            sparse: indexer.sparse,
        })
        .collect();
    (fields, indexers)
}

fn import_sequence_tails(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    table: &TableType,
) -> Vec<NativeTableSequenceTail> {
    table
        .sequence_tails
        .iter()
        .map(|tail| NativeTableSequenceTail {
            start: tail.start,
            value: import_type(checker, handles, &tail.value),
            readonly: tail.readonly,
        })
        .collect()
}

pub(super) fn export_table(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    ty: TypeId,
) -> Option<TableType> {
    if checker.native_table_description_error(ty).is_some() {
        // A generic mapper can produce a domain that a written Lua table
        // recipe would reject. Keep that immutable backend type as an exact
        // capability; its operations report the native schema failure.
        return None;
    }
    let TypeData::NativeTable(table) = checker.data(ty) else {
        return None;
    };
    // A class description is a session capability, not a forgeable structural table recipe.
    if table.nominal_origin.is_some() { return None }
    // Copy the immutable native descriptors before recursively exporting any
    // member. Recursion may demand further types from this same checker.
    let fields = table.fields.to_vec();
    let indexers = table.indexers.to_vec();
    let tails = table.sequence_tails.to_vec();
    let fresh = table.fresh;
    let collision_candidates = table.collision_candidates;
    Some(TableType {
        fields: fields
            .into_iter()
            .map(|field| TableField {
                key: export_literal_key(checker, field.key),
                value: export_type(checker, handles, field.value),
                optional: field.optional,
                readonly: field.readonly,
            })
            .collect(),
        indexers: indexers
            .into_iter()
            .map(|indexer| TableIndexer {
                key: export_type(checker, handles, indexer.key),
                value: export_type(checker, handles, indexer.value),
                readonly: indexer.readonly,
                sparse: indexer.sparse,
            })
            .collect(),
        sequence_tails: tails
            .into_iter()
            .map(|tail| TableSequenceTail {
                start: tail.start,
                value: export_type(checker, handles, tail.value),
                readonly: tail.readonly,
            })
            .collect(),
        fresh,
        collision_candidates,
    })
}

impl SessionQueries<'_, '_, '_> {
    pub(super) fn array_type_native(&mut self, element: &Type) -> Type {
        self.table_type_native(&TableType {
            indexers: vec![TableIndexer::new(Type::Number, element.clone())],
            ..Default::default()
        })
        .unwrap_or_else(|error| std::panic::panic_any(error))
    }

    pub(super) fn table_type_native(&mut self, table: &TableType) -> Result<Type, TableError> {
        let ty = try_import_table(self.checker, self.handles, table)?;
        Ok(export_type(self.checker, self.handles, ty))
    }

    pub(super) fn table_type_in_context_native(
        &mut self,
        table: &TableType,
        expected: &Type,
    ) -> Result<Type, TableError> {
        let ty = try_import_table_with_context(self.checker, self.handles, table, Some(expected))?;
        Ok(export_type(self.checker, self.handles, ty))
    }

    pub(super) fn table_with_sequence_native(
        &mut self,
        table: &TableType,
        start: i64,
        pack: &ValuePack,
        expected: Option<&Type>,
    ) -> Result<Type, TableError> {
        let (fields, indexers) = import_descriptors(self.checker, self.handles, table);
        let tails = import_sequence_tails(self.checker, self.handles, table);
        let pack = super::native_packs::import_pack(self.checker, self.handles, pack);
        let expected = expected.map(|ty| import_type(self.checker, self.handles, ty));
        let flavor = if table.fresh {
            NativeTableFlavor::Literal
        } else {
            NativeTableFlavor::Declared
        };
        self.checker
            .native_table_with_sequence_and_policy(
                &fields,
                &indexers,
                &tails,
                flavor,
                table.collision_candidates || table.fresh,
                start,
                pack,
                expected,
            )
            .map(|ty| export_type(self.checker, self.handles, ty))
            .map_err(|error| export_error(self.checker, self.handles, error))
    }

    pub(super) fn table_sequence_context_native(
        &mut self,
        expected: &Type,
        start: i64,
    ) -> Option<ValuePack> {
        let expected = import_type(self.checker, self.handles, expected);
        self.checker
            .native_table_sequence_context(expected, start)
            .map(|pack| self.handles.retain_pack(self.checker, pack))
    }

    pub(super) fn table_lookup_native(
        &mut self,
        table: &Type,
        key: &TableKey,
    ) -> Result<Type, TableError> {
        let table = import_type(self.checker, self.handles, table);
        let key = import_access_key(self.checker, self.handles, key);
        self.checker
            .native_table_read(table, key)
            .map(|ty| export_type(self.checker, self.handles, ty))
            .map_err(|error| export_error(self.checker, self.handles, error))
    }

    pub(super) fn table_write_native(
        &mut self,
        table: &Type,
        key: &TableKey,
        value: &Type,
    ) -> Result<(), TableError> {
        let table = import_type(self.checker, self.handles, table);
        let key = import_access_key(self.checker, self.handles, key);
        let value = import_type(self.checker, self.handles, value);
        self.checker
            .native_table_write(table, key, value)
            .map_err(|error| export_error(self.checker, self.handles, error))
    }

    pub(super) fn table_value_context_native(
        &mut self,
        expected: &Type,
        key: &TableKey,
    ) -> Option<Type> {
        let expected = import_type(self.checker, self.handles, expected);
        let key = import_access_key(self.checker, self.handles, key);
        // Preserve an exact contextual type as a session capability. Exporting
        // a callable description here would demand its return type too early.
        self.checker
            .native_table_value_context(expected, key)
            .map(|ty| self.handles.retain(ty))
    }
}

fn import_literal_key(checker: &Checker<'_, '_>, key: &LuaKey) -> NativeTableKey {
    match key {
        LuaKey::String(bytes) => NativeTableKey::String(checker.atoms().intern(bytes)),
        LuaKey::Integer(value) => NativeTableKey::Integer(*value),
        LuaKey::Number(value) => NativeTableKey::Float(value.to_bits()),
        LuaKey::Boolean(value) => NativeTableKey::Boolean(*value),
    }
}

fn export_literal_key(checker: &Checker<'_, '_>, key: NativeTableKey) -> LuaKey {
    match key {
        NativeTableKey::String(value) => LuaKey::String(checker.atoms().bytes(value).to_vec()),
        NativeTableKey::Integer(value) => LuaKey::Integer(value),
        NativeTableKey::Float(bits) => LuaKey::Number(f64::from_bits(bits)),
        NativeTableKey::Boolean(value) => LuaKey::Boolean(value),
    }
}

fn import_access_key(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    key: &TableKey,
) -> NativeTableAccessKey {
    match key {
        TableKey::Literal(key) => NativeTableAccessKey::Literal(import_literal_key(checker, key)),
        TableKey::Type(ty) => NativeTableAccessKey::Type(import_type(checker, handles, ty)),
    }
}

fn export_access_key(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    key: NativeTableAccessKey,
) -> TableKey {
    match key {
        NativeTableAccessKey::Literal(key) => TableKey::Literal(export_literal_key(checker, key)),
        NativeTableAccessKey::Type(ty) => TableKey::Type(export_type(checker, handles, ty)),
    }
}

pub(super) fn export_error(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    error: NativeTableError,
) -> TableError {
    match error {
        NativeTableError::InvalidKey { key } => TableError::InvalidKey {
            key: export_access_key(checker, handles, key),
        },
        NativeTableError::NotTable { ty } => TableError::NotTable {
            ty: export_type(checker, handles, ty),
        },
        NativeTableError::MissingKey { key } => TableError::MissingKey {
            key: export_access_key(checker, handles, key),
        },
        NativeTableError::KeyType { key, expected } => TableError::KeyType {
            key: export_type(checker, handles, key),
            expected: export_type(checker, handles, expected),
        },
        NativeTableError::ValueType { value, expected } => TableError::ValueType {
            value: export_type(checker, handles, value),
            expected: export_type(checker, handles, expected),
        },
        NativeTableError::ReadOnly { key } => TableError::ReadOnly {
            key: export_access_key(checker, handles, key),
        },
        NativeTableError::RequiredDeletion { key } => TableError::RequiredDeletion {
            key: export_literal_key(checker, key),
        },
        NativeTableError::DuplicateDeclaredField { key } => TableError::DuplicateDeclaredField {
            key: export_literal_key(checker, key),
        },
        NativeTableError::InvalidSequenceStart { start } => {
            TableError::InvalidSequenceStart { start }
        }
        NativeTableError::SequenceTooComplex {
            count,
            alternatives,
        } => TableError::SequenceTooComplex {
            count,
            alternatives,
        },
        NativeTableError::UnresolvedSequence => TableError::UnresolvedSequence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Engine, NativeFunctionKey, NativeTypeParameterKey, NativeTypeProvider, Parameter,
        TypeQueries,
    };

    fn literal(key: LuaKey) -> TableKey {
        TableKey::Literal(key)
    }

    fn string_key(bytes: impl Into<Vec<u8>>) -> LuaKey {
        LuaKey::String(bytes.into())
    }

    fn dictionary(key: Type, value: Type, sparse: bool) -> TableType {
        TableType {
            indexers: vec![TableIndexer {
                key,
                value,
                readonly: false,
                sparse,
            }],
            ..Default::default()
        }
    }

    fn sequence_pack(types: Vec<Type>) -> ValuePack {
        ValuePack::Alternatives(vec![super::super::PackAlternative {
            fixed: types
                .into_iter()
                .map(super::super::PackSlot::required)
                .collect(),
            tail: None,
        }])
    }

    #[test]
    fn ranged_sequence_tails_preserve_prefix_domains_nil_and_exact_owned_roundtrips() {
        Engine::new().with_session(|queries| {
            let base = TableType {
                fields: vec![TableField::new(LuaKey::Integer(1), Type::Boolean)],
                fresh: true,
                ..Default::default()
            };
            let table = queries
                .table_with_sequence(&base, 2, &ValuePack::variadic(Type::String), None)
                .unwrap();
            assert_eq!(
                queries.table_lookup(&table, &literal(LuaKey::Integer(1))),
                Ok(Type::Boolean)
            );
            let element = queries
                .table_lookup(&table, &literal(LuaKey::Integer(2)))
                .unwrap();
            assert!(queries.is_assignable(&Type::String, &element));
            assert!(queries.is_assignable(&Type::Nil, &element));
            assert!(!queries.is_assignable(&Type::Boolean, &element));
            assert!(matches!(
                queries.table_lookup(&table, &literal(LuaKey::Integer(0))),
                Err(TableError::MissingKey { .. })
            ));
            assert!(matches!(
                queries.table_lookup(&table, &literal(LuaKey::Number(2.5))),
                Err(TableError::MissingKey { .. })
            ));
            let Type::Table(description) = &table else {
                panic!("range descriptor remains owned")
            };
            assert_eq!(
                description.sequence_tails,
                vec![TableSequenceTail {
                    start: 2,
                    value: Type::String,
                    readonly: false
                }]
            );
            let inspected = queries.inspect_type(&table);
            assert_eq!(
                queries.table_lookup(&inspected, &literal(LuaKey::Integer(1))),
                Ok(Type::Boolean)
            );
            assert!(queries.is_assignable(&table, &inspected));
            assert!(queries.is_assignable(&inspected, &table));
            assert!(
                queries
                    .table_write(&table, &literal(LuaKey::Integer(2)), &Type::String)
                    .is_ok()
            );
            assert!(
                queries
                    .table_write(&table, &literal(LuaKey::Integer(2)), &Type::Nil)
                    .is_ok()
            );
            assert!(matches!(
                queries.table_write(&table, &literal(LuaKey::Integer(2)), &Type::Number),
                Err(TableError::ValueType { .. })
            ));
            assert!(matches!(
                queries.table_write(&table, &literal(LuaKey::Integer(1)), &Type::String),
                Err(TableError::ValueType { .. })
            ));
            let string_array = queries.array_type(&Type::String);
            assert!(!queries.is_assignable(&table, &string_array));
        });
    }

    #[test]
    fn sequence_construction_distinguishes_no_store_from_explicit_nil_collision() {
        Engine::new().with_session(|queries| {
            let base = TableType {
                fields: vec![TableField::new(LuaKey::Integer(2), Type::Number)],
                fresh: true,
                ..Default::default()
            };
            let untouched = queries
                .table_with_sequence(&base, 2, &ValuePack::empty(), None)
                .unwrap();
            assert_eq!(
                queries.table_lookup(&untouched, &literal(LuaKey::Integer(2))),
                Ok(Type::Number)
            );
            let deleted = queries
                .table_with_sequence(&base, 2, &ValuePack::single(Type::Nil), None)
                .unwrap();
            let value = queries
                .table_lookup(&deleted, &literal(LuaKey::Integer(2)))
                .unwrap();
            assert!(queries.is_assignable(&Type::Nil, &value));
            let possible = queries
                .table_with_sequence(
                    &base,
                    2,
                    &ValuePack::variadic(Type::Union(vec![Type::String, Type::Nil])),
                    None,
                )
                .unwrap();
            let value = queries
                .table_lookup(&possible, &literal(LuaKey::Integer(2)))
                .unwrap();
            assert!(queries.is_assignable(&Type::Number, &value));
            assert!(queries.is_assignable(&Type::String, &value));
            assert!(queries.is_assignable(&Type::Nil, &value));
        });
    }

    #[test]
    fn sequence_pack_alternatives_remain_correlated_native_table_rows() {
        Engine::new().with_session(|queries| {
            let pack = ValuePack::Alternatives(vec![
                super::super::PackAlternative {
                    fixed: vec![super::super::PackSlot::required(Type::BooleanLiteral(
                        false,
                    ))],
                    tail: None,
                },
                super::super::PackAlternative {
                    fixed: vec![
                        super::super::PackSlot::required(Type::BooleanLiteral(true)),
                        super::super::PackSlot::required(Type::Number),
                    ],
                    tail: None,
                },
            ]);
            let base = TableType {
                fresh: true,
                ..Default::default()
            };
            let table = queries.table_with_sequence(&base, 1, &pack, None).unwrap();
            let expected = Type::Union(vec![
                Type::Table(TableType {
                    fields: vec![TableField::new(
                        LuaKey::Integer(1),
                        Type::BooleanLiteral(false),
                    )],
                    ..Default::default()
                }),
                Type::Table(TableType {
                    fields: vec![
                        TableField::new(LuaKey::Integer(1), Type::BooleanLiteral(true)),
                        TableField::new(LuaKey::Integer(2), Type::Number),
                    ],
                    ..Default::default()
                }),
            ]);
            // Constructor literal policy widens discriminants without context; the
            // expected row context is the same native input used for ordinary fields.
            let contextual = queries
                .table_with_sequence(&base, 1, &pack, Some(&expected))
                .unwrap();
            assert!(matches!(table, Type::Union(_)));
            assert!(queries.is_assignable(&contextual, &expected));
            let crossed = Type::Table(TableType {
                fields: vec![
                    TableField::new(LuaKey::Integer(1), Type::BooleanLiteral(false)),
                    TableField::new(LuaKey::Integer(2), Type::Number),
                ],
                ..Default::default()
            });
            assert!(!queries.is_assignable(&contextual, &crossed));
        });
    }

    #[test]
    fn sequence_context_and_readonly_ranges_use_native_pack_and_table_workers() {
        Engine::new().with_session(|queries| {
            let expected = Type::Table(TableType {
                fields: vec![
                    TableField::new(LuaKey::Integer(1), Type::Boolean),
                    TableField::new(LuaKey::Integer(2), Type::Number),
                    TableField::new(LuaKey::Integer(3), Type::String),
                ],
                ..Default::default()
            });
            let remaining = queries.table_sequence_context(&expected, 2).unwrap();
            assert!(
                queries
                    .pack_assignable(&remaining, &sequence_pack(vec![Type::Number, Type::String]))
            );
            assert!(!queries.pack_assignable(&ValuePack::empty(), &remaining));
            let array = queries.array_type(&Type::Number);
            let expected_tail = queries.table_sequence_context(&array, 3).unwrap();
            assert!(queries.pack_assignable(&ValuePack::empty(), &expected_tail));
            assert!(queries.pack_assignable(
                &sequence_pack(vec![Type::Number, Type::Number]),
                &expected_tail
            ));
            let readonly = queries
                .table_type(&TableType {
                    sequence_tails: vec![TableSequenceTail {
                        start: 2,
                        value: Type::String,
                        readonly: true,
                    }],
                    ..Default::default()
                })
                .unwrap();
            assert!(matches!(
                queries.table_write(&readonly, &literal(LuaKey::Integer(2)), &Type::String),
                Err(TableError::ReadOnly { .. })
            ));
            assert!(matches!(
                queries.table_with_sequence(&TableType::default(), 0, &ValuePack::empty(), None),
                Err(TableError::InvalidSequenceStart { start: 0 })
            ));
        });
    }

    #[test]
    fn constructor_collision_candidates_survive_storage_separately_from_freshness() {
        Engine::new().with_session(|queries| {
            let constructed = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(LuaKey::Integer(1), Type::Number)],
                    indexers: vec![TableIndexer::new(Type::Number, Type::String)],
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            let stored = queries.widen(&constructed);
            let Type::Table(description) = &stored else {
                panic!("stored table description")
            };
            assert!(!description.fresh);
            assert!(description.collision_candidates);
            for table in [&constructed, &stored] {
                let value = queries.table_lookup(table, &literal_key_one()).unwrap();
                assert!(queries.is_assignable(&Type::Number, &value));
                assert!(queries.is_assignable(&Type::String, &value));
                assert!(!queries.is_assignable(&Type::Nil, &value));
            }
            let reconstructed = queries.table_type(description).unwrap();
            let value = queries
                .table_lookup(&reconstructed, &literal_key_one())
                .unwrap();
            assert!(queries.is_assignable(&Type::Number, &value));
            assert!(queries.is_assignable(&Type::String, &value));
            let extended = queries
                .table_with_sequence(description, 2, &ValuePack::empty(), None)
                .unwrap();
            let value = queries.table_lookup(&extended, &literal_key_one()).unwrap();
            assert!(queries.is_assignable(&Type::Number, &value));
            assert!(queries.is_assignable(&Type::String, &value));

            fn literal_key_one() -> TableKey {
                literal(LuaKey::Integer(1))
            }
        });
    }

    #[test]
    fn exact_runtime_keys_preserve_bytes_integer_precision_and_lua_domains() {
        Engine::new().with_session(|queries| {
            let table = queries
                .table_type(&TableType {
                    fields: vec![
                        TableField::new(LuaKey::Integer(1), Type::Number),
                        TableField::new(string_key(b"1"), Type::String),
                        TableField::new(LuaKey::Boolean(true), Type::Boolean),
                        TableField::new(string_key(vec![255]), Type::Number),
                        TableField::new(LuaKey::Integer(9_007_199_254_740_992), Type::Boolean),
                        TableField::new(LuaKey::Integer(9_007_199_254_740_993), Type::String),
                    ],
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            for (key, expected) in [
                (LuaKey::Integer(1), Type::Number),
                (LuaKey::Number(1.0), Type::Number),
                (string_key(b"1"), Type::String),
                (LuaKey::Boolean(true), Type::Boolean),
                (string_key(vec![255]), Type::Number),
                (LuaKey::Integer(9_007_199_254_740_992), Type::Boolean),
                (LuaKey::Integer(9_007_199_254_740_993), Type::String),
            ] {
                assert_eq!(queries.table_lookup(&table, &literal(key)), Ok(expected));
            }
            assert!(matches!(
                queries.table_lookup(&table, &literal(string_key("missing"))),
                Err(TableError::MissingKey { .. })
            ));
            assert!(matches!(
                queries.table_lookup(&table, &TableKey::Type(Type::Nil)),
                Err(TableError::InvalidKey { .. })
            ));
            assert!(matches!(
                queries.table_lookup(&table, &literal(LuaKey::Number(f64::NAN))),
                Err(TableError::InvalidKey { .. })
            ));
            assert_eq!(
                queries.table_type(&dictionary(Type::Nil, Type::String, false)),
                Err(TableError::InvalidKey {
                    key: TableKey::Type(Type::Nil)
                })
            );
        });
    }

    #[test]
    fn constructors_use_native_collision_union_and_declared_duplicates_are_errors() {
        Engine::new().with_session(|queries| {
            let fields = vec![
                TableField::new(LuaKey::Integer(1), Type::Number),
                TableField::new(LuaKey::Number(1.0), Type::String),
            ];
            let constructor = queries
                .table_type(&TableType {
                    fields: fields.clone(),
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            let value = queries
                .table_lookup(&constructor, &literal(LuaKey::Integer(1)))
                .unwrap();
            let union = Type::Union(vec![Type::Number, Type::String]);
            assert!(queries.is_assignable(&value, &union));
            assert!(queries.is_assignable(&union, &value));
            assert!(!queries.is_assignable(&value, &Type::Number));
            assert_eq!(
                queries.table_type(&TableType {
                    fields,
                    ..Default::default()
                }),
                Err(TableError::DuplicateDeclaredField {
                    key: LuaKey::Integer(1)
                })
            );
        });
    }

    #[test]
    fn native_constructor_dynamic_candidates_do_not_hide_exact_key_collisions() {
        Engine::new().with_session(|queries| {
            let table = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(LuaKey::Integer(1), Type::Number)],
                    indexers: vec![TableIndexer::new(Type::Number, Type::String)],
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            let value = queries
                .table_lookup(&table, &literal(LuaKey::Integer(1)))
                .unwrap();
            assert!(queries.is_assignable(&Type::Number, &value));
            assert!(queries.is_assignable(&Type::String, &value));
            assert!(!queries.is_assignable(&Type::Nil, &value));
            assert!(!queries.is_assignable(&value, &Type::Number));

            let nullable_candidate = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(LuaKey::Integer(1), Type::Number)],
                    indexers: vec![TableIndexer::new(
                        Type::Number,
                        Type::Union(vec![Type::String, Type::Nil]),
                    )],
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            let maybe_absent = queries
                .table_lookup(&nullable_candidate, &literal(LuaKey::Integer(1)))
                .unwrap();
            assert!(queries.is_assignable(&Type::Number, &maybe_absent));
            assert!(queries.is_assignable(&Type::String, &maybe_absent));
            assert!(queries.is_assignable(&Type::Nil, &maybe_absent));
        });
    }

    #[test]
    fn dynamic_union_keys_join_reads_and_require_every_write_domain() {
        Engine::new().with_session(|queries| {
            let table = queries
                .table_type(&TableType {
                    indexers: vec![
                        TableIndexer::new(Type::Number, Type::String),
                        TableIndexer::new(Type::String, Type::Number),
                    ],
                    ..Default::default()
                })
                .unwrap();
            let key = TableKey::Type(Type::Union(vec![Type::Number, Type::String]));
            let value = queries.table_lookup(&table, &key).unwrap();
            assert!(queries.is_assignable(&Type::Number, &value));
            assert!(queries.is_assignable(&Type::String, &value));
            assert_eq!(queries.table_write(&table, &key, &Type::Nil), Ok(()));
            assert!(matches!(
                queries.table_write(&table, &key, &Type::Number),
                Err(TableError::ValueType { .. })
            ));
        });
    }

    #[test]
    fn indexer_reads_preserve_declared_policy_and_native_nil_deletion_rules() {
        Engine::new().with_session(|queries| {
            let table = queries
                .table_type(&dictionary(Type::String, Type::Number, false))
                .unwrap();
            let key = literal(string_key("entry"));
            assert_eq!(queries.table_lookup(&table, &key), Ok(Type::Number));
            assert_eq!(queries.table_write(&table, &key, &Type::Nil), Ok(()));
            assert_eq!(queries.table_write(&table, &key, &Type::Number), Ok(()));
            assert_eq!(
                queries.table_write(&table, &key, &Type::String),
                Err(TableError::ValueType {
                    value: Type::String,
                    expected: Type::Number,
                })
            );
            let sparse = queries
                .table_type(&dictionary(Type::String, Type::Number, true))
                .unwrap();
            let sparse_read = queries.table_lookup(&sparse, &key).unwrap();
            assert!(queries.is_assignable(&Type::Nil, &sparse_read));
            assert!(queries.is_assignable(&Type::Number, &sparse_read));
            assert!(!queries.is_assignable(&sparse_read, &Type::Number));

            let required = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(string_key("entry"), Type::Number)],
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(
                queries.table_write(&required, &key, &Type::Nil),
                Err(TableError::RequiredDeletion {
                    key: string_key("entry")
                })
            );
            let optional = queries
                .table_type(&TableType {
                    fields: vec![TableField {
                        optional: true,
                        ..TableField::new(string_key("entry"), Type::Number)
                    }],
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(queries.table_write(&optional, &key, &Type::Nil), Ok(()));
            let optional_read = queries.table_lookup(&optional, &key).unwrap();
            assert!(queries.is_assignable(&Type::Nil, &optional_read));

            let explicitly_nullable = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(
                        string_key("entry"),
                        Type::Union(vec![Type::Number, Type::Nil]),
                    )],
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(
                queries.table_write(&explicitly_nullable, &key, &Type::Nil),
                Ok(())
            );
        });
    }

    #[test]
    fn lua_array_annotations_are_numeric_tables_without_js_properties_or_key_coercion() {
        Engine::new().with_session(|queries| {
            let array = queries.array_type(&Type::Number);
            let dictionary = queries
                .table_type(&dictionary(Type::Number, Type::Number, false))
                .unwrap();
            assert!(queries.is_assignable(&array, &dictionary));
            assert!(queries.is_assignable(&dictionary, &array));
            assert_eq!(
                queries.table_lookup(&array, &literal(LuaKey::Integer(2))),
                Ok(Type::Number)
            );
            assert_eq!(
                queries.table_lookup(&array, &literal(LuaKey::Number(2.0))),
                Ok(Type::Number)
            );
            for key in [string_key("2"), string_key("length"), string_key("map")] {
                assert!(queries.table_lookup(&array, &literal(key)).is_err());
            }
            let sequence = queries
                .table_type(&TableType {
                    fields: vec![
                        TableField::new(LuaKey::Integer(1), Type::Number),
                        TableField::new(LuaKey::Integer(3), Type::Number),
                    ],
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            assert!(queries.is_assignable(&sequence, &array));
        });
    }

    #[test]
    fn opaque_kernel_arrays_and_tuples_do_not_expose_js_members_through_lua_lookup() {
        use crate::{PackAlternative, PackFunctionType, PackSlot, ValuePack};
        Engine::new().with_session(|queries| {
            for rest in [
                ValuePack::variadic(Type::Number),
                ValuePack::Alternatives(vec![PackAlternative {
                    fixed: vec![
                        PackSlot::required(Type::Number),
                        PackSlot::required(Type::Number),
                    ],
                    tail: None,
                }]),
            ] {
                let function = queries.pack_function(&PackFunctionType {
                    params: Vec::new(),
                    rest: Some(rest),
                    type_parameters: Vec::new(),
                    returns: ValuePack::single(Type::Nil),
                });
                let signature = queries.function_signature(&function).unwrap();
                let kernel = &signature.params[0].ty;
                assert!(matches!(kernel, Type::SessionType(_)));
                for key in [
                    literal(string_key("length")),
                    literal(string_key("map")),
                    literal(LuaKey::Integer(1)),
                ] {
                    assert!(matches!(
                        queries.table_lookup(kernel, &key),
                        Err(TableError::NotTable { .. })
                    ));
                    assert!(matches!(
                        queries.table_write(kernel, &key, &Type::Number),
                        Err(TableError::NotTable { .. })
                    ));
                    assert!(queries.table_value_context(kernel, &key).is_none());
                }
                let called = queries.call_pack(&function, &[Type::Number, Type::Number], &[], None);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
            }
        });
    }

    #[test]
    fn boolean_reference_and_callable_indexer_domains_use_native_relations() {
        Engine::new().with_session(|queries| {
            let boolean = queries
                .table_type(&dictionary(Type::Boolean, Type::String, false))
                .unwrap();
            for key in [true, false] {
                assert_eq!(
                    queries.table_lookup(&boolean, &literal(LuaKey::Boolean(key))),
                    Ok(Type::String)
                );
            }
            assert!(matches!(
                queries.table_lookup(&boolean, &literal(string_key("true"))),
                Err(TableError::KeyType { .. })
            ));
            let shape = Type::Table(TableType {
                fields: vec![TableField::new(string_key("id"), Type::Number)],
                ..Default::default()
            });
            let objects = queries
                .table_type(&dictionary(shape.clone(), Type::String, false))
                .unwrap();
            assert_eq!(
                queries.table_lookup(&objects, &TableKey::Type(shape)),
                Ok(Type::String)
            );
            let callable = Type::Function(crate::FunctionType {
                params: vec![],
                returns: Box::new(Type::Number),
            });
            let functions = queries
                .table_type(&dictionary(callable.clone(), Type::Boolean, false))
                .unwrap();
            assert_eq!(
                queries.table_lookup(&functions, &TableKey::Type(callable)),
                Ok(Type::Boolean)
            );
        });
    }

    #[test]
    fn readonly_tables_reject_writes_and_freshness_survives_owned_roundtrips() {
        Engine::new().with_session(|queries| {
            let key = literal(string_key("id"));
            let readonly = queries
                .table_type(&TableType {
                    fields: vec![TableField {
                        readonly: true,
                        ..TableField::new(string_key("id"), Type::Number)
                    }],
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(
                queries.table_write(&readonly, &key, &Type::Number),
                Err(TableError::ReadOnly { key: key.clone() })
            );
            let readonly_indexer = queries
                .table_type(&TableType {
                    indexers: vec![TableIndexer {
                        readonly: true,
                        ..TableIndexer::new(Type::String, Type::Number)
                    }],
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(
                queries.table_write(&readonly_indexer, &key, &Type::Nil),
                Err(TableError::ReadOnly { key })
            );
            let target = Type::Table(TableType {
                fields: vec![TableField::new(string_key("id"), Type::Number)],
                ..Default::default()
            });
            let fresh = queries
                .table_type(&TableType {
                    fields: vec![
                        TableField::new(string_key("id"), Type::NumberLiteral(1.0)),
                        TableField::new(string_key("extra"), Type::BooleanLiteral(true)),
                    ],
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            assert!(!queries.is_assignable(&fresh, &target));
            let stored = queries.widen(&fresh);
            assert!(queries.is_assignable(&stored, &target));
            assert_eq!(
                queries.table_lookup(&stored, &literal(string_key("id"))),
                Ok(Type::Number)
            );
        });
    }

    #[test]
    fn mixed_legacy_shapes_preserve_native_key_domains_and_fresh_excess_fields() {
        Engine::new().with_session(|queries| {
            let legacy = Type::Shape(vec![crate::Property::new("x", Type::Number)]);
            let native = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(string_key("x"), Type::Number)],
                    ..Default::default()
                })
                .unwrap();
            assert!(queries.is_assignable(&legacy, &native));
            assert!(queries.is_assignable(&native, &legacy));
            for key in [LuaKey::Integer(1), LuaKey::Boolean(true)] {
                let required = queries
                    .table_type(&TableType {
                        fields: vec![TableField::new(key, Type::Number)],
                        ..Default::default()
                    })
                    .unwrap();
                assert!(!queries.is_assignable(&legacy, &required));
            }
            let fresh = queries
                .table_type(&TableType {
                    fields: vec![
                        TableField::new(string_key("x"), Type::Number),
                        TableField::new(LuaKey::Integer(1), Type::Number),
                        TableField::new(LuaKey::Boolean(true), Type::Number),
                    ],
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            assert!(!queries.is_assignable(&fresh, &legacy));
            let stored = queries.widen(&fresh);
            assert!(queries.is_assignable(&stored, &legacy));
            let numeric = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(LuaKey::Integer(1), Type::Number)],
                    ..Default::default()
                })
                .unwrap();
            let named = Type::Shape(vec![crate::Property::new("1", Type::Number)]);
            assert!(!queries.is_assignable(&numeric, &named));
            assert!(!queries.is_assignable(&named, &numeric));
        });
    }

    #[test]
    fn weak_tables_compare_exact_keys_without_treating_numeric_fields_as_empty() {
        Engine::new().with_session(|queries| {
            let source = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(LuaKey::Integer(1), Type::Number)],
                    ..Default::default()
                })
                .unwrap();
            for (key, accepts) in [
                (LuaKey::Integer(1), true),
                (LuaKey::Integer(2), false),
                (string_key("1"), false),
            ] {
                let target = queries
                    .table_type(&TableType {
                        fields: vec![TableField {
                            optional: true,
                            ..TableField::new(key, Type::Number)
                        }],
                        ..Default::default()
                    })
                    .unwrap();
                assert_eq!(queries.is_assignable(&source, &target), accepts);
                let empty = queries.table_type(&TableType::default()).unwrap();
                assert!(queries.is_assignable(&empty, &target));
            }
            let legacy = Type::Shape(vec![crate::Property {
                optional: true,
                ..crate::Property::new("1", Type::Number)
            }]);
            assert!(!queries.is_assignable(&source, &legacy));
        });
    }

    #[test]
    fn contextual_union_fields_use_matching_members_while_reads_require_every_member() {
        Engine::new().with_session(|queries| {
            let first = Type::Table(TableType {
                fields: vec![TableField::new(
                    LuaKey::Integer(1),
                    Type::BooleanLiteral(true),
                )],
                ..Default::default()
            });
            let second = Type::Table(TableType {
                fields: vec![TableField::new(
                    LuaKey::Integer(2),
                    Type::StringLiteral(b"other".to_vec()),
                )],
                ..Default::default()
            });
            let expected = Type::Union(vec![first, second]);
            let key = literal(LuaKey::Integer(1));
            assert!(queries.table_lookup(&expected, &key).is_err());
            let context = queries.table_value_context(&expected, &key).unwrap();
            assert_eq!(queries.inspect_type(&context), Type::BooleanLiteral(true));
            let actual = queries
                .table_type_in_context(
                    &TableType {
                        fields: vec![TableField::new(
                            LuaKey::Integer(1),
                            Type::BooleanLiteral(true),
                        )],
                        fresh: true,
                        ..Default::default()
                    },
                    &expected,
                )
                .unwrap();
            assert!(queries.is_assignable(&actual, &expected));
            assert_eq!(
                queries.table_lookup(&actual, &key),
                Ok(Type::BooleanLiteral(true))
            );
        });
    }

    #[test]
    fn contextual_unit_fields_survive_exact_description_export_and_import() {
        Engine::new().with_session(|queries| {
            let expected = Type::Table(TableType {
                fields: vec![
                    TableField::new(string_key("flag"), Type::BooleanLiteral(true)),
                    TableField::new(string_key("count"), Type::NumberLiteral(3.0)),
                    TableField::new(string_key("kind"), Type::StringLiteral(b"entry".to_vec())),
                ],
                ..Default::default()
            });
            let recipe = TableType {
                fields: vec![
                    TableField::new(string_key("flag"), Type::BooleanLiteral(true)),
                    TableField::new(string_key("count"), Type::NumberLiteral(3.0)),
                    TableField::new(string_key("kind"), Type::StringLiteral(b"entry".to_vec())),
                ],
                fresh: true,
                ..Default::default()
            };
            let matching = queries.table_type_in_context(&recipe, &expected).unwrap();
            assert!(matches!(matching, Type::Table(_)));
            assert!(queries.is_assignable(&matching, &expected));
            let inspected = queries.inspect_type(&matching);
            assert!(queries.is_assignable(&inspected, &expected));
            assert_eq!(
                queries.table_lookup(&inspected, &literal(string_key("flag"))),
                Ok(Type::BooleanLiteral(true))
            );
            let uncontextual = queries.table_type(&recipe).unwrap();
            assert!(!queries.is_assignable(&uncontextual, &expected));

            let mismatching = queries
                .table_type_in_context(
                    &TableType {
                        fields: vec![
                            TableField::new(string_key("flag"), Type::BooleanLiteral(false)),
                            recipe.fields[1].clone(),
                            recipe.fields[2].clone(),
                        ],
                        ..recipe
                    },
                    &expected,
                )
                .unwrap();
            assert_eq!(
                queries.table_lookup(&mismatching, &literal(string_key("flag"))),
                Ok(Type::BooleanLiteral(false))
            );
            assert!(!queries.is_assignable(&mismatching, &expected));
        });
    }

    #[test]
    fn sparse_constructor_entries_check_values_without_requiring_every_index_to_exist() {
        Engine::new().with_session(|queries| {
            let target = Type::Table(dictionary(Type::String, Type::Number, false));
            let constructor = queries
                .table_type(&TableType {
                    fresh: true,
                    ..dictionary(Type::String, Type::Number, false)
                })
                .unwrap();
            let value = queries
                .table_lookup(&constructor, &literal(string_key("entry")))
                .unwrap();
            assert!(queries.is_assignable(&Type::Nil, &value));
            assert!(queries.is_assignable(&constructor, &target));
            let wrong_values = queries
                .table_type(&TableType {
                    fresh: true,
                    ..dictionary(Type::String, Type::String, false)
                })
                .unwrap();
            assert!(!queries.is_assignable(&wrong_values, &target));
            let deletion_only = queries
                .table_type(&TableType {
                    fresh: true,
                    ..dictionary(Type::String, Type::Nil, false)
                })
                .unwrap();
            assert!(queries.is_assignable(&deletion_only, &target));
            let explicitly_nullable = queries
                .table_type(&dictionary(
                    Type::String,
                    Type::Union(vec![Type::Number, Type::Nil]),
                    false,
                ))
                .unwrap();
            assert!(!queries.is_assignable(&explicitly_nullable, &target));
            let optional_field = queries
                .table_type(&TableType {
                    fields: vec![TableField {
                        optional: true,
                        ..TableField::new(string_key("entry"), Type::Number)
                    }],
                    ..Default::default()
                })
                .unwrap();
            assert!(queries.is_assignable(&optional_field, &target));
        });
    }

    #[derive(Default)]
    struct GenericDictionary;

    impl GenericDictionary {
        fn function() -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 8181,
                index: 0,
            }
        }

        fn parameter() -> NativeTypeParameterKey {
            NativeTypeParameterKey {
                snapshot: 8181,
                index: 0,
            }
        }
    }

    impl NativeTypeProvider for GenericDictionary {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key == Self::function()
        }

        fn function_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            vec![Parameter {
                name: "key".to_owned(),
                ty: Type::NativeTypeParameter(Self::parameter()),
                optional: false,
            }]
        }

        fn function_declared_return(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            let parameter = Type::NativeTypeParameter(Self::parameter());
            Some(Type::Table(dictionary(parameter.clone(), parameter, false)))
        }

        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            panic!("written generic return avoids the body demand")
        }

        fn function_type_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<NativeTypeParameterKey> {
            vec![Self::parameter()]
        }

        fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
            key == Self::parameter()
        }

        fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
            "K".to_owned()
        }

        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("acyclic generic dictionary")
        }

        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("shallow generic dictionary")
        }
    }

    #[test]
    fn generic_dictionary_key_and_value_mappers_roundtrip_without_raw_type_ids() {
        Engine::new().with_native_provider(GenericDictionary::default, |_, queries| {
            let function = Type::NativeFunction(GenericDictionary::function());
            for (argument, key) in [
                (Type::String, literal(string_key("value"))),
                (Type::Number, literal(LuaKey::Integer(1))),
                (Type::Boolean, literal(LuaKey::Boolean(true))),
            ] {
                let called = queries.call(&function, &[argument.clone()]);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
                assert_eq!(
                    queries.table_lookup(&called.return_type, &key),
                    Ok(argument)
                );
            }
        });
    }

    #[test]
    fn invalid_instantiated_key_domains_remain_exact_and_fail_without_reconstruction_panics() {
        Engine::new().with_native_provider(GenericDictionary::default, |_, queries| {
            let function = Type::NativeFunction(GenericDictionary::function());
            let called = queries.call(&function, &[Type::Nil]);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(
                queries.table_lookup(&called.return_type, &literal(string_key("value"))),
                Err(TableError::InvalidKey {
                    key: TableKey::Type(Type::Nil)
                })
            );
            assert!(matches!(called.return_type, Type::SessionType(_)));
        });
    }

    #[derive(Default)]
    struct GenericTablePattern;

    impl GenericTablePattern {
        fn function() -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 8183,
                index: 0,
            }
        }

        fn string_function() -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 8183,
                index: 1,
            }
        }

        fn parameter(index: u32) -> NativeTypeParameterKey {
            NativeTypeParameterKey {
                snapshot: 8183,
                index,
            }
        }

        fn pattern() -> Type {
            Type::Table(dictionary(
                Type::NativeTypeParameter(Self::parameter(0)),
                Type::NativeTypeParameter(Self::parameter(1)),
                false,
            ))
        }

        fn pattern_for(function: NativeFunctionKey) -> Type {
            if function == Self::string_function() {
                Type::Table(dictionary(
                    Type::String,
                    Type::NativeTypeParameter(Self::parameter(1)),
                    false,
                ))
            } else {
                Self::pattern()
            }
        }
    }

    impl NativeTypeProvider for GenericTablePattern {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key == Self::function() || key == Self::string_function()
        }

        fn function_parameters(
            &self,
            function: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            vec![Parameter {
                name: "table".to_owned(),
                ty: Self::pattern_for(function),
                optional: false,
            }]
        }

        fn function_declared_return(
            &self,
            function: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            Some(Self::pattern_for(function))
        }

        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            panic!("written dictionary pattern return avoids body demand")
        }

        fn function_type_parameters(
            &self,
            function: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<NativeTypeParameterKey> {
            if function == Self::string_function() {
                vec![Self::parameter(1)]
            } else {
                vec![Self::parameter(0), Self::parameter(1)]
            }
        }

        fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
            key.snapshot == 8183 && key.index < 2
        }

        fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
            if key.index == 0 { "K" } else { "V" }.to_owned()
        }

        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("acyclic dictionary pattern")
        }

        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("shallow dictionary pattern")
        }
    }

    #[test]
    fn native_inference_reads_both_dictionary_domains_and_constructor_fields() {
        Engine::new().with_native_provider(GenericTablePattern::default, |_, queries| {
            let function = Type::NativeFunction(GenericTablePattern::function());
            for (domain, value, key) in [
                (Type::String, Type::Number, literal(string_key("value"))),
                (Type::Number, Type::String, literal(LuaKey::Integer(1))),
                (Type::Boolean, Type::Number, literal(LuaKey::Boolean(true))),
            ] {
                let argument = queries
                    .table_type(&dictionary(domain, value.clone(), false))
                    .unwrap();
                let called = queries.call(&function, &[argument]);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
                assert_eq!(queries.table_lookup(&called.return_type, &key), Ok(value));
            }
            let constructor = queries
                .table_type(&TableType {
                    fields: vec![
                        TableField::new(LuaKey::Integer(1), Type::String),
                        TableField::new(LuaKey::Integer(3), Type::String),
                    ],
                    fresh: true,
                    ..Default::default()
                })
                .unwrap();
            let called = queries.call(&function, &[constructor]);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(
                queries.table_lookup(&called.return_type, &literal(LuaKey::Integer(1))),
                Ok(Type::String)
            );
            let dynamic = queries
                .table_type(&TableType {
                    fresh: true,
                    ..dictionary(Type::String, Type::Number, false)
                })
                .unwrap();
            let called = queries.call(&function, &[dynamic]);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(
                queries.table_lookup(&called.return_type, &literal(string_key("value"))),
                Ok(Type::Number)
            );
        });
    }

    #[test]
    fn known_key_domains_do_not_infer_values_from_unrelated_entries() {
        Engine::new().with_native_provider(GenericTablePattern::default, |_, queries| {
            let function = Type::NativeFunction(GenericTablePattern::string_function());
            for argument in [
                TableType {
                    indexers: vec![
                        TableIndexer::new(Type::String, Type::String),
                        TableIndexer::new(Type::Number, Type::Boolean),
                    ],
                    ..Default::default()
                },
                TableType {
                    fields: vec![
                        TableField::new(string_key("value"), Type::String),
                        TableField::new(LuaKey::Integer(1), Type::Boolean),
                    ],
                    ..Default::default()
                },
            ] {
                let argument = queries.table_type(&argument).unwrap();
                let called = queries.call(&function, &[argument]);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
                assert_eq!(
                    queries.table_lookup(&called.return_type, &literal(string_key("value"))),
                    Ok(Type::String)
                );
            }
        });
    }

    #[derive(Default)]
    struct LazyContext {
        body_demands: std::cell::Cell<usize>,
    }

    impl LazyContext {
        fn function() -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 8182,
                index: 0,
            }
        }
    }

    impl NativeTypeProvider for LazyContext {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key == Self::function()
        }

        fn function_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            Vec::new()
        }

        fn function_declared_return(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            None
        }

        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            self.body_demands.set(self.body_demands.get() + 1);
            Type::Number
        }

        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("acyclic table callback")
        }

        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("shallow table callback")
        }
    }

    #[test]
    fn table_callback_context_is_an_exact_capability_without_eager_body_demands() {
        Engine::new().with_native_provider(LazyContext::default, |provider, queries| {
            let table = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(
                        string_key("callback"),
                        Type::NativeFunction(LazyContext::function()),
                    )],
                    ..Default::default()
                })
                .unwrap();
            let context = queries
                .table_value_context(&table, &literal(string_key("callback")))
                .unwrap();
            assert!(matches!(context, Type::SessionType(_)));
            assert_eq!(provider.body_demands.get(), 0);
            let signature = queries.function_signature(&context).unwrap();
            assert_eq!(*signature.returns, Type::Number);
            assert_eq!(provider.body_demands.get(), 1);
            assert_eq!(queries.call(&context, &[]).return_type, Type::Number);
            assert_eq!(provider.body_demands.get(), 1);
        });
    }
}
