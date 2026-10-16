//! Original Lua table keys and native table construction/query adaptation.
//! Literal evaluation belongs to Lua syntax; normalization, relations, joins,
//! indexer checking and duplicate-constructor policy remain native operations.

use crate::Checker;
use crate::native_packs::AnnotationPack;
use luanalysis_engine::{
    LuaKey, NativeEnvironmentKey, TableError, TableField as NativeField, TableFieldOrigin, TableIndexer, TableKey, TableType, Type,
};
use luanalysis_syntax::{
    Expr, ExprKind, Span, TableField, TableFieldKind, TypeExpr, TypeExprKind, TypeField,
    TypeFieldKey, TypeLiteral, UnaryOperator,
};

impl Checker<'_> {
    pub(super) fn native_member_flow_type(&mut self, expression: &Expr, declared: Type, receiver_environment: Option<NativeEnvironmentKey>) -> Type {
        if let (Some(native), Some(at)) = (self.native, self.flow_at) {
            // The caller captured this before any computed-key evaluation.
            if receiver_environment.is_some()
                && let Some(result) = native.flow_environment_member_type(expression, at, &declared, self.engine) {
                self.incomplete_flow |= result.incomplete;
                return result.ty;
            }
            let result =
                native.flow_reference_type(expression, at, &declared, &declared, self.engine);
            self.incomplete_flow |= result.incomplete;
            result.ty
        } else {
            self.reference(expression)
                .and_then(|reference| self.assigned_fields.get(&reference).cloned())
                .unwrap_or(declared)
        }
    }

    pub(super) fn native_table_annotation(&mut self, fields: &[TypeField], span: Span) -> Type {
        let mut table = TableType::default();
        let mut origins = Vec::with_capacity(fields.len());
        for field in fields {
            let value = self.annotation(&field.ty);
            let (key, origin) = match &field.key {
                TypeFieldKey::Named(name) => (
                    TableKey::Literal(LuaKey::String(name.text.as_bytes().to_vec())),
                    name.span,
                ),
                TypeFieldKey::Sequence { index } => (sequence_key(*index), field.span),
                TypeFieldKey::Indexed(key) => {
                    let descriptor = literal_type_key(key)
                        .map_or_else(|| TableKey::Type(self.annotation(key)), TableKey::Literal);
                    (descriptor, key.span)
                }
            };
            match &key {
                TableKey::Literal(key) => table.fields.push(NativeField::new(key.clone(), value)),
                TableKey::Type(key) => table.indexers.push(TableIndexer::new(key.clone(), value)),
            }
            origins.push((key, origin));
        }
        self.finish_native_table(table, &origins, span, None)
    }

    pub(super) fn native_dictionary_annotation(&mut self, args: &[TypeExpr], span: Span) -> Type {
        let [key, value] = args else {
            self.error(
                "LUA2003",
                span,
                "table<K,V> requires exactly two type arguments",
            );
            return Type::Unknown;
        };
        let key = self.annotation(key);
        let value = self.annotation(value);
        let origins = [(TableKey::Type(key.clone()), args[0].span)];
        self.finish_native_table(
            TableType {
                indexers: vec![TableIndexer::new(key, value)],
                ..TableType::default()
            },
            &origins,
            span,
            None,
        )
    }

    pub(super) fn native_array_annotation(&mut self, element: &TypeExpr) -> Type {
        let element = self.annotation(element);
        self.engine.array_type(&element)
    }

    pub(super) fn native_table_expression(
        &mut self,
        fields: &[TableField],
        span: Span,
        expected: Option<&Type>,
    ) -> Type {
        let mut table = TableType {
            fresh: true,
            ..TableType::default()
        };
        let mut origins = Vec::with_capacity(fields.len());
        for (position, field) in fields.iter().enumerate() {
            let (key, origin) = match &field.kind {
                TableFieldKind::Named(name) => (
                    TableKey::Literal(LuaKey::String(name.text.as_bytes().to_vec())),
                    name.span,
                ),
                TableFieldKind::Sequence { index } => (sequence_key(*index), field.value.span),
                TableFieldKind::Keyed(key) => (self.native_table_key(key), key.span),
            };
            let written = field.annotation.as_ref().map(|annotation| {
                AnnotationPack::annotation(annotation).lower(&mut |ty| self.annotation(ty))
            });
            if position + 1 == fields.len()
                && matches!(field.kind, TableFieldKind::Sequence { .. })
                && matches!(
                    field.value.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Vararg
                )
            {
                let TableFieldKind::Sequence { index } = field.kind else {
                    unreachable!()
                };
                if let Some(native) = self.native {
                    let recorded = native
                        .packs()
                        .table_sequence(span)
                        .expect("an original constructor has a sequence recipe")
                        .elements
                        .last()
                        .expect("the final sequence field has a recorded element");
                    debug_assert_eq!(recorded.index, index);
                    debug_assert_eq!(recorded.expression.span, field.value.span);
                    debug_assert_eq!(
                        recorded.expression.adjustment,
                        crate::native_packs::Adjustment::Expand
                    );
                }
                let start =
                    i64::try_from(index).expect("source sequence index fits Lua integer domain");
                let inferred_context = expected
                    .and_then(|expected| self.engine.table_sequence_context(expected, start));
                let context = written.as_ref().or(inferred_context.as_ref());
                let actual = self.expression_pack(&field.value, context);
                let pack = if let Some(written) = &written {
                    if !self.engine.pack_assignable(&actual, written) {
                        self.error("TS2322", field.value.span,
                            "Lua table field result pack is not assignable to its declared result pack.");
                    }
                    self.engine.return_pack_with_cardinality(written, &actual)
                } else { actual };
                let origin = if written.is_some() { TableFieldOrigin::Declared } else { TableFieldOrigin::Inferred };
                return match self
                    .engine
                    .table_with_sequence_origin(&table, start, &pack, origin, expected)
                {
                    Ok(ty) => ty,
                    Err(error) => {
                        self.native_table_error(error, field.value.span, field.value.span);
                        Type::Unknown
                    }
                };
            }
            let context =
                expected.and_then(|expected| self.engine.table_value_context(expected, &key));
            let declared = written.as_ref().map(|written| self.engine.pack_slot(written, 0));
            if let (Some(declared), Some(context), Some(annotation)) = (&declared, &context, &field.annotation) {
                self.compatible(declared, context, annotation.span);
            }
            let actual = self.expression_with_expected(&field.value, declared.as_ref().or(context.as_ref()));
            if let Some(declared) = &declared {
                self.compatible(&actual, declared, field.value.span);
            }
            // Luanalysis infers field literals, unlike mutable scalar locals.
            // Use the native regular-literal operation without declaring the
            // field or making the table readonly. Written bounds stay ahead
            // of the initializer's value as before.
            let value = declared.unwrap_or_else(|| self.engine.regular_type(&actual));
            match &key {
                TableKey::Literal(key) => table.fields.push(if written.is_some() {
                    NativeField::declared(key.clone(), value)
                } else { NativeField::new(key.clone(), value) }),
                TableKey::Type(key) => {
                    let mut indexer = if written.is_some() { TableIndexer::declared(key.clone(), value) }
                        else { TableIndexer::new(key.clone(), value) };
                    indexer.sparse = true;
                    table.indexers.push(indexer);
                }
            }
            origins.push((key, origin));
        }
        self.finish_native_table(table, &origins, span, expected)
    }

    /// Evaluate the original key before its value, including calls/captures,
    /// while retaining exact static key identity outside ordinary f64 types.
    pub(super) fn native_table_key(&mut self, expression: &Expr) -> TableKey {
        let ty = self.expression(expression);
        literal_key(expression).map_or(TableKey::Type(ty), TableKey::Literal)
    }

    pub(super) fn native_table_read(
        &mut self,
        receiver: &Type,
        key: &TableKey,
        span: Span,
    ) -> Type {
        match self.engine.table_lookup(receiver, key) {
            Ok(ty) => ty,
            Err(TableError::PendingContext) => {
                self.incomplete_flow = true;
                self.engine.error_type()
            }
            Err(error) => {
                self.native_table_error_with_owner(error, span, span, Some(receiver));
                Type::Unknown
            }
        }
    }

    pub(super) fn native_named_property_read(
        &mut self,
        receiver: &Type,
        name: &luanalysis_syntax::Name,
        receiver_span: Span,
    ) -> Type {
        let key = TableKey::Literal(LuaKey::String(name.text.as_bytes().to_vec()));
        match self.engine.table_lookup(receiver, &key) {
            Ok(ty) => ty,
            Err(TableError::PendingContext) => {
                self.incomplete_flow = true;
                self.engine.error_type()
            }
            Err(
                TableError::KeyType { .. }
                | TableError::NotTable { .. }
                | TableError::MissingKey { .. },
            ) => {
                self.error_types(
                    "TS2339",
                    name.span,
                    format!(
                        "Property '{}' does not exist on type '{{}}'.",
                        name.text
                    ),
                    &[receiver],
                );
                Type::Unknown
            }
            Err(error) => {
                self.native_table_error(error, name.span, receiver_span);
                Type::Unknown
            }
        }
    }

    /// The caller supplies the frozen declared receiver/key and the original
    /// RHS span. Native writes own assignability and the Lua deletion policy.
    pub(super) fn native_table_write(
        &mut self,
        receiver: &Type,
        key: &TableKey,
        value: &Type,
        target_span: Span,
        key_span: Span,
        value_span: Span,
    ) -> bool {
        let result = if matches!(key, TableKey::Type(_))
            && let (Some(native), Some(input)) = (self.native, self.flow_at)
            && let Some(store) = native.runtime_tables.store_for_target(target_span)
            && store.key.graph.region == input.graph.region
        {
            let at = luanalysis_engine::NativeFlowKey { graph: input.graph, node: store.key.node };
            self.engine.table_write_at_flow(&crate::native_flow::FlowAdapter { sources: native.iterator_sources(), values: native }, at, receiver, key, value)
        } else { self.engine.table_write(receiver, key, value) };
        match result {
            Ok(()) => true,
            Err(error) => {
                self.native_table_error_with_owner(error, key_span, value_span, Some(receiver));
                false
            }
        }
    }

    fn finish_native_table(
        &mut self,
        table: TableType,
        origins: &[(TableKey, Span)],
        span: Span,
        expected: Option<&Type>,
    ) -> Type {
        let constructed = match expected {
            Some(expected) => self.engine.table_type_in_context(&table, expected),
            None => self.engine.table_type(&table),
        };
        match constructed {
            Ok(ty) => ty,
            Err(error) => {
                let key = match &error {
                    TableError::InvalidKey { key }
                    | TableError::MissingKey { key }
                    | TableError::ReadOnly { key } => Some(key.clone()),
                    TableError::DuplicateDeclaredField { key }
                    | TableError::RequiredDeletion { key } => Some(TableKey::Literal(key.clone())),
                    _ => None,
                };
                let origin = key
                    .and_then(|key| {
                        origins
                            .iter()
                            .find(|(candidate, _)| *candidate == key)
                            .map(|(_, span)| *span)
                    })
                    .unwrap_or(span);
                self.native_table_error(error, origin, span);
                Type::Unknown
            }
        }
    }

    pub(super) fn native_table_error(&mut self, error: TableError, key_span: Span, value_span: Span) {
        self.native_table_error_with_owner(error, key_span, value_span, None);
    }

    fn native_table_error_with_owner(&mut self, error: TableError, key_span: Span, value_span: Span, owner: Option<&Type>) {
        if matches!(error, TableError::PendingContext) {
            self.incomplete_flow = true;
            return;
        }
        let (code, origin) = match &error {
            TableError::PendingContext => unreachable!("pending member context handled above"),
            TableError::InvalidKey { .. } | TableError::KeyType { .. } => ("TS2538", key_span),
            TableError::NotTable { .. } => ("TS7053", key_span),
            TableError::MissingKey { .. } => ("TS2339", key_span),
            TableError::ValueType { .. } | TableError::RequiredDeletion { .. } => ("TS2322", value_span),
            TableError::ReadOnly { .. } => ("TS2540", key_span),
            TableError::DuplicateDeclaredField { .. } => ("TS2300", key_span),
            TableError::InvalidSequenceStart { .. } => ("LUA2002", value_span),
            TableError::SequenceTooComplex { .. } => ("LUA2019", value_span),
            TableError::UnresolvedSequence => ("LUA2016", value_span),
        };
        let (template, types) = table_error_description_with_owner(&error, owner);
        self.error_types(code, origin, template, &types.iter().collect::<Vec<_>>());
    }
}

/// Build diagnostic text without querying or formatting native type operands.
pub(crate) fn table_error_description(error: &TableError) -> (String, Vec<Type>) {
    table_error_description_with_owner(error, None)
}

fn table_error_description_with_owner(error: &TableError, owner: Option<&Type>) -> (String, Vec<Type>) {
    fn key(key: &TableKey, types: &mut Vec<Type>) -> String {
        match key {
            TableKey::Literal(LuaKey::String(bytes)) => {
                types.push(Type::StringLiteral(bytes.clone()));
                "{}".into()
            }
            TableKey::Literal(LuaKey::Integer(value)) => value.to_string(),
            TableKey::Literal(LuaKey::Number(value)) => value.to_string(),
            TableKey::Literal(LuaKey::Boolean(value)) => value.to_string(),
            TableKey::Type(Type::Nil | Type::Undefined) => "nil".into(),
            TableKey::Type(ty) => { types.push(ty.clone()); "{}".into() }
        }
    }
    let mut types = Vec::new();
    let template = match error {
        TableError::PendingContext => "Lua table member context remains unresolved.".into(),
        TableError::InvalidKey { key: value } => format!("{} cannot be used as a Lua table key.", key(value, &mut types)),
        TableError::NotTable { ty } => {
            types.push(ty.clone());
            "Type '{}' cannot be indexed as a Lua table.".into()
        }
        TableError::MissingKey { key: value } => {
            let key = key(value, &mut types);
            if let Some(owner) = owner {
                types.push(owner.clone());
                format!("Property {key} does not exist on type '{{}}'.")
            } else {
                format!("Property {key} does not exist on this table type.")
            }
        }
        TableError::KeyType { key, expected } => {
            types.extend([key.clone(), expected.clone()]);
            "Key type '{}' is not assignable to index type '{}'.".into()
        }
        TableError::ValueType { value, expected } => {
            types.extend([value.clone(), expected.clone()]);
            "Type '{}' is not assignable to type '{}'.".into()
        }
        TableError::ReadOnly { key: value } => format!("Cannot assign to readonly property {}.", key(value, &mut types)),
        TableError::RequiredDeletion { key: value } => format!("Nil cannot delete required property {}.", key(&TableKey::Literal(value.clone()), &mut types)),
        TableError::DuplicateDeclaredField { key: value } => format!("Duplicate declared property {}.", key(&TableKey::Literal(value.clone()), &mut types)),
        TableError::InvalidSequenceStart { start } => format!("Invalid Lua sequence starting index {start}."),
        TableError::SequenceTooComplex { count, alternatives } => format!("Lua table result sequence exceeds native complexity limits ({count} fields, {alternatives} alternatives)."),
        TableError::UnresolvedSequence => "Lua table result sequence could not be resolved from its native pack.".into(),
    };
    (template, types)
}

fn sequence_key(index: usize) -> TableKey {
    TableKey::Literal(LuaKey::Integer(
        i64::try_from(index).expect("an original field ordinal fits the Lua integer domain"),
    ))
}

pub(super) fn literal_type_key(ty: &TypeExpr) -> Option<LuaKey> {
    match &ty.kind {
        TypeExprKind::Literal(TypeLiteral::Number(spelling)) => numeric_key(spelling),
        TypeExprKind::Literal(TypeLiteral::String(bytes)) => Some(LuaKey::String(bytes.clone())),
        TypeExprKind::Literal(TypeLiteral::Boolean(value)) => Some(LuaKey::Boolean(*value)),
        TypeExprKind::Parenthesized(inner) => literal_type_key(inner),
        _ => None,
    }
}

pub(super) fn describe_table(table: &TableType, mut render: impl FnMut(&Type) -> String) -> String {
    if table.fields.is_empty() && table.indexers.len() == 1 && table.sequence_tails.is_empty() {
        let indexer = &table.indexers[0];
        let key = render(&indexer.key);
        let value = render(&indexer.value);
        return format!(
            "table<{key}, {}{value}>",
            if indexer.sparse { "nil | " } else { "" }
        );
    }
    let mut members =
        Vec::with_capacity(table.fields.len() + table.indexers.len() + table.sequence_tails.len());
    for field in &table.fields {
        let key = match &field.key {
            LuaKey::String(bytes) if named_key(bytes) => {
                String::from_utf8(bytes.clone()).expect("ASCII name")
            }
            LuaKey::String(bytes) => {
                format!("[{}]", crate::describe(&Type::StringLiteral(bytes.clone())))
            }
            LuaKey::Integer(value) => format!("[{value}]"),
            LuaKey::Number(value) => format!("[{value}]"),
            LuaKey::Boolean(value) => format!("[{value}]"),
        };
        members.push(format!(
            "{key}{}: {}",
            if field.optional { "?" } else { "" },
            render(&field.value)
        ));
    }
    for indexer in &table.indexers {
        let key = render(&indexer.key);
        let value = render(&indexer.value);
        members.push(format!(
            "[{key}]: {}{value}",
            if indexer.sparse { "nil | " } else { "" }
        ));
    }
    for tail in &table.sequence_tails {
        members.push(format!(
            "[{}...]: nil | {}",
            tail.start,
            render(&tail.value)
        ));
    }
    format!("{{ {} }}", members.join(", "))
}

fn named_key(bytes: &[u8]) -> bool {
    let Some(first) = bytes.first() else {
        return false;
    };
    (first.is_ascii_alphabetic() || *first == b'_')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        && !matches!(
            bytes,
            b"and"
                | b"break"
                | b"do"
                | b"else"
                | b"elseif"
                | b"end"
                | b"false"
                | b"for"
                | b"function"
                | b"goto"
                | b"if"
                | b"in"
                | b"local"
                | b"nil"
                | b"not"
                | b"or"
                | b"repeat"
                | b"return"
                | b"then"
                | b"true"
                | b"until"
                | b"while"
        )
}

/// Keep Lua's integer domain before any conversion to the backend's ordinary
/// number-literal type. In particular, adjacent integers above 2^53 differ.
fn numeric_key(spelling: &str) -> Option<LuaKey> {
    let (negative, unsigned) = spelling
        .strip_prefix('-')
        .map_or((false, spelling), |unsigned| (true, unsigned));
    let hexadecimal = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"));
    let integer = if let Some(hexadecimal) = hexadecimal {
        if hexadecimal.contains(['.', 'p', 'P']) || hexadecimal.is_empty() {
            None
        } else {
            // Lua 5.4 integer hexadecimal syntax accumulates modulo 2^64.
            let mut value = 0_u64;
            for digit in hexadecimal.chars() {
                value = value
                    .wrapping_mul(16)
                    .wrapping_add(u64::from(digit.to_digit(16)?));
            }
            Some(value as i64)
        }
    } else if !unsigned.is_empty() && unsigned.bytes().all(|byte| byte.is_ascii_digit()) {
        unsigned.parse::<i64>().ok()
    } else {
        None
    };
    if let Some(integer) = integer {
        return Some(LuaKey::Integer(if negative {
            integer.wrapping_neg()
        } else {
            integer
        }));
    }
    // Decimal overflow/fraction/exponent and hexadecimal float syntax are
    // floating literals. Integral-float canonicalization is the core's job.
    crate::number(spelling).map(LuaKey::Number)
}

pub(super) fn literal_key(expression: &Expr) -> Option<LuaKey> {
    match &expression.kind {
        ExprKind::String(bytes) => Some(LuaKey::String(bytes.clone())),
        ExprKind::Boolean(value) => Some(LuaKey::Boolean(*value)),
        ExprKind::Number(spelling) => numeric_key(spelling),
        ExprKind::Parenthesized(inner) => literal_key(inner),
        ExprKind::Unary {
            op: UnaryOperator::Negate,
            value,
            ..
        } => match literal_key(value)? {
            LuaKey::Integer(value) => Some(LuaKey::Integer(value.wrapping_neg())),
            LuaKey::Number(value) => Some(LuaKey::Number(-value)),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::numeric_key;
    use luanalysis_engine::LuaKey;

    #[test]
    fn lua_integer_key_spelling_preserves_all_bits() {
        assert!(matches!(
            numeric_key("9007199254740992"),
            Some(LuaKey::Integer(9007199254740992))
        ));
        assert!(matches!(
            numeric_key("9007199254740993"),
            Some(LuaKey::Integer(9007199254740993))
        ));
        assert!(matches!(
            numeric_key("0xffffffffffffffff"),
            Some(LuaKey::Integer(-1))
        ));
        assert!(matches!(
            numeric_key("0x8000000000000000"),
            Some(LuaKey::Integer(i64::MIN))
        ));
        assert!(matches!(
            numeric_key("-0x8000000000000000"),
            Some(LuaKey::Integer(i64::MIN))
        ));
        assert!(matches!(
            numeric_key("0x10000000000000000"),
            Some(LuaKey::Integer(0))
        ));
    }

    #[test]
    fn lua_floating_syntax_and_decimal_overflow_stay_float_keys() {
        assert!(matches!(numeric_key("1.0"), Some(LuaKey::Number(value)) if value == 1.0));
        assert!(matches!(numeric_key("0x1p0"), Some(LuaKey::Number(value)) if value == 1.0));
        assert!(matches!(
            numeric_key("9223372036854775808"),
            Some(LuaKey::Number(_))
        ));
        assert!(numeric_key("0xnothex").is_none());
    }
}
