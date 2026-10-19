//! Original Lua function annotations, call arguments and result-list contexts.
//! Signature identity, lazy return inference and whole-pack relations are native.

use super::native_packs::{Adjustment, AnnotationPack, adjustment};
use super::{Binding, Checker};
use luanalysis_engine::{
    CallError, CallResult, FunctionType, PackArgument, PackCallResult, PackFunctionType, Parameter,
    Type, ValuePack,
};
use luanalysis_syntax::{Expr, ExprKind, Function, Name, Span};
use crate::collections::HashSet;

impl Checker<'_> {
    pub(super) fn expression_pack(
        &mut self,
        expression: &Expr,
        expected: Option<&ValuePack>,
    ) -> ValuePack {
        if let Some(pack) = self.asserted_expression_pack(expression) {
            return self.finish_class_definition_pack(expression.span, pack);
        }
        let previous = self.flow_at;
        if let (Some(native), Some(at)) = (self.native, previous) {
            self.flow_at = Some(native.flow_input_for_expression(expression, at));
        }
        let pack = match &expression.kind {
            ExprKind::Call { callee, args } => {
                let pack = self.call_expression_pack(callee, args, expression.span, expected);
                self.finish_class_definition_pack(expression.span, pack)
            }
            ExprKind::MethodCall {
                receiver,
                name,
                args,
            } => {
                let pack = self.method_call_pack(receiver, name, args, expression.span, expected);
                self.finish_class_definition_pack(expression.span, pack)
            }
            ExprKind::Vararg => {
                let pack = self.vararg_pack(expression.span);
                self.finish_class_definition_pack(expression.span, pack)
            }
            _ => {
                let context = expected.and_then(|pack| self.engine.pack_contextual_slot(pack, 0));
                ValuePack::single(self.expression_with_expected(expression, context.as_ref()))
            }
        };
        self.flow_at = previous;
        pack
    }

    /// `expressions` are the tree's own or shared ones (`SharedExpr`).
    pub(super) fn expression_list_pack<E: std::borrow::Borrow<Expr>>(
        &mut self,
        expressions: &[E],
        expected: Option<&ValuePack>,
    ) -> ValuePack {
        let Some((last, prefix_expressions)) = expressions.split_last() else {
            return ValuePack::empty();
        };
        let last = last.borrow();
        let mut prefix = Vec::with_capacity(prefix_expressions.len() + 1);
        for (index, expression) in prefix_expressions.iter().enumerate() {
            let expression = expression.borrow();
            let context = expected.and_then(|pack| self.engine.pack_contextual_slot(pack, index));
            prefix.push(self.expression_with_expected(expression, context.as_ref()));
        }
        let tail_context = expected.and_then(|pack| {
            self.engine.pack_contextual_slot(pack, prefix.len())?;
            self.engine.pack_contextual_tail(pack, &prefix)
        });
        let tail = match adjustment(last, true) {
            Adjustment::Expand => self.expression_pack(last, tail_context.as_ref()),
            Adjustment::Scalar => {
                let context =
                    expected.and_then(|pack| self.engine.pack_contextual_slot(pack, prefix.len()));
                ValuePack::single(self.expression_with_expected(last, context.as_ref()))
            }
        };
        self.engine.concat_pack(&prefix, &tail)
    }

    pub(super) fn adjusted_list_values(
        &mut self,
        expressions: &[Expr],
        destinations: impl ExactSizeIterator<Item = Span>,
        expected: Option<&ValuePack>,
        missing_origin: Span,
    ) -> Vec<(Type, Span)> {
        let count = destinations.len();
        let first_diagnostic = self.report.diagnostics.len();
        let pack = self.expression_list_pack(expressions, expected);
        if self.native.is_some_and(|native| native.inspections())
            && !self.report.diagnostics.iter().skip(first_diagnostic)
                .any(|diagnostic| diagnostic.category() == crate::DiagnosticCategory::Error)
        {
            self.inspect_assignment_result_count(expressions, &destinations.collect::<Vec<_>>(), &pack);
        }
        let adjusted = match self.engine.adjust_pack(&pack, count) {
            Ok(pack) => pack,
            Err(error) => {
                self.error(
                    "LUA2019",
                    missing_origin,
                    format!("native Lua result-pack adjustment failed: {error:?}"),
                );
                return vec![(Type::Unknown, missing_origin); count];
            }
        };
        let prefix = expressions.len().saturating_sub(1);
        (0..count)
            .map(|index| {
                let origin = expressions
                    .get(index)
                    .filter(|_| index < prefix)
                    .or_else(|| expressions.last())
                    .map_or(missing_origin, |expression| expression.span);
                (self.engine.pack_slot(&adjusted, index), origin)
            })
            .collect()
    }

    fn vararg_pack(&mut self, origin: Span) -> ValuePack {
        let Some(native) = self.native else {
            return ValuePack::variadic(Type::Any);
        };
        let Some(key) = self.type_context else {
            if native.has_module_index() {
                self.error("LUA2036",origin,"Module chunk varargs require a native loader-argument contract.");
                return ValuePack::variadic(Type::Unknown);
            }
            return ValuePack::variadic(Type::Any);
        };
        let function = native.function(key);
        if function.vararg.is_none() {
            self.error(
                "LUA1001",
                origin,
                "vararg expression has no variadic function scope",
            );
            return ValuePack::never();
        }
        if let Some(captured) = self
            .captured_function
            .or_else(|| self.engine.current_captured_function())
        {
            let value = self.engine.captured_function_value(&captured);
            if self.engine.native_function_key(&value) == Some(key) {
                if let Some(pack) = self.engine.captured_rest_pack(&captured) {
                    return pack;
                }
            }
        }
        if let Some(context) = self.contextual_function {
            if function.annotations.vararg.is_none() {
                return self
                    .engine
                    .contextual_rest_pack(&context, function.params.len() + usize::from(native.implicit_receiver(function).is_some()));
            }
        }
        self.engine
            .signature_rest_pack(&Type::NativeFunction(key))
            .unwrap_or_else(ValuePack::empty)
    }

    fn call_expression_pack(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
        expected: Option<&ValuePack>,
    ) -> ValuePack {
        if let Some(pack) = self.native_require_pack(callee, args, span) { return pack; }
        let callee_type = self.expression(callee);
        let arguments = self.call_arguments(args);
        let call = self
            .native
            .map(|native| native.expression_key_for_span(span));
        let result =
            self.engine
                .call_native_arguments_in_frame(&callee_type, call, &arguments, &[], expected,
                    luanalysis_engine::NativeCallerFrame { contextual: self.contextual_function, captured: self.captured_function });
        let origins: Vec<_> = args.iter().map(|argument| argument.span).collect();
        let checked = result.errors.is_empty();
        let pack = self.pack_call_result(result, &callee_type, &origins, callee.span, span);
        if checked && let Some(pack) = self.standard_pattern_call(callee, args) { return pack }
        let first = self.engine.pack_slot(&pack, 0);
        // A class definition's table is the class's, not a runtime table's.
        let defines_class = self.native.is_some_and(|native| native.is_class_definition_initializer(span));
        let table = if defines_class { first.clone() } else { self.setmetatable_result(callee, &callee_type, args, first.clone()) };
        if table == first { pack } else { ValuePack::single(table) }
    }

    pub(super) fn call_arguments(&mut self, args: &[Expr]) -> Vec<PackArgument> {
        args.iter()
            .enumerate()
            .map(|(index, argument)| {
                let expands = adjustment(argument, index + 1 == args.len()) == Adjustment::Expand;
                if let Some(native) = self.native {
                    let key = native.expression_key(argument);
                    if expands {
                        PackArgument::ExpandExpression(key)
                    } else {
                        PackArgument::Expression(key)
                    }
                } else if expands {
                    PackArgument::Expand(self.expression_pack(argument, None))
                } else {
                    PackArgument::Resolved(self.expression(argument))
                }
            })
            .collect()
    }

    pub(super) fn pack_call_result(
        &mut self,
        result: PackCallResult,
        callee: &Type,
        origins: &[Span],
        callee_span: Span,
        span: Span,
    ) -> ValuePack {
        self.call_predicate = result.predicate.clone().map(|predicate| (span, predicate));
        self.call_result(
            CallResult {
                return_type: result.return_type,
                errors: result.errors,
                predicate: result.predicate,
            },
            callee,
            origins,
            callee_span,
            span,
        );
        result.return_pack
    }

    pub(super) fn function_annotation(
        &mut self,
        function: &luanalysis_syntax::FunctionType,
    ) -> Type {
        if function.parameters_span.is_none() && function.returns_span.is_none() && function.generics.is_empty() {
            return Type::RuntimeFunction;
        }
        if let Some(native)=self.native {
            if let Some(key)=native.signature_key_for(function.span) {
                return self.engine.signature_type(key);
            }
        }
        let mut optional_seen = false;
        let params = function
            .params
            .iter()
            .map(|param| {
                if optional_seen && !param.optional {
                    self.error(
                        "TS1016",
                        param.name.span,
                        "A required parameter cannot follow an optional parameter.",
                    );
                }
                optional_seen |= param.optional;
                let ty = param
                    .ty
                    .as_ref()
                    .map(|ty| self.annotation(ty))
                    .unwrap_or_else(|| {
                        self.implicit_parameter_any(
                            "TS7006",
                            param.name.span,
                            &param.name.text,
                            format!(
                                "Parameter '{}' implicitly has an 'any' type.",
                                param.name.text
                            ),
                        );
                        Type::Any
                    });
                Parameter {
                    name: param.name.text.clone(),
                    ty,
                    optional: param.optional,
                }
            })
            .collect();
        let returns = if function.returns_span.is_some() {
            AnnotationPack::new(&function.returns, function.packs.as_deref(), function.span)
                .lower(&mut |ty| self.annotation(ty))
        } else {
            ValuePack::variadic(Type::Any)
        };
        let returns = self.engine.declared_result_pack(&returns);
        let returns = self.engine.pack_with_implicit_empty(&returns);
        let rest = function
            .vararg
            .as_ref()
            .map(|vararg| ValuePack::variadic(self.annotation(&vararg.ty)))
            .or_else(|| function.parameters_span.is_none().then(|| ValuePack::variadic(Type::Any)));
        self.engine.pack_function(&PackFunctionType {
            params,
            rest,
            type_parameters: Vec::new(),
            returns,
        })
    }

    fn function_signature(&mut self, function: &Function) -> FunctionType {
        let (params, returned) = self.signature_parts(function);
        let returned = returned.unwrap_or_else(|| {
            self.error(
                "LUA2014",
                function.span,
                "function return inference requires the native declaration provider",
            );
            Type::Any
        });
        FunctionType {
            params,
            returns: Box::new(returned),
        }
    }

    /// Duplicate and unknown `---@param` tags. Purely syntactic, so it does
    /// not depend on any contextual or captured frame.
    pub(super) fn check_parameter_annotations(&mut self, function: &Function) {
        let receiver = self.native.and_then(|native| native.implicit_receiver(function));
        let mut seen = HashSet::default();
        for tag in &function.annotations.params {
            if !seen.insert(&tag.name.text) {
                self.error(
                    "LUA2011",
                    tag.name.span,
                    format!("duplicate annotation for parameter '{}'", tag.name.text),
                );
            }
            if !(receiver.is_some() && tag.name.text == "self") && !function
                .params
                .iter()
                .any(|param| param.text == tag.name.text)
            {
                self.error(
                    "LUA2011",
                    tag.name.span,
                    format!("annotation refers to unknown parameter '{}'", tag.name.text),
                );
            }
        }
    }

    pub(super) fn signature_parts(
        &mut self,
        function: &Function,
    ) -> (Vec<Parameter>, Option<Type>) {
        let context = function
            .annotations
            .type_annotation
            .as_ref()
            .and_then(|annotation| {
                if annotation.types.len() != 1 {
                    self.error(
                        "LUA2010",
                        annotation.span,
                        "a function definition requires one function type",
                    );
                    return None;
                }
                let ty = self.annotation(&annotation.types[0]);
                match self.engine.function_signature(&ty) {
                    Some(signature) => Some(signature),
                    None => {
                        self.error(
                            "LUA2010",
                            annotation.span,
                            "a function definition requires a function type",
                        );
                        None
                    }
                }
            });
        let receiver = self.native.and_then(|native| native.implicit_receiver(function));
        let offset = usize::from(receiver.is_some());
        self.check_parameter_annotations(function);
        let mut params = Vec::new();
        let mut optional_seen = false;
        if let Some(binding) = receiver {
            let tag = (!function.params.iter().any(|parameter| parameter.text == "self"))
                .then(|| function.annotations.params.iter().find(|tag| tag.name.text == "self")).flatten();
            let (ty, optional) = if let Some(tag) = tag {
                (self.annotation(&tag.ty), tag.optional)
            } else if let Some(ty) = self.native.and_then(|native| native.applied_method_self_type(native.key_for(function), self.engine)) {
                (ty, false)
            } else if let Some(parameter) = context.as_ref().and_then(|context| context.params.first()) {
                (parameter.ty.clone(), parameter.optional)
            } else if let Some(ty) = self.contextual_function.and_then(|context| self.engine.contextual_parameter_type(&context, 0)) {
                (ty, false)
            } else if let Some(ty) = self.native.and_then(|native| native.default_receiver_type(function, self.engine)) {
                (ty, false)
            } else {
                self.implicit_parameter_any("TS7006", self.bound.declaration(binding).span, "self", "The implicit receiver has no type contract.");
                (Type::Any, false)
            };
            optional_seen = optional;
            params.push(Parameter { name: "self".into(), ty, optional });
        }
        for (index, param) in function.params.iter().enumerate() {
            let (ty, optional) = if let Some(tag) = function
                .annotations
                .params
                .iter()
                .find(|tag| tag.name.text == param.text)
            {
                (self.annotation(&tag.ty), tag.optional)
            } else if let Some(expected) = context
                .as_ref()
                .and_then(|context| context.params.get(index + offset))
            {
                (expected.ty.clone(), expected.optional)
            } else if let Some(ty) = self
                .contextual_function
                .and_then(|context| self.engine.contextual_parameter_type(&context, index + offset))
            {
                (ty, false)
            } else {
                self.implicit_parameter_any(
                    "TS7006",
                    param.span,
                    &param.text,
                    format!("Parameter '{}' implicitly has an 'any' type.", param.text),
                );
                (Type::Any, untyped_parameters(function))
            };
            if optional_seen && !optional {
                self.error(
                    "TS1016",
                    param.span,
                    "A required parameter cannot follow an optional parameter.",
                );
            }
            optional_seen |= optional;
            params.push(Parameter {
                name: param.text.clone(),
                ty,
                optional,
            });
        }
        let returns = if let Some(annotation) = &function.annotations.returns {
            match annotation.types.as_slice() {
                [ty] if annotation.packs.is_none() => Some(self.annotation(ty)),
                _ => {
                    let pack =
                        AnnotationPack::annotation(annotation).lower(&mut |ty| self.annotation(ty));
                    Some(self.engine.pack_slot(&pack, 0))
                }
            }
        } else if let Some(context) = context {
            Some(*context.returns)
        } else {
            None
        };
        (params, returns)
    }

    pub(super) fn local_function(&mut self, name: &Name, function: &Function) {
        let legacy_signature = self
            .native
            .is_none()
            .then(|| self.function_signature(function));
        let ty = if let Some(native) = self.native {
            native.source_function_value(function, self.check_mode, self.engine)
        } else {
            Type::Function(legacy_signature.as_ref().unwrap().clone())
        };
        let mut constraint = ty.clone();
        if let Some(annotation) = &function.annotations.type_annotation {
            if let [expected] = annotation.types.as_slice() {
                let expected = self.annotation(expected);
                self.compatible(&ty, &expected, name.span);
                constraint = expected;
            }
        }
        let binding = self
            .bound
            .binding_at(name.span.start)
            .expect("bound local function");
        let value = self.engine.assignment_value(&constraint, &ty);
        self.locals.insert(
            binding,
            Binding {
                constraint: constraint.clone(),
                initial: constraint.clone(),
                value,
                evolves_from_nil: false,
            },
        );
        self.record_binding(name, binding, &constraint);
        if let Some(signature) = legacy_signature {
            self.function_body(function, &signature);
        }
    }

    pub(super) fn function_expression(&mut self, function: &Function) -> Type {
        if let Some(native) = self.native {
            return native.source_function_value(function, self.check_mode, self.engine);
        }
        let signature = self.function_signature(function);
        self.function_body(function, &signature);
        Type::Function(signature)
    }

    pub(super) fn named_function(
        &mut self,
        target: &Expr,
        function: &Function,
        method_span: Option<Span>,
    ) {
        let _ = method_span; // The original receiver binding is owned by the binder.
        if let ExprKind::Name(name) = &target.kind {
            if self.bound.binding_at(name.span.start).is_none() {
                if let Some(constraint) = self.global_read(name) {
                    // The declared global types the implementation's parameters and body, as an
                    // assignment's target does (`name = function(...) end`).
                    let value = match self.native {
                        Some(native) => native.source_named_function_value(function, target, &constraint, self.check_mode, self.engine),
                        None => self.function_expression(function),
                    };
                    self.compatible(&value, &constraint, function.span);
                    return;
                }
                self.error(
                    "LUA2021",
                    name.span,
                    "global function definitions require the global declaration integration",
                );
                return;
            }
        }
        let expected = self.assignment_target_context(target);
        let ty = if let (Some(native), Some(expected)) = (self.native, expected.as_ref()) {
            native.source_named_function_value(function, target, expected, self.check_mode, self.engine)
        } else { self.function_expression(function) };
        self.assign_values(std::slice::from_ref(target), vec![(ty, function.span)]);
    }

    fn function_body(&mut self, function: &Function, signature: &FunctionType) {
        // Checking a declaration must not execute its writes. Mutable captures
        // also cannot inherit narrowing from the time the closure was created.
        let outer_locals = self.locals.clone();
        let outer_fields = std::mem::take(&mut self.assigned_fields);
        let outer_return = self.return_type.replace((*signature.returns).clone());
        for binding in self.locals.values_mut() {
            binding.value = binding.constraint.clone();
        }
        let receiver = self.bound.implicit_receiver(function.span);
        if let Some(binding) = receiver {
            let param = &signature.params[0];
            let ty = if param.optional { Type::Union(vec![param.ty.clone(), Type::Nil]) } else { param.ty.clone() };
            self.locals.insert(binding, Binding { constraint: ty.clone(), initial: ty.clone(), value: ty.clone(), evolves_from_nil: false });
            self.record_semantic_binding(binding, &ty);
        }
        for (name, param) in function.params.iter().zip(signature.params.iter().skip(usize::from(receiver.is_some()))) {
            let binding = self
                .bound
                .binding_at(name.span.start)
                .expect("bound parameter");
            let ty = if param.optional && !untyped_parameters(function) {
                Type::Union(vec![param.ty.clone(), Type::Nil])
            } else {
                param.ty.clone()
            };
            self.locals.insert(
                binding,
                Binding {
                    constraint: ty.clone(),
                    initial: ty.clone(),
                    value: ty.clone(),
                    evolves_from_nil: false,
                },
            );
            self.record_binding(name, binding, &ty);
        }
        let returned = self.statements(&function.body);
        if !returned && !self.engine.allows_implicit_return(&signature.returns) {
            let origin = function
                .annotations
                .returns
                .as_ref()
                .map_or(function.span, |annotation| annotation.span);
            let (code, message) = if matches!(*signature.returns, Type::Never) {
                (
                    "TS2534",
                    "A function returning never cannot have a reachable end point.",
                )
            } else {
                (
                    "TS2355",
                    "A function whose declared type is neither nil, void, nor any must return a value.",
                )
            };
            self.error(code, origin, message);
        }
        self.locals = outer_locals;
        self.assigned_fields = outer_fields;
        self.return_type = outer_return;
    }

    pub(super) fn return_statement<E: std::borrow::Borrow<Expr>>(&mut self, values: &[E], span: Span) {
        let expected_pack = self.return_pack.clone();
        let context = expected_pack.clone().or_else(|| self.contextual_function
            .map(|context| self.engine.contextual_return_pack(&context)));
        let pack = self.return_expression_pack(values, span, context.as_ref());
        if let Some(expected) = expected_pack {
            if !self.engine.pack_assignable(&pack, &expected) {
                let origin = self.return_contract_origin(span).unwrap_or_else(|| {
                    self.return_pack_error_origin(values, span, &pack, &expected)
                });
                self.error_packs(
                    "TS2322",
                    origin,
                    "Lua result pack is not assignable to the declared result pack. Actual: {}. Expected: {}.",
                    &[&pack, &expected],
                );
            }
        } else if let Some(expected) = self.return_type.clone() {
            let value = self.engine.pack_slot(&pack, 0);
            self.compatible(
                &value,
                &expected,
                self.return_contract_origin(span).unwrap_or_else(|| values.first().map_or(span, |value| value.borrow().span)),
            );
        }
        self.return_values.push(self.engine.pack_slot(&pack, 0));
        self.return_packs.push(pack);
    }

    pub(super) fn call_expression(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
        discard: bool,
    ) -> Type {
        self.call_expression_with_expected(callee, args, span, discard, None)
    }

    pub(super) fn call_expression_with_expected(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
        _discard: bool,
        expected: Option<&Type>,
    ) -> Type {
        if let Some(pack) = self.native_require_pack(callee, args, span) { return self.engine.pack_slot(&pack, 0); }
        let callee_type = self.expression(callee);
        let arguments = self.call_arguments(args);
        let call = self
            .native
            .map(|native| native.expression_key_for_span(span));
        let result =
            self.engine
                .call_native_arguments_scalar_in_frame(&callee_type, call, &arguments, &[], expected,
                    luanalysis_engine::NativeCallerFrame { contextual: self.contextual_function, captured: self.captured_function });
        let origins: Vec<_> = args.iter().map(|argument| argument.span).collect();
        let checked = result.errors.is_empty();
        let ty = self.call_result(result, &callee_type, &origins, callee.span, span);
        if checked && let Some(pack) = self.standard_pattern_call(callee, args) { return self.engine.pack_slot(&pack, 0) }
        if self.native.is_some_and(|native| native.is_class_definition_initializer(span)) { return ty }
        self.setmetatable_result(callee, &callee_type, args, ty)
    }

    /// The result of a standard pattern function, `match`, `gmatch` or `find`, called with a
    /// literal pattern: what it matches is a string, and an empty capture, `()`, a position.
    /// `None` leaves the declared result: the pattern is not literal or `find` may be plain.
    pub(super) fn pattern_function_result(&mut self, function: &str, pattern: Option<&Expr>, plain: Option<&Expr>) -> Option<ValuePack> {
        use luanalysis_engine::{PackAlternative, PackSlot};
        let mut pattern = pattern?;
        while let ExprKind::Parenthesized(inner) = &pattern.kind { pattern = inner; }
        let ExprKind::String(bytes) = &pattern.kind else { return None };
        let plain = match plain.map(|plain| &plain.kind) {
            None | Some(ExprKind::Nil | ExprKind::Boolean(false)) => false,
            Some(ExprKind::Boolean(true)) if function == "find" => true,
            Some(_) => return None,
        };
        let captures = if plain { Vec::new() } else { crate::pattern_captures::captures(bytes)? };
        let captured: Vec<Type> = captures.iter().map(|capture| match capture {
            crate::pattern_captures::Capture::String => Type::String,
            crate::pattern_captures::Capture::Position => Type::Number,
        }).collect();
        let row = |types: Vec<Type>| PackAlternative { fixed: types.into_iter().map(PackSlot::required).collect(), tail: None };
        let matched = if captured.is_empty() { vec![Type::String] } else { captured.clone() };
        Some(match function {
            "match" => ValuePack::Alternatives(vec![row(vec![Type::Nil]), row(matched)]),
            "find" => ValuePack::Alternatives(vec![row(vec![Type::Nil]), row([Type::Number, Type::Number].into_iter().chain(captured).collect())]),
            "gmatch" => ValuePack::single(self.engine.pack_function(&PackFunctionType {
                params: Vec::new(), rest: None, type_parameters: Vec::new(), returns: ValuePack::Alternatives(vec![row(matched)]),
            })),
            _ => return None,
        })
    }

    /// `string.match(s, pattern)` and the like through the standard `string` global.
    fn standard_pattern_call(&mut self, callee: &Expr, args: &[Expr]) -> Option<ValuePack> {
        let ExprKind::Field { base, name } = &callee.kind else { return None };
        let ExprKind::Name(library) = &base.kind else { return None };
        if library.text != "string" || self.bound.binding_at(library.span.start).is_some() { return None }
        self.pattern_function_result(&name.text, args.get(1), args.get(3))
    }

    /// Lua's `setmetatable` returns the table it is given. A table that a constructor names keeps
    /// that identity through the call, so stores after it extend the table as they would directly.
    fn setmetatable_result(&mut self, callee: &Expr, callee_type: &Type, args: &[Expr], ty: Type) -> Type {
        let Some(first) = args.first() else { return ty };
        if self.engine.runtime_table_declaration(&ty).is_some() { return ty }
        // The same syntactic `setmetatable` that attributes the stores to that table's schema, or
        // another alias the checker proves is the standard function.
        let syntactic = self.native.is_some_and(|native| native.names_standard_setmetatable(callee, 0));
        let standard = syntactic || matches!(&callee.kind, ExprKind::Name(name) if self.bound.binding_at(name.span.start).is_some()) && {
            let operation = self.native_standard_operation(callee);
            matches!(operation, Some(luanalysis_engine::NativeStandardOperation::Standard(site)) if site.kind == luanalysis_engine::NativeIntrinsicKind::SetMetatable)
                || self.engine.function_intrinsic(callee_type).is_some_and(|site| site.kind == luanalysis_engine::NativeIntrinsicKind::SetMetatable)
        };
        if !standard { return ty }
        // A class's definition table as the metatable makes the table an instance of the class.
        if let Some(metatable) = args.get(1)
            && matches!(metatable.kind, ExprKind::Name(_))
        {
            let metatable = self.expression(metatable);
            // A table's own schema, which a captured runtime table reads as, is not a class.
            if self.engine.runtime_table_schema(&metatable).is_none()
                && let Some(declaration) = self.engine.named_object_declaration(&metatable)
                && let Ok(class) = self.engine.named_declared_type(declaration)
            {
                return class;
            }
        }
        let Some(declaration) = self.native.and_then(|native| native.runtime_table_owner_declaration(first)) else { return ty };
        self.engine.runtime_table_value(declaration, &ty).unwrap_or(ty)
    }

    pub(super) fn call_result(
        &mut self,
        result: CallResult,
        callee_type: &Type,
        argument_spans: &[Span],
        callee_span: Span,
        span: Span,
    ) -> Type {
        for error in result.errors {
            match error {
                CallError::NotCallable => self.error_types("TS2349", callee_span, "Type '{}' has no call signatures.", &[callee_type]),
                CallError::Arity { actual, minimum, maximum } => {
                    let expected = match maximum {
                        Some(maximum) if maximum == minimum => minimum.to_string(),
                        Some(maximum) => format!("{minimum}-{maximum}"),
                        None => format!("at least {minimum}"),
                    };
                    self.error("TS2554", span, format!("Expected {expected} arguments, but got {actual}."));
                }
                CallError::PackArity { actual_minimum, actual_maximum, minimum, maximum, .. } => {
                    let expected = arity_range(minimum, maximum);
                    let actual = arity_range(actual_minimum, actual_maximum);
                    self.error("TS2554", span, format!("Expected {expected} arguments, but got {actual}."));
                }
                CallError::Argument { index, source, target } => {
                    let origin = argument_spans.get(index).or_else(|| argument_spans.last()).copied().unwrap_or(span);
                    self.error_types("TS2345", origin, "Argument of type '{}' is not assignable to parameter of type '{}'.", &[&source, &target]);
                }
                CallError::TypeArgumentArity { actual, minimum, maximum } => {
                    let expected = if maximum == minimum { minimum.to_string() } else { format!("{minimum}-{maximum}") };
                    self.error("TS2558", callee_span, format!("Expected {expected} type arguments, but got {actual}."));
                }
                CallError::TypeArgument { source, target, .. } => {
                    self.error_types("TS2344", callee_span, "Type '{}' does not satisfy the constraint '{}'.", &[&source, &target]);
                }
                unsupported => self.error("LUA2018", callee_span, format!("native call form {unsupported:?} requires a later call-checking integration")),
            }
        }
        result.return_type
    }
}

fn arity_range(minimum: usize, maximum: Option<usize>) -> String {
    match maximum {
        Some(maximum) if maximum == minimum => minimum.to_string(),
        Some(maximum) => format!("{minimum}-{maximum}"),
        None => format!("at least {minimum}"),
    }
}

/// A function written with no parameter types, neither `---@param` nor `---@type`. As TypeScript
/// reads a JavaScript function without JSDoc, a call can omit its parameters, for which Lua
/// passes nil: that makes them optional to callers, not nil in the body.
pub(crate) fn untyped_parameters(function: &Function) -> bool {
    function.annotations.params.is_empty() && function.annotations.type_annotation.is_none()
}
