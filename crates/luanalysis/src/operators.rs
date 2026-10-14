//! Lua operator semantics over the native checking session's types.
//!
//! Numeric/string bounds and result unions use the shared engine. Lua's
//! operand-valued short circuiting and its two falsey values are language
//! semantics, not JavaScript operator rules. Metamethods, integer domains and
//! arbitrary table indexers require their own later native boundaries.

use super::{Checker, describe};
use luanalysis_engine::Type;
use luanalysis_syntax::{BinaryOperator, Expr, ExprKind, Name, Span, UnaryOperator};

impl Checker<'_> {
    pub(super) fn unary_expression(
        &mut self,
        op: UnaryOperator,
        _op_span: Span,
        value: &Expr,
    ) -> Type {
        let value_type = self.expression(value);
        if matches!(value_type, Type::Never) {
            return Type::Never;
        }
        match op {
            UnaryOperator::Not => {
                let (truthy, falsey) = self.lua_truthiness(&value_type);
                match (is_never(&truthy), is_never(&falsey)) {
                    (true, true) => Type::Never,
                    (true, false) => Type::BooleanLiteral(true),
                    (false, true) => Type::BooleanLiteral(false),
                    (false, false) => Type::Boolean,
                }
            }
            UnaryOperator::Length => {
                if self.length_operand(&value_type) {
                    Type::Number
                } else {
                    self.error(
                        "LUA2020",
                        value.span,
                        format!(
                            "Operator '#' requires a string or table operand; got '{}'. Length metamethods are not implemented.",
                            describe(&value_type)
                        ),
                    );
                    Type::Any
                }
            }
            UnaryOperator::Negate | UnaryOperator::BitwiseNot => {
                if self.engine.is_assignable(&value_type, &Type::Number) {
                    // Preserve a literal negative number. Integer precision and
                    // bitwise ranges are not encoded by the TS number domain.
                    if op == UnaryOperator::Negate {
                        if let Type::NumberLiteral(number) = value_type {
                            return Type::NumberLiteral(-number);
                        }
                    }
                    Type::Number
                } else {
                    self.error(
                        "TS2356",
                        value.span,
                        format!(
                            "Operator '{}' requires a number operand; got '{}'. Arithmetic metamethods are not implemented.",
                            unary_spelling(op),
                            describe(&value_type)
                        ),
                    );
                    Type::Any
                }
            }
        }
    }

    pub(super) fn binary_expression(
        &mut self,
        op: BinaryOperator,
        op_span: Span,
        left: &Expr,
        right: &Expr,
    ) -> Type {
        let left_type = self.expression(left);
        if matches!(left_type, Type::Never) {
            return Type::Never;
        }
        if matches!(op, BinaryOperator::And | BinaryOperator::Or) {
            let (truthy, falsey) = self.lua_truthiness(&left_type);
            let (kept, evaluated) = match op {
                BinaryOperator::And => (falsey, truthy),
                BinaryOperator::Or => (truthy, falsey),
                _ => unreachable!(),
            };
            if is_never(&evaluated) {
                // The RHS is not a runtime demand on this path. In particular,
                // do not request a lazy signature or report missing names there.
                return kept;
            }
            let right_type = if let Some(reference) = self.reference(left) {
                // This expression-local transfer uses the existing direct-write
                // facts. It is not a second CFG or generic inference engine.
                // Keep declaration constraints and restore incoming facts after
                // the conditional demand; querying a RHS must not execute it.
                let previous_locals = self.locals.clone();
                let previous_fields = self.assigned_fields.clone();
                if reference.path.is_empty() {
                    if let Some(binding) = self.locals.get_mut(&reference.binding) {
                        binding.value = evaluated;
                    }
                } else {
                    self.assigned_fields.insert(reference, evaluated);
                }
                let result = self.expression(right);
                self.locals = previous_locals;
                self.assigned_fields = previous_fields;
                result
            } else {
                self.expression(right)
            };
            return self.engine.union_reduced(&[kept, right_type]);
        }

        let right_type = self.expression(right);
        if matches!(right_type, Type::Never) {
            return Type::Never;
        }
        match op {
            BinaryOperator::Equal | BinaryOperator::NotEqual => Type::Boolean,
            BinaryOperator::Concat => {
                let accepted = Type::Union(vec![Type::String, Type::Number]);
                let valid_left = self.engine.is_assignable(&left_type, &accepted);
                let valid_right = self.engine.is_assignable(&right_type, &accepted);
                if !valid_left {
                    self.concat_error(left.span, &left_type);
                }
                if !valid_right {
                    self.concat_error(right.span, &right_type);
                }
                if valid_left && valid_right {
                    Type::String
                } else {
                    Type::Any
                }
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                let both_numbers = self.engine.is_assignable(&left_type, &Type::Number)
                    && self.engine.is_assignable(&right_type, &Type::Number);
                let both_strings = self.engine.is_assignable(&left_type, &Type::String)
                    && self.engine.is_assignable(&right_type, &Type::String);
                if both_numbers || both_strings {
                    Type::Boolean
                } else {
                    self.error(
                        "TS2365",
                        op_span,
                        format!(
                            "Operator '{}' requires two numbers or two strings; got '{}' and '{}'. Comparison metamethods are not implemented.",
                            binary_spelling(op),
                            describe(&left_type),
                            describe(&right_type)
                        ),
                    );
                    Type::Any
                }
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::FloorDivide
            | BinaryOperator::Modulo
            | BinaryOperator::Power
            | BinaryOperator::BitwiseAnd
            | BinaryOperator::BitwiseOr
            | BinaryOperator::BitwiseXor
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight => {
                let valid_left = self.engine.is_assignable(&left_type, &Type::Number);
                let valid_right = self.engine.is_assignable(&right_type, &Type::Number);
                if !valid_left {
                    self.numeric_error("TS2362", left.span, op, &left_type);
                }
                if !valid_right {
                    self.numeric_error("TS2363", right.span, op, &right_type);
                }
                if valid_left && valid_right {
                    Type::Number
                } else {
                    Type::Any
                }
            }
            BinaryOperator::And | BinaryOperator::Or => unreachable!(),
        }
    }

    /// Exact primitive partitioning uses Lua truthiness. `any`, `unknown` and
    /// future unspecialized native descriptions conservatively occur in both
    /// partitions; no JS truthiness predicate is used to filter them.
    fn lua_truthiness(&mut self, ty: &Type) -> (Type, Type) {
        match ty {
            Type::Never => (Type::Never, Type::Never),
            Type::Nil | Type::Undefined | Type::BooleanLiteral(false) => (Type::Never, ty.clone()),
            Type::Boolean => (Type::BooleanLiteral(true), Type::BooleanLiteral(false)),
            Type::BooleanLiteral(true)
            | Type::Number
            | Type::NumberLiteral(_)
            | Type::String
            | Type::StringLiteral(_)
            | Type::Shape(_)
            | Type::FreshShape(_)
            | Type::Function(_)
            | Type::NativeFunction(_) => (ty.clone(), Type::Never),
            Type::Union(types) => {
                let mut truthy = Vec::new();
                let mut falsey = Vec::new();
                for ty in types {
                    let (yes, no) = self.lua_truthiness(ty);
                    truthy.push(yes);
                    falsey.push(no);
                }
                (
                    self.engine.union_reduced(&truthy),
                    self.engine.union_reduced(&falsey),
                )
            }
            Type::Any | Type::Unknown => (ty.clone(), ty.clone()),
            _ => {
                // In particular, reuse native type-parameter constraints when
                // they establish a numeric/string bound. Do not treat failure
                // of nil assignability TO a parameter as proof of truthiness.
                if self.engine.is_assignable(ty, &Type::Number)
                    || self.engine.is_assignable(ty, &Type::String)
                {
                    (ty.clone(), Type::Never)
                } else {
                    (ty.clone(), ty.clone())
                }
            }
        }
    }

    fn length_operand(&mut self, ty: &Type) -> bool {
        match ty {
            Type::Any | Type::Never | Type::Shape(_) | Type::FreshShape(_) => true,
            Type::Union(types) => types.iter().all(|ty| self.length_operand(ty)),
            Type::Intersection(types) => types.iter().any(|ty| self.length_operand(ty)),
            // A structural empty-object target also accepts non-table values in
            // TS; it cannot be used as a test for the Lua table runtime tag.
            _ => self.engine.is_assignable(ty, &Type::String),
        }
    }

    fn numeric_error(&mut self, code: &'static str, span: Span, op: BinaryOperator, ty: &Type) {
        self.error(
            code,
            span,
            format!(
                "Operator '{}' requires a number operand; got '{}'. Arithmetic metamethods are not implemented.",
                binary_spelling(op),
                describe(ty)
            ),
        );
    }

    fn concat_error(&mut self, span: Span, ty: &Type) {
        self.error(
            "LUA2020",
            span,
            format!(
                "Operator '..' requires a string or number operand; got '{}'. Concatenation metamethods are not implemented.",
                describe(ty)
            ),
        );
    }

    pub(super) fn index_expression(
        &mut self,
        expression: &Expr,
        base: &Expr,
        index: &Expr,
    ) -> Type {
        let receiver = self.expression(base);
        // Preserve the original expression query, even though only a literal
        // byte-string key is currently representable by record member lookup.
        self.expression(index);
        let Some(bytes) = literal_string_key(index) else {
            self.error(
                "LUA2021",
                index.span,
                "nonliteral-string table indexing requires native Lua key/indexer support; numeric keys are not coerced to strings",
            );
            return Type::Any;
        };
        let Ok(key) = std::str::from_utf8(bytes) else {
            self.error(
                "LUA2021",
                index.span,
                "non-UTF-8 table keys require the pending Lua byte-key representation",
            );
            return Type::Any;
        };
        if matches!(receiver, Type::Any) {
            return Type::Any;
        }
        if let Some(ty) = self.engine.property_type(&receiver, key) {
            self.reference(expression)
                .and_then(|reference| self.assigned_fields.get(&reference).cloned())
                .unwrap_or(ty)
        } else {
            self.error(
                "TS2339",
                index.span,
                format!(
                    "Property '{}' does not exist on type '{}'.",
                    key,
                    describe(&receiver)
                ),
            );
            Type::Any
        }
    }

    pub(super) fn method_call_expression(
        &mut self,
        receiver: &Expr,
        name: &Name,
        args: &[Expr],
        span: Span,
        discard: bool,
    ) -> Type {
        let receiver_type = self.expression(receiver);
        let callee = if matches!(receiver_type, Type::Any) {
            Type::Any
        } else if let Some(callee) = self.engine.property_type(&receiver_type, &name.text) {
            self.reference(receiver)
                .and_then(|mut reference| {
                    reference.path.push(name.text.clone());
                    self.assigned_fields.get(&reference).cloned()
                })
                .unwrap_or(callee)
        } else {
            self.error(
                "TS2339",
                name.span,
                format!(
                    "Property '{}' does not exist on type '{}'.",
                    name.text,
                    describe(&receiver_type)
                ),
            );
            Type::Any
        };
        self.record_expression_type(name.span, &callee);
        let mut arguments = Vec::with_capacity(args.len() + 1);
        let mut origins = Vec::with_capacity(args.len() + 1);
        arguments.push(receiver_type);
        origins.push(receiver.span);
        for arg in args {
            arguments.push(self.expression(arg));
            origins.push(arg.span);
        }
        self.call_types(&callee, &arguments, &origins, name.span, span, discard)
    }
}

fn is_never(ty: &Type) -> bool {
    matches!(ty, Type::Never)
}

pub(super) fn literal_string_key(expr: &Expr) -> Option<&[u8]> {
    match &expr.kind {
        ExprKind::String(bytes) => Some(bytes),
        ExprKind::Parenthesized(inner) => literal_string_key(inner),
        _ => None,
    }
}

fn unary_spelling(op: UnaryOperator) -> &'static str {
    match op {
        UnaryOperator::Not => "not",
        UnaryOperator::Length => "#",
        UnaryOperator::Negate => "-",
        UnaryOperator::BitwiseNot => "~",
    }
}

fn binary_spelling(op: BinaryOperator) -> &'static str {
    match op {
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::FloorDivide => "//",
        BinaryOperator::Modulo => "%",
        BinaryOperator::Power => "^",
        BinaryOperator::Concat => "..",
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "~=",
        BinaryOperator::Less => "<",
        BinaryOperator::LessEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterEqual => ">=",
        BinaryOperator::BitwiseAnd => "&",
        BinaryOperator::BitwiseOr => "|",
        BinaryOperator::BitwiseXor => "~",
        BinaryOperator::ShiftLeft => "<<",
        BinaryOperator::ShiftRight => ">>",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
    }
}
