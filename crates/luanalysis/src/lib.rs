//! Original-source checking for the first native Lua subset.
//!
//! Lua syntax and straight-line bindings are owned here. Type construction,
//! literal widening, member lookup, freshness and assignability use the pinned
//! Bun core. Generic inference, branch flow and project services come later.

pub mod bind;
mod functions;

use bind::{BindingId, BoundFile};
use luanalysis_engine::{Engine, Property, Type, TypeQueries};
use luanalysis_syntax::{
    Expr, ExprKind, Name, Span, Stmt, StmtKind, TypeAnnotation, TypeExpr, TypeExprKind,
};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingInfo {
    pub binding: BindingId,
    pub name: String,
    pub span: Span,
    pub ty: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeInfo {
    pub span: Span,
    pub ty: String,
}

#[derive(Clone, Debug, Default)]
pub struct CheckReport {
    pub diagnostics: Vec<Diagnostic>,
    pub bindings: Vec<BindingInfo>,
    pub types: Vec<TypeInfo>,
    pub bound: Option<BoundFile>,
}

impl CheckReport {
    /// Returns an owned report's displayed type at an original UTF-8 byte offset.
    pub fn type_at(&self, byte_offset: usize) -> Option<&str> {
        if let Some(occurrence) = self
            .bound
            .as_ref()
            .and_then(|bound| bound.occurrence_at(byte_offset))
        {
            if matches!(
                occurrence.kind,
                bind::OccurrenceKind::Declaration | bind::OccurrenceKind::Annotation
            ) {
                let binding = occurrence.binding?;
                return self
                    .bindings
                    .iter()
                    .find(|info| info.binding == binding)
                    .map(|info| info.ty.as_str());
            }
        }
        if let Some(info) = self
            .types
            .iter()
            .filter(|info| info.span.start <= byte_offset && byte_offset < info.span.end)
            .min_by_key(|info| info.span.end - info.span.start)
        {
            return Some(&info.ty);
        }
        let binding = self.bound.as_ref()?.binding_at(byte_offset)?;
        self.bindings
            .iter()
            .find(|info| info.binding == binding)
            .map(|info| info.ty.as_str())
    }
}

#[derive(Clone)]
struct Binding {
    constraint: Type,
    value: Type,
    evolves_from_nil: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Reference {
    binding: BindingId,
    path: Vec<String>,
}

impl Reference {
    fn is_strict_ancestor_of(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.path.len() < other.path.len()
            && other.path.starts_with(&self.path)
    }
}

enum Write {
    Local(BindingId, Type),
    Field(Reference, Type),
}

struct Checker<'a> {
    engine: &'a mut dyn TypeQueries,
    bound: &'a BoundFile,
    locals: HashMap<BindingId, Binding>,
    assigned_fields: HashMap<Reference, Type>,
    report: CheckReport,
    return_type: Option<Type>,
}

/// Checks the documented native subset without generating TypeScript source.
pub fn check(source: &str) -> CheckReport {
    let file = luanalysis_syntax::parse(source);
    if !file.diagnostics.is_empty() {
        return CheckReport {
            diagnostics: file
                .diagnostics
                .into_iter()
                .map(|error| Diagnostic {
                    code: "LUA1001",
                    message: error.message,
                    span: error.span,
                })
                .collect(),
            bindings: Vec::new(),
            types: Vec::new(),
            bound: None,
        };
    }
    let bound = bind::bind(&file);
    let mut report = Engine::new().with_session(|engine| {
        let mut checker = Checker {
            engine,
            bound: &bound,
            locals: HashMap::new(),
            assigned_fields: HashMap::new(),
            report: CheckReport::default(),
            return_type: None,
        };
        checker.statements(&file.statements);
        checker.report.diagnostics.sort_by(|a, b| {
            (a.span.start, a.span.end, a.code, &a.message).cmp(&(
                b.span.start,
                b.span.end,
                b.code,
                &b.message,
            ))
        });
        checker.report.diagnostics.dedup();
        checker
            .report
            .bindings
            .sort_by_key(|binding| binding.span.start);
        checker
            .report
            .types
            .sort_by_key(|info| (info.span.start, info.span.end));
        checker.report
    });
    report.bound = Some(bound);
    report
}

impl Checker<'_> {
    /// Returns true when this straight-line block terminates with a return or
    /// an explicitly declared nonreturning call.
    fn statements(&mut self, statements: &[Stmt]) -> bool {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local {
                    names,
                    values,
                    annotation,
                } => {
                    self.local(names, values, annotation.as_ref());
                }
                StmtKind::Assign {
                    targets,
                    values,
                    annotation,
                } => {
                    self.assign(targets, values, annotation.as_ref());
                }
                StmtKind::Block { body, .. } => {
                    if self.statements(body) {
                        return true;
                    }
                }
                StmtKind::LocalFunction { name, function } => self.local_function(name, function),
                StmtKind::Return(values) => {
                    self.return_statement(values, statement.span);
                    return true;
                }
                StmtKind::Call(expr) => {
                    if let ExprKind::Call { callee, args } = &expr.kind {
                        if matches!(
                            self.call_expression(callee, args, expr.span, true),
                            Type::Never
                        ) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn error(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.report.diagnostics.push(Diagnostic {
            code,
            span,
            message: message.into(),
        });
    }

    fn annotation(&mut self, ty: &TypeExpr) -> Type {
        match &ty.kind {
            TypeExprKind::Named(name) => match name.as_str() {
                "any" => Type::Any,
                "unknown" => Type::Unknown,
                "never" => Type::Never,
                "nil" => Type::Nil,
                "boolean" => Type::Boolean,
                "number" => Type::Number,
                "string" => Type::String,
                "void" => Type::Void,
                _ => {
                    self.error(
                        "LUA2001",
                        ty.span,
                        format!("type {name:?} is not supported in this slice"),
                    );
                    Type::Any
                }
            },
            TypeExprKind::Union(types) => {
                Type::Union(types.iter().map(|ty| self.annotation(ty)).collect())
            }
            TypeExprKind::Record(fields) => Type::Shape(
                fields
                    .iter()
                    .map(|field| Property::new(field.name.text.clone(), self.annotation(&field.ty)))
                    .collect(),
            ),
            TypeExprKind::Parenthesized(ty) => self.annotation(ty),
            TypeExprKind::Function(function) => self.function_annotation(function),
            TypeExprKind::Error => Type::Any,
        }
    }

    fn expression(&mut self, expr: &Expr) -> Type {
        let ty = match &expr.kind {
            ExprKind::Nil => Type::Nil,
            ExprKind::Boolean(value) => Type::BooleanLiteral(*value),
            ExprKind::Number(spelling) => match number(spelling) {
                Some(value) => Type::NumberLiteral(value),
                None => {
                    self.error("LUA2002", expr.span, "invalid numeric literal");
                    Type::Any
                }
            },
            ExprKind::String(value) => Type::StringLiteral(value.clone()),
            ExprKind::Name(name) => match self
                .bound
                .binding_at(name.span.start)
                .and_then(|id| self.locals.get(&id))
            {
                Some(binding) => binding.value.clone(),
                None => {
                    self.error(
                        "TS2304",
                        name.span,
                        format!("Cannot find name '{}'.", name.text),
                    );
                    Type::Any
                }
            },
            ExprKind::Parenthesized(value) => self.expression(value),
            ExprKind::Table(fields) => {
                let mut properties: Vec<Property> = Vec::new();
                let mut duplicate = false;
                for field in fields {
                    let value = self.expression(&field.value);
                    let property =
                        Property::new(field.name.text.clone(), self.engine.widen(&value));
                    if properties.iter().any(|item| item.name == property.name) {
                        self.error("LUA2007", field.name.span, format!("duplicate table field '{}' has undefined assignment order in Lua 5.4 and is not supported", field.name.text));
                        duplicate = true;
                    } else {
                        properties.push(property);
                    }
                }
                if duplicate {
                    Type::Any
                } else {
                    Type::FreshShape(properties)
                }
            }
            ExprKind::Field { base, name } => {
                let receiver = self.expression(base);
                if matches!(receiver, Type::Any) {
                    return Type::Any;
                }
                if let Some(ty) = self.engine.property_type(&receiver, &name.text) {
                    self.reference(expr)
                        .and_then(|reference| self.assigned_fields.get(&reference).cloned())
                        .unwrap_or(ty)
                } else {
                    self.error(
                        "TS2339",
                        name.span,
                        format!(
                            "Property '{}' does not exist on type '{}'.",
                            name.text,
                            describe(&receiver)
                        ),
                    );
                    Type::Any
                }
            }
            ExprKind::Function(function) => self.function_expression(function),
            ExprKind::Call { callee, args } => self.call_expression(callee, args, expr.span, false),
            ExprKind::Error => Type::Any,
        };
        self.report.types.push(TypeInfo {
            span: expr.span,
            ty: describe(&ty),
        });
        ty
    }

    fn compatible(&mut self, value: &Type, target: &Type, origin: Span) -> bool {
        let relation = self.engine.compare(value, target);
        if !relation.assignable {
            self.error(
                "TS2322",
                origin,
                format!(
                    "Type '{}' is not assignable to type '{}'.",
                    describe(value),
                    describe(target)
                ),
            );
        }
        relation.assignable
    }

    fn local(&mut self, names: &[Name], values: &[Expr], annotation: Option<&TypeAnnotation>) {
        // Resolve every initializer before introducing any name in this list.
        let values: Vec<_> = values
            .iter()
            .map(|value| (self.expression(value), value.span))
            .collect();
        let annotations: Vec<_> = annotation.map_or_else(Vec::new, |annotation| {
            annotation
                .types
                .iter()
                .map(|ty| self.annotation(ty))
                .collect()
        });
        if annotations.len() > names.len() {
            self.error(
                "LUA2003",
                annotation.unwrap().span,
                "more annotated types than declared names",
            );
        }
        for (index, name) in names.iter().enumerate() {
            let (value, origin) = values.get(index).cloned().unwrap_or((Type::Nil, name.span));
            let widened = self.engine.widen(&value);
            let constraint = annotations
                .get(index)
                .cloned()
                .unwrap_or_else(|| widened.clone());
            self.compatible(&value, &constraint, origin);
            let narrowed = self.engine.assignment_value(&constraint, &value);
            let id = self
                .bound
                .binding_at(name.span.start)
                .expect("bound Lua declaration");
            self.report.bindings.push(BindingInfo {
                binding: id,
                name: name.text.clone(),
                span: name.span,
                ty: describe(&constraint),
            });
            self.locals.insert(
                id,
                Binding {
                    constraint,
                    value: narrowed,
                    evolves_from_nil: annotations.get(index).is_none()
                        && matches!(widened, Type::Nil | Type::Undefined),
                },
            );
        }
    }

    fn assign(&mut self, targets: &[Expr], values: &[Expr], annotation: Option<&TypeAnnotation>) {
        if let Some(annotation) = annotation {
            self.error(
                "LUA2004",
                annotation.span,
                "@type casts on assignments are not implemented yet",
            );
        }
        let values: Vec<_> = values
            .iter()
            .map(|value| (self.expression(value), value.span))
            .collect();
        // Validate all destinations against the state before committing writes.
        let mut writes = Vec::new();
        for (index, target) in targets.iter().enumerate() {
            let (value, origin) = values
                .get(index)
                .cloned()
                .unwrap_or((Type::Nil, target.span));
            match &target.kind {
                ExprKind::Name(name) => {
                    let Some(id) = self.bound.binding_at(name.span.start) else {
                        self.error(
                            "TS2304",
                            name.span,
                            format!("Cannot find name '{}'.", name.text),
                        );
                        continue;
                    };
                    let Some(binding) = self.locals.get(&id) else {
                        continue;
                    };
                    let constraint = binding.constraint.clone();
                    if binding.evolves_from_nil && !matches!(value, Type::Nil | Type::Undefined) {
                        self.error("LUA2005", origin, "inference for a nil-initialized local requires the pending flow integration");
                    } else if self.compatible(&value, &constraint, origin) {
                        writes.push(Write::Local(
                            id,
                            self.engine.assignment_value(&constraint, &value),
                        ));
                    }
                }
                ExprKind::Field { base, name } => {
                    let receiver = self.expression(base);
                    if matches!(receiver, Type::Any) {
                        continue;
                    }
                    let Some(constraint) = self.engine.property_type(&receiver, &name.text) else {
                        self.error(
                            "TS2339",
                            name.span,
                            format!(
                                "Property '{}' does not exist on type '{}'.",
                                name.text,
                                describe(&receiver)
                            ),
                        );
                        continue;
                    };
                    if self.compatible(&value, &constraint, origin) {
                        if let Some(reference) = self.reference(target) {
                            writes.push(Write::Field(
                                reference,
                                self.engine.assignment_value(&constraint, &value),
                            ));
                        }
                    }
                }
                _ => self.error("LUA2006", target.span, "unsupported assignment destination"),
            }
        }
        // Every field destination captured its old receiver before any stores.
        // A root/member replacement anywhere in this assignment makes a fact
        // about that old table unusable through its former source path. This
        // holds even when the replacement is stored before the field write.
        let replaced: Vec<_> = writes
            .iter()
            .filter_map(|write| match write {
                Write::Local(id, _) => self.locals.get(id).map(|_| Reference {
                    binding: *id,
                    path: Vec::new(),
                }),
                Write::Field(reference, _) => Some(reference.clone()),
            })
            .collect();
        // Match the initial dialect's Lua 5.4 implementation: stores unwind
        // right-to-left, so a repeated destination retains its leftmost value.
        for write in writes.into_iter().rev() {
            match write {
                Write::Local(id, value) => {
                    if let Some(binding) = self.locals.get_mut(&id) {
                        self.assigned_fields
                            .retain(|reference, _| reference.binding != id);
                        binding.value = value;
                    }
                }
                Write::Field(reference, value) => {
                    self.assigned_fields.retain(|previous, _| {
                        previous.binding != reference.binding
                            || !previous.path.starts_with(&reference.path)
                    });
                    if !replaced
                        .iter()
                        .any(|ancestor| ancestor.is_strict_ancestor_of(&reference))
                    {
                        self.assigned_fields.insert(reference, value);
                    }
                }
            }
        }
    }

    fn reference(&self, expr: &Expr) -> Option<Reference> {
        match &expr.kind {
            ExprKind::Name(name) => self.bound.binding_at(name.span.start).map(|id| Reference {
                binding: id,
                path: Vec::new(),
            }),
            ExprKind::Parenthesized(inner) => self.reference(inner),
            ExprKind::Field { base, name } => {
                let mut reference = self.reference(base)?;
                reference.path.push(name.text.clone());
                Some(reference)
            }
            _ => None,
        }
    }
}

fn number(spelling: &str) -> Option<f64> {
    let (sign, unsigned) = spelling
        .strip_prefix('-')
        .map_or((1.0, spelling), |unsigned| (-1.0, unsigned));
    let Some(hex) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    else {
        return spelling.parse().ok();
    };
    let (mantissa, exponent) = hex
        .split_once(['p', 'P'])
        .map_or((hex, 0), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().unwrap_or(i32::MAX))
        });
    if exponent == i32::MAX {
        return None;
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let mut value = 0.0;
    for digit in whole.chars() {
        value = value * 16.0 + digit.to_digit(16)? as f64;
    }
    let mut place = 1.0 / 16.0;
    for digit in fraction.chars() {
        value += digit.to_digit(16)? as f64 * place;
        place /= 16.0;
    }
    Some(sign * value * 2.0_f64.powi(exponent))
}

/// Lua-oriented presentation of resolved type descriptions.
pub fn describe(ty: &Type) -> String {
    match ty {
        Type::Any => "any".into(),
        Type::Unknown => "unknown".into(),
        Type::Never => "never".into(),
        Type::Nil | Type::Undefined => "nil".into(),
        Type::Void => "void".into(),
        Type::Boolean => "boolean".into(),
        Type::Number => "number".into(),
        Type::String => "string".into(),
        Type::BooleanLiteral(value) => value.to_string(),
        Type::NumberLiteral(value) => value.to_string(),
        Type::StringLiteral(value) => {
            let mut result = String::from("\"");
            for byte in value {
                match byte {
                    b'"' => result.push_str("\\\""),
                    b'\\' => result.push_str("\\\\"),
                    32..=126 => result.push(*byte as char),
                    _ => result.push_str(&format!("\\x{byte:02x}")),
                }
            }
            result.push('"');
            result
        }
        Type::Union(types) => types.iter().map(describe).collect::<Vec<_>>().join(" | "),
        Type::Intersection(types) => types.iter().map(describe).collect::<Vec<_>>().join(" & "),
        Type::Shape(properties) | Type::FreshShape(properties) => format!(
            "{{ {} }}",
            properties
                .iter()
                .map(|property| {
                    format!(
                        "{}{}: {}",
                        property.name,
                        if property.optional { "?" } else { "" },
                        describe(&property.ty)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Function(function) => format!(
            "fun({}): {}",
            function
                .params
                .iter()
                .map(|parameter| {
                    format!(
                        "{}{}: {}",
                        parameter.name,
                        if parameter.optional { "?" } else { "" },
                        describe(&parameter.ty)
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
            describe(&function.returns)
        ),
    }
}

/// One-based original-source line and Unicode scalar column for CLI output.
pub fn location(source: &str, span: Span) -> (usize, usize) {
    let mut offset = span.start.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let mut line = 1;
    let mut column = 1;
    let mut chars = source[..offset].chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                line += 1;
                column = 1;
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\n' => {
                line += 1;
                column = 1;
            }
            _ => column += 1,
        }
    }
    (line, column)
}
