//! Native function annotations and scalar call integration. Signature identity
//! and lazy return inference belong to the native declaration provider; Lua
//! result packs remain a separate integration gate.

use super::{Binding, Checker, describe};
use luanalysis_engine::{CallError, FunctionType, Parameter, Type};
use luanalysis_syntax::{Expr, Function, Name, Span};
use std::collections::HashSet;

impl Checker<'_> {
    pub(super) fn function_annotation(
        &mut self,
        function: &luanalysis_syntax::FunctionType,
    ) -> Type {
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
                        self.error(
                            "TS7006",
                            param.name.span,
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
        let returns = match function.returns.as_slice() {
            [] => Type::Void,
            [ty] => self.annotation(ty),
            _ => {
                self.error(
                    "LUA2016",
                    function.span,
                    "multiple function results require the pending result-pack integration",
                );
                Type::Any
            }
        };
        Type::Function(FunctionType {
            params,
            returns: Box::new(returns),
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
                match self.annotation(&annotation.types[0]) {
                    Type::Function(signature) => Some(signature),
                    _ => {
                        self.error(
                            "LUA2010",
                            annotation.span,
                            "a function definition requires a function type",
                        );
                        None
                    }
                }
            });
        let mut seen = HashSet::new();
        for tag in &function.annotations.params {
            if !seen.insert(&tag.name.text) {
                self.error(
                    "LUA2011",
                    tag.name.span,
                    format!("duplicate annotation for parameter '{}'", tag.name.text),
                );
            }
            if !function
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
        let mut params = Vec::new();
        let mut optional_seen = false;
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
                .and_then(|context| context.params.get(index))
            {
                (expected.ty.clone(), expected.optional)
            } else {
                self.error(
                    "TS7006",
                    param.span,
                    format!("Parameter '{}' implicitly has an 'any' type.", param.text),
                );
                (Type::Any, false)
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
                [ty] => Some(self.annotation(ty)),
                _ => {
                    self.error(
                        "LUA2016",
                        annotation.span,
                        "multiple function results require the pending result-pack integration",
                    );
                    Some(Type::Any)
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
            Type::NativeFunction(native.key_for(function))
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
            return Type::NativeFunction(native.key_for(function));
        }
        let signature = self.function_signature(function);
        self.function_body(function, &signature);
        Type::Function(signature)
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
        for (name, param) in function.params.iter().zip(&signature.params) {
            let binding = self
                .bound
                .binding_at(name.span.start)
                .expect("bound parameter");
            let ty = if param.optional {
                Type::Union(vec![param.ty.clone(), Type::Nil])
            } else {
                param.ty.clone()
            };
            self.locals.insert(
                binding,
                Binding {
                    constraint: ty.clone(),
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

    pub(super) fn return_statement(&mut self, values: &[Expr], span: Span) {
        if values.len() > 1 {
            self.error(
                "LUA2016",
                span,
                "multiple function results require the pending result-pack integration",
            );
        }
        let mut value = Type::Nil;
        let expected = self.return_type.clone();
        for (index, expr) in values.iter().enumerate() {
            let ty = self.expression_with_expected(
                expr,
                (index == 0).then_some(expected.as_ref()).flatten(),
            );
            if index == 0 {
                value = ty;
            }
        }
        if let Some(expected) = self.return_type.clone() {
            self.compatible(
                &value,
                &expected,
                values.first().map_or(span, |value| value.span),
            );
        }
        self.return_values.push(value);
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
        discard: bool,
        expected: Option<&Type>,
    ) -> Type {
        let callee_type = self.expression(callee);
        let arguments: Vec<_> = args.iter().map(|arg| self.expression(arg)).collect();
        let spans: Vec<_> = args.iter().map(|arg| arg.span).collect();
        self.call_types_with_expected(
            &callee_type,
            &arguments,
            &spans,
            callee.span,
            span,
            discard,
            expected,
        )
    }

    pub(super) fn call_types_with_expected(
        &mut self,
        callee_type: &Type,
        arguments: &[Type],
        argument_spans: &[Span],
        callee_span: Span,
        span: Span,
        discard: bool,
        expected: Option<&Type>,
    ) -> Type {
        let result = self
            .engine
            .call_with_type_arguments(callee_type, arguments, &[], expected);
        for error in result.errors {
            match error {
                CallError::NotCallable => self.error("TS2349", callee_span, format!("Type '{}' has no call signatures.", describe(callee_type))),
                CallError::Arity { actual, minimum, maximum } => {
                    let expected = match maximum {
                        Some(maximum) if maximum == minimum => minimum.to_string(),
                        Some(maximum) => format!("{minimum}-{maximum}"),
                        None => format!("at least {minimum}"),
                    };
                    self.error("TS2554", span, format!("Expected {expected} arguments, but got {actual}."));
                }
                CallError::Argument { index, source, target } => {
                    self.error("TS2345", argument_spans.get(index).copied().unwrap_or(span), format!("Argument of type '{}' is not assignable to parameter of type '{}'.", describe(&source), describe(&target)));
                }
                CallError::TypeArgumentArity { actual, minimum, maximum } => {
                    let expected = if maximum == minimum { minimum.to_string() } else { format!("{minimum}-{maximum}") };
                    self.error("TS2558", callee_span, format!("Expected {expected} type arguments, but got {actual}."));
                }
                CallError::TypeArgument { source, target, .. } => {
                    self.error("TS2344", callee_span, format!("Type '{}' does not satisfy the constraint '{}'.", describe(&source), describe(&target)));
                }
                unsupported => self.error("LUA2018", callee_span, format!("native call form {unsupported:?} requires a later call-checking integration")),
            }
        }
        // `void` does not establish a Lua result count. Its zero/multiple-result
        // expansion must be represented before such calls can be used as values.
        if !discard && contains_void(&result.return_type) {
            self.error("LUA2016", span, "void call results require the pending Lua result-pack integration; only discarded void calls are supported");
        }
        result.return_type
    }
}

fn contains_void(ty: &Type) -> bool {
    match ty {
        Type::Void => true,
        Type::Union(types) | Type::Intersection(types) => types.iter().any(contains_void),
        _ => false,
    }
}
