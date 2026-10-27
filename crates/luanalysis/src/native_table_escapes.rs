//! Where a table constructor without a contract is passed to a call or given a written
//! `---@type`: code with that declared view may write wider values into the table's literal
//! fields. Such a field has the wider type wherever the table is used, so that its type is
//! consistent (`escaped_field_type`). A read-only field of the view writes nothing.
use crate::Checker;
use crate::collections::{HashMap, HashSet};
use crate::flow;
use crate::native_functions::NativeRegistry;
use crate::native_packs::AnnotationPack;
use luanalysis_engine::{
    CapturedFunctionContext, ContextualFunctionContext, LuaKey, NativeCheckMode, NativeExpressionKey, TableKey, Type,
};
use luanalysis_syntax::{Expr, ExprKind, Span, StmtKind, TableFieldKind, TypeAnnotation};

/// Constructors nested in an escaping table follow its escapes this deep.
const MAX_NESTING: usize = 4;

#[derive(Clone, Debug)]
enum EscapeTarget {
    /// The parameter at `index` of `call`, counting a method call's receiver as parameter 0.
    /// A function of the table itself (`own`) is known only from what it writes: its type
    /// is the table's.
    Argument { call: flow::EvaluationId, expression: NativeExpressionKey, index: usize, own: bool },
    /// A value given a written `---@type`, at its `slot`.
    Annotation { annotation: TypeAnnotation, slot: usize, value: NativeExpressionKey },
}

#[derive(Clone, Debug)]
struct TableEscape {
    target: EscapeTarget,
    /// The keys from the escaping table down to this constructor's table.
    path: Vec<LuaKey>,
}

/// Each uncontracted constructor's escapes, by its span. Syntactic: computed once per registry.
#[derive(Default)]
pub(crate) struct TableEscapes {
    constructors: HashMap<(usize, usize), Vec<TableEscape>>,
}

/// A constructor's named literal field, in the frames it was evaluated in.
pub(crate) type EscapeWideningKey =
    ((usize, usize), Vec<u8>, Option<ContextualFunctionContext>, Option<CapturedFunctionContext>);
/// A constructor, in the frames it was evaluated in.
pub(crate) type EscapeViewKey = ((usize, usize), Option<ContextualFunctionContext>, Option<CapturedFunctionContext>);

/// What an escape's declared view lets code write into a field.
enum FieldView {
    /// A writable field of this type.
    Field(Type),
    /// The view is `any` or `unknown`: what is written is known only from what a callee does.
    Unknown,
}

fn unparenthesized(mut expression: &Expr) -> &Expr {
    while let ExprKind::Parenthesized(inner) = &expression.kind {
        expression = inner;
    }
    expression
}

fn flow_key(key: &LuaKey) -> flow::Key {
    match key {
        LuaKey::String(bytes) => flow::Key::String(bytes.clone()),
        LuaKey::Integer(value) => flow::Key::Integer(*value),
        LuaKey::Number(value) => flow::Key::Float(value.to_bits()),
        LuaKey::Boolean(value) => flow::Key::Boolean(*value),
    }
}

impl NativeRegistry {
    pub(crate) fn table_escapes(&self) -> &TableEscapes {
        self.table_escapes.get_or_init(|| self.compute_table_escapes())
    }

    fn compute_table_escapes(&self) -> TableEscapes {
        let mut escapes = TableEscapes::default();
        let mut memo = HashMap::default();
        // A constructor written as the value is contextually typed there; only a table that
        // is also reachable by a name or path can be read again.
        let mut owner = |expression: &Expr| -> Option<NativeExpressionKey> {
            if matches!(unparenthesized(expression).kind, ExprKind::Table(_)) { return None; }
            let owner = self.syntactic_table_owner(expression, None, 0, &mut memo)?;
            self.runtime_tables.constructor(owner)?.is_lexically_uncontracted().then_some(owner)
        };
        let span = |owner: NativeExpressionKey| {
            let span = self.expressions().get(owner).expression.span;
            (span.start, span.end)
        };
        let program = &self.iterator_sources().program;
        for evaluation in &program.evaluations {
            let expression = &program.expression(evaluation.expression).expression;
            let (callee_root, first) = match &expression.kind {
                ExprKind::Call { callee, .. } if !self.names_standard_setmetatable(callee, 0) => {
                    let mut root = &**callee;
                    while let ExprKind::Field { base, .. } | ExprKind::Index { base, .. } = &unparenthesized(root).kind { root = base; }
                    (root, 0)
                }
                ExprKind::MethodCall { receiver, .. } => (&**receiver, 1),
                _ => continue,
            };
            let callee_owner = owner(callee_root);
            for (position, argument) in evaluation.operands.iter().skip(1).enumerate() {
                let argument = &program.expression(program.evaluation(*argument).expression).expression;
                let Some(table) = owner(argument) else { continue };
                escapes.constructors.entry(span(table)).or_default().push(TableEscape {
                    target: EscapeTarget::Argument {
                        call: evaluation.id,
                        expression: self.expressions().key_for_span(expression.span),
                        index: first + position,
                        own: Some(table) == callee_owner,
                    },
                    path: Vec::new(),
                });
            }
        }
        let mut statements = Vec::new();
        crate::native_runtime_tables::collect_statements(self.statements(), &mut statements);
        for (_, source) in self.expressions().sources() {
            if let ExprKind::Function(function) = &source.expression.kind {
                crate::native_runtime_tables::collect_statements(&function.body, &mut statements);
            }
        }
        let mut seen = HashSet::default();
        for statement in statements {
            if !seen.insert((statement.span.start, statement.span.end)) { continue; }
            let (StmtKind::Local { values, annotation: Some(annotation), .. }
                | StmtKind::Assign { values, annotation: Some(annotation), .. }) = &statement.kind else { continue };
            for (slot, value) in values.iter().enumerate() {
                let Some(table) = owner(value) else { continue };
                escapes.constructors.entry(span(table)).or_default().push(TableEscape {
                    target: EscapeTarget::Annotation { annotation: annotation.clone(), slot, value: self.expressions().key_for_span(value.span) },
                    path: Vec::new(),
                });
            }
        }
        // A table's escapes reach the constructors written as its fields.
        let mut pending: Vec<_> = escapes.constructors.iter()
            .map(|(&span, escapes)| (span, escapes.clone(), 0))
            .collect();
        while let Some((parent, inherited, depth)) = pending.pop() {
            if depth == MAX_NESTING { continue; }
            let key = self.expressions().key_for_span(Span::new(parent.0, parent.1));
            let Some(constructor) = self.runtime_tables.constructor(key) else { continue };
            for field in &constructor.fields {
                let key = match &field.kind {
                    TableFieldKind::Named(name) => LuaKey::String(name.text.as_bytes().to_vec()),
                    TableFieldKind::Keyed(key) => match crate::native_tables::literal_key(key) { Some(key) => key, None => continue },
                    TableFieldKind::Sequence { index } => LuaKey::Integer(i64::try_from(*index).expect("sequence index fits Lua integer")),
                };
                let value = unparenthesized(&field.value);
                if !matches!(value.kind, ExprKind::Table(_)) { continue; }
                let child = self.expressions().key_for_span(value.span);
                if !self.runtime_tables.constructor(child).is_some_and(|child| child.is_lexically_uncontracted()) { continue; }
                let nested: Vec<_> = inherited.iter().map(|escape| {
                    let mut escape = escape.clone();
                    escape.path.push(key.clone());
                    escape
                }).collect();
                escapes.constructors.entry((value.span.start, value.span.end)).or_default().extend(nested.iter().cloned());
                pending.push(((value.span.start, value.span.end), nested, depth + 1));
            }
        }
        escapes
    }
}

impl Checker<'_> {
    /// The type of the literal field `key` of the uncontracted constructor at `constructor`,
    /// when the table escapes to a declared view that may write a wider value there: the
    /// literal and each wider type such a view's field accepts.
    pub(crate) fn escaped_field_type(&mut self, constructor: Span, key: &TableKey, literal: &Type) -> Option<Type> {
        let native = self.native?;
        let TableKey::Literal(field @ LuaKey::String(name)) = key else { return None };
        let escapes = native.table_escapes().constructors.get(&(constructor.start, constructor.end))?;
        let cache = ((constructor.start, constructor.end), name.clone(), self.contextual_function, self.captured_function);
        if let Some(known) = native.escape_widenings.borrow().get(&cache) { return known.clone(); }
        let (views, complete) = self.escape_views(native, constructor, escapes)?;
        let mut wider = Vec::new();
        for (escape, views) in escapes.iter().zip(views.iter()) {
            for view in views {
                match self.escaped_field_view(view.clone(), &escape.path, key) {
                    Some(FieldView::Field(field)) => {
                        let field = self.present_field(literal, field);
                        if self.engine.is_assignable(literal, &field) && !self.engine.is_assignable(&field, literal) {
                            wider.push(field);
                        }
                    }
                    Some(FieldView::Unknown) => {
                        let EscapeTarget::Argument { call, index, .. } = &escape.target else { continue };
                        let path: Vec<_> = escape.path.iter().chain([field]).map(flow_key).collect();
                        if native.iterator_sources().call_may_write_parameter(*call, *index, &path) {
                            let base = self.engine.base_of_literal(literal);
                            if base != *literal { wider.push(base); }
                        }
                    }
                    None => {}
                }
            }
        }
        let result = (!wider.is_empty()).then(|| {
            wider.push(literal.clone());
            self.engine.union_reduced(&wider)
        });
        if complete { native.escape_widenings.borrow_mut().insert(cache, result.clone()); }
        result
    }

    /// The declared views each of a constructor's `escapes` gives its table, decided once for
    /// all its fields, and whether they are complete. None while they are being decided: a view
    /// that reads this table again sees its literals.
    fn escape_views(&mut self, native: &NativeRegistry, constructor: Span, escapes: &[TableEscape]) -> Option<(std::rc::Rc<Vec<Vec<Type>>>, bool)> {
        let cache = ((constructor.start, constructor.end), self.contextual_function, self.captured_function);
        if let Some(known) = native.escape_views.borrow().get(&cache) { return Some((std::rc::Rc::clone(known), true)); }
        if !native.escape_views_active.borrow_mut().insert(cache) { return None; }
        let mut complete = true;
        let views = escapes.iter().map(|escape| match &escape.target {
            EscapeTarget::Argument { own: true, .. } => vec![Type::Unknown],
            EscapeTarget::Argument { expression, index, .. } => {
                let call = &native.expressions().get(*expression).expression;
                let mut checker = native.checker_for_expression(*expression, NativeCheckMode::TYPE_ONLY, &mut *self.engine);
                let callee = match &call.kind {
                    ExprKind::Call { callee, .. } => Some(checker.expression(callee)),
                    ExprKind::MethodCall { receiver, name, .. } => {
                        let receiver = checker.expression(receiver);
                        checker.engine.property_type(&receiver, &name.text)
                    }
                    _ => None,
                };
                complete &= !checker.incomplete_flow;
                callee.map_or_else(Vec::new, |callee| self.engine.call_parameter_types(&callee, *index))
            }
            EscapeTarget::Annotation { annotation, slot, value } => {
                let mut checker = native.checker_for_expression(*value, NativeCheckMode::TYPE_ONLY, &mut *self.engine);
                let pack = AnnotationPack::annotation(annotation).lower(&mut |ty| checker.annotation(ty));
                complete &= !checker.incomplete_flow;
                vec![checker.engine.pack_slot(&pack, *slot)]
            }
        }).collect();
        native.escape_views_active.borrow_mut().remove(&cache);
        let views = std::rc::Rc::new(views);
        if complete { native.escape_views.borrow_mut().insert(cache, std::rc::Rc::clone(&views)); }
        Some((views, complete))
    }

    /// `ty` without nil unless `literal` is nil: an optional field of a view does not make the
    /// table's present field optional, as an optional parameter does not make a local optional.
    fn present_field(&mut self, literal: &Type, ty: Type) -> Type {
        if matches!(literal, Type::Nil) || !self.engine.contains_nil(&ty) { return ty; }
        let mut parts = vec![self.engine.lua_truthiness_type(&ty, true)];
        if self.engine.is_assignable(&Type::BooleanLiteral(false), &ty) { parts.push(Type::BooleanLiteral(false)); }
        self.engine.union_reduced(&parts)
    }

    /// What `view`, at `path` below it, lets code write into the field `key`.
    fn escaped_field_view(&mut self, mut view: Type, path: &[LuaKey], key: &TableKey) -> Option<FieldView> {
        for step in path {
            if matches!(view, Type::Any | Type::Unknown) { return Some(FieldView::Unknown); }
            let table = self.engine.lua_truthiness_type(&view, true);
            view = self.engine.table_value_context(&table, &TableKey::Literal(step.clone()))?;
        }
        if matches!(view, Type::Any | Type::Unknown) { return Some(FieldView::Unknown); }
        let table = self.engine.lua_truthiness_type(&view, true);
        let field = self.engine.table_value_context(&table, key)?;
        if matches!(field, Type::Any | Type::Unknown) { return Some(FieldView::Unknown); }
        self.engine.table_write(&table, key, &field).is_ok().then_some(FieldView::Field(field))
    }
}
