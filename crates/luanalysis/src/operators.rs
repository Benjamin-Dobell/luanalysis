//! Lua operator semantics over the native checking session's types.
//!
//! Numeric/string bounds and result unions use the shared engine. Lua's
//! operand-valued short circuiting and its two falsey values are language
//! semantics, not JavaScript operator rules. Metamethods, integer domains and
//! arbitrary table indexers require their own later native boundaries.

use super::Checker;
use luanalysis_engine::{PackArgument, TableError, Type, ValuePack};
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
                    self.error_types(
                        "LUA2020",
                        value.span,
                        "Operator '#' requires a string or table operand; got '{}'. Length metamethods are not implemented.",
                        &[&value_type],
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
                    self.error_types(
                        "TS2356",
                        value.span,
                        format!(
                            "Operator '{}' requires a number operand; got '{{}}'. Arithmetic metamethods are not implemented.",
                            unary_spelling(op)
                        ),
                        &[&value_type],
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
            return if self.runtime_value {
                self.engine.runtime_value_union(&[kept, right_type])
            } else {
                self.engine.union_reduced(&[kept, right_type])
            };
        }

        let right_type = self.expression(right);
        if matches!(right_type, Type::Never) {
            return Type::Never;
        }
        match op {
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                self.inspect_comparison(left, right, &left_type, &right_type);
                Type::Boolean
            }
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
                    self.error_types(
                        "TS2365",
                        op_span,
                        format!(
                            "Operator '{}' requires two numbers or two strings; got '{{}}' and '{{}}'. Comparison metamethods are not implemented.",
                            binary_spelling(op)
                        ),
                        &[&left_type, &right_type],
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

    /// Keep opaque regular literals and generic constraints in the same native
    /// Lua truthiness domain used by graph predicates.
    pub(crate) fn lua_truthiness(&mut self, ty: &Type) -> (Type, Type) {
        (self.engine.lua_truthiness_type(ty, true), self.engine.lua_truthiness_type(ty, false))
    }

    fn length_operand(&mut self, ty: &Type) -> bool {
        // Query the canonical runtime domains, preserving opaque callable-table
        // carriers. A public function-signature projection is not a Lua tag.
        self.engine.is_assignable(ty, &Type::Union(vec![Type::String, Type::RuntimeTable]))
    }

    fn numeric_error(&mut self, code: &'static str, span: Span, op: BinaryOperator, ty: &Type) {
        self.error_types(
            code,
            span,
            format!(
                "Operator '{}' requires a number operand; got '{{}}'. Arithmetic metamethods are not implemented.",
                binary_spelling(op)
            ),
            &[ty],
        );
    }

    fn concat_error(&mut self, span: Span, ty: &Type) {
        self.error_types(
            "LUA2020",
            span,
            "Operator '..' requires a string or number operand; got '{}'. Concatenation metamethods are not implemented.",
            &[ty],
        );
    }

    pub(super) fn method_call_expression(
        &mut self,
        receiver: &Expr,
        name: &Name,
        args: &[Expr],
        span: Span,
        discard: bool,
    ) -> Type {
        self.method_call_expression_with_expected(receiver, name, args, span, discard, None)
    }

    pub(super) fn method_call_expression_with_expected(
        &mut self,
        receiver: &Expr,
        name: &Name,
        args: &[Expr],
        span: Span,
        _discard: bool,
        expected: Option<&Type>,
    ) -> Type {
        let (callee, arguments, origins) = self.method_call_arguments(receiver, name, args);
        let call = self
            .native
            .map(|native| native.expression_key_for_span(span));
        let result =
            self.engine
                .call_native_arguments_scalar_in_frame(&callee, call, &arguments, &[], expected,
                    luanalysis_engine::NativeCallerFrame { contextual: self.contextual_function, captured: self.captured_function });
        self.call_result(result, &callee, &origins, name.span, span)
    }

    pub(super) fn method_call_pack(
        &mut self,
        receiver: &Expr,
        name: &Name,
        args: &[Expr],
        span: Span,
        expected: Option<&ValuePack>,
    ) -> ValuePack {
        let (callee, arguments, origins) = self.method_call_arguments(receiver, name, args);
        let call = self
            .native
            .map(|native| native.expression_key_for_span(span));
        let result = self
            .engine
            .call_native_arguments_in_frame(&callee, call, &arguments, &[], expected,
                luanalysis_engine::NativeCallerFrame { contextual: self.contextual_function, captured: self.captured_function });
        self.pack_call_result(result, &callee, &origins, name.span, span)
    }

    fn method_call_arguments(
        &mut self,
        receiver: &Expr,
        name: &Name,
        args: &[Expr],
    ) -> (Type, Vec<PackArgument>, Vec<Span>) {
        let receiver_type = self.expression(receiver);
        let member_contract = self.runtime_member_contract(receiver,receiver_type.clone());
        let callee = if matches!(receiver_type, Type::Any) {
            Type::Any
        } else {
            match self.engine.property_type_checked(&member_contract, &name.text) {
            Ok(Some(callee)) => self.native.is_none().then(|| self.reference(receiver)).flatten()
                .and_then(|mut reference| {
                    reference.path.push(name.text.clone());
                    self.assigned_fields.get(&reference).cloned()
                })
                .unwrap_or(callee),
            Err(TableError::ValueType { value, expected }) => {
                self.error_types("TS2345", receiver.span, "Receiver type '{}' does not satisfy '{}'.", &[&value, &expected]);
                self.engine.error_type()
            }
            Ok(None) | Err(TableError::KeyType { .. } | TableError::MissingKey { .. } | TableError::NotTable { .. }) => {
            self.error_types(
                "TS2339",
                name.span,
                format!(
                    "Property '{}' does not exist on type '{{}}'.",
                    name.text
                ),
                &[&receiver_type],
            );
            Type::Any
            }
            Err(error) => { self.native_table_error(error, name.span, receiver.span); self.engine.error_type() }
            }
        };
        self.record_expression_type(name.span, &callee);
        // A receiver is never contextually typed: its type, computed once above, is the `self`
        // argument of every resolution pass. Checking it again per pass made a chain of method
        // calls (`builder:a(1):b(2):c(3)`) exponential in its length.
        let mut arguments = vec![PackArgument::Resolved(receiver_type)];
        arguments.extend(self.call_arguments(args));
        let mut origins = vec![receiver.span];
        origins.extend(args.iter().map(|argument| argument.span));
        (callee, arguments, origins)
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
