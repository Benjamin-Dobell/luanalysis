//! Lua 5.1's `module(name, ...)`, and Luanalysis v1's `---@module Name`, which names the type of
//! the table it makes.
//!
//! A top-level `module("X")` statement makes the global table `X` the rest of its chunk's global
//! environment. A free name the chunk assigns after it, in its own statements or in the functions
//! it defines there (`name = v`, `function name() end`), is then a field of `X`, not a global, and
//! a free name that is one of those fields, or a `---@field` of the table's documented type,
//! reads that field. Other free names read the globals, as `module("X", package.seeall)` lets
//! them and as v1 read them either way. `_M`, `_NAME` and `_PACKAGE` are the table, its name and
//! its package prefix (`""`), as `module` sets them.
//!
//! The checker reads such a chunk as the Lua that says so: `X = {}` before the call, which the
//! call's `---@module X` (or `---@class X`) documents, and `X.name` for each of those names. The
//! fields are then the table's members, which other files see through the global `X` and its
//! type, as they see those of a class table they assign field by field. The tree keeps every
//! original span; a name it supplies, which no source text spells there, has the span of the
//! text it stands for or an empty one, so that renaming leaves it alone.
//!
//! Only a literal name that is a Lua name is read so. A dotted name, `module("a.b")`, stores the
//! table in a field of another, which that table's type would need to declare, and
//! `module(...)` is named by whichever `require` loads its chunk.
use crate::bind::BoundFile;
use crate::collections::HashSet;
use luanalysis_syntax::{
    Expr, ExprKind, File, ModuleEnvironment, Name, NamedRecordKind, NamedTypeDeclarationKind, Span, Stmt,
    StmtKind, TableFieldKind, TypeFieldKey,
};

/// Reads `file`'s top-level `module("name")` statements as the module docs describe, and lists
/// them in `File::module_environments`. `source` is the file's text.
pub(crate) fn read(file: &mut File, source: &str) {
    if !file.statements.iter().any(|statement| module_call(statement).is_some()) {
        return;
    }
    let bound = crate::bind::bind(file);
    // A local `module` is some other function, and a local named as the table would take the
    // names meant for the table.
    let calls: Vec<(usize, String, Span)> = file.statements.iter().enumerate()
        .filter_map(|(index, statement)| {
            let (callee, name, string) = module_call(statement)?;
            (free(&bound, callee) && !bound.declarations.iter().any(|declaration| declaration.name == name))
                .then_some((index, name, string))
        })
        .collect();
    let mut environments = Vec::with_capacity(calls.len());
    for (position, (index, name, _)) in calls.iter().enumerate() {
        let call = file.statements[*index].span;
        let end = calls.get(position + 1).map_or(file.statements.len(), |(next, ..)| *next);
        let mut members = HashSet::default();
        for statement in &file.statements[index + 1..end] {
            stored_names(statement, &bound, &mut members);
        }
        for declaration in &file.type_declarations {
            if documents(declaration, call, name)
                && let NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind
            {
                members.extend(fields.iter().filter_map(|field| match &field.key {
                    TypeFieldKey::Named(field) => Some(field.text.clone()),
                    _ => None,
                }));
            }
        }
        let reading = Reading { bound: &bound, table: name, members: &members };
        for statement in &mut file.statements[index + 1..end] {
            reading.statement(statement);
        }
        let span = Span::new(call.end, calls.get(position + 1).map_or(file.span.end, |(next, ..)| file.statements[*next].span.start));
        let mut members: Vec<String> = members.into_iter().collect();
        members.sort_unstable();
        environments.push(ModuleEnvironment { name: name.clone(), span, members });
    }
    // The table's declarations, from the last so that the earlier statements keep their places.
    for (index, name, string) in calls.into_iter().rev() {
        let call = file.statements[index].span;
        // Empty spans at the call's start, which no other statement or expression has.
        let at = Span::new(call.start, call.start);
        for declaration in &mut file.type_declarations {
            if documents(declaration, call, &name) {
                declaration.attachment = Some(at);
            }
        }
        let written = written_name(source, string);
        let target = Expr { kind: ExprKind::Name(Name { text: name, span: written }), span: written };
        let value = Expr { kind: ExprKind::Table(Vec::new()), span: at };
        file.statements.insert(index, Stmt { kind: StmtKind::Assign { targets: vec![target], values: vec![value], annotation: None }, span: at });
    }
    file.module_environments = environments;
}

/// Whether `receiver` is the table of a module's field the checker reads a free name as: the
/// `X`, with an empty span, of `X.name`. Its stores are the chunk's variables.
pub(crate) fn supplied_table(receiver: &Expr) -> bool {
    matches!(receiver.kind, ExprKind::Name(_)) && receiver.span.start == receiver.span.end
}

/// The callee, table name and name string of a call statement `module("name", ...)` whose name
/// is a Lua name.
fn module_call(statement: &Stmt) -> Option<(&Name, String, Span)> {
    let StmtKind::Call(Expr { kind: ExprKind::Call { callee, args }, .. }) = &statement.kind else { return None };
    let ExprKind::Name(callee) = &callee.kind else { return None };
    if callee.text != "module" { return None }
    let Some(Expr { kind: ExprKind::String(name), span }) = args.first() else { return None };
    let name = std::str::from_utf8(name).ok().filter(|name| luanalysis_syntax::is_identifier(name))?;
    Some((callee, name.to_owned(), *span))
}

/// Whether a record declaration documents the table of the `module("name")` call statement
/// `call`: one attached to it, or a `---@module name` anywhere in the chunk, as v1 read one.
fn documents(declaration: &luanalysis_syntax::NamedTypeDeclaration, call: Span, name: &str) -> bool {
    match &declaration.kind {
        NamedTypeDeclarationKind::Record { flavor, .. } => declaration.attachment == Some(call)
            || *flavor == NamedRecordKind::Module && declaration.name.text == name,
        NamedTypeDeclarationKind::Alias { .. } => false,
    }
}

/// The table name's text in its string, `X` in `"X"` or `[[X]]`; or, when the string spells it
/// otherwise (with escapes), an empty span at the string's start.
fn written_name(source: &str, string: Span) -> Span {
    let written = source.get(string.start..string.end).unwrap_or_default();
    let delimiter = match written.as_bytes().first() {
        Some(b'"' | b'\'') => 1,
        Some(b'[') => written.bytes().skip(1).take_while(|&byte| byte == b'=').count() + 2,
        _ => return Span::new(string.start, string.start),
    };
    let inner = Span::new(string.start + delimiter, string.end.saturating_sub(delimiter));
    match source.get(inner.start..inner.end) {
        Some(text) if luanalysis_syntax::is_identifier(text) => inner,
        _ => Span::new(string.start, string.start),
    }
}

/// Whether the name expression `name` reads or writes a global: its occurrence binds no local,
/// and no local `_ENV` redirects it.
fn free(bound: &BoundFile, name: &Name) -> bool {
    bound.occurrence_at(name.span.start).is_some_and(|occurrence| occurrence.span == name.span && occurrence.binding.is_none())
        && name.text != "_ENV"
        && bound.environment_binding_at(name.span.start).is_none()
}

/// The free names `statement` and the statements and functions in it assign.
fn stored_names(statement: &Stmt, bound: &BoundFile, names: &mut HashSet<String>) {
    let mut store = |target: &Expr| if let ExprKind::Name(name) = &target.kind && free(bound, name) {
        names.insert(name.text.clone());
    };
    match &statement.kind {
        StmtKind::Assign { targets, .. } => targets.iter().for_each(&mut store),
        StmtKind::FunctionDeclaration { target, .. } => store(target),
        _ => {}
    }
    let mut nested = Vec::new();
    nested_bodies(statement, &mut nested);
    for body in nested {
        for statement in body {
            stored_names(statement, bound, names);
        }
    }
}

/// The statement lists directly in `statement`: its blocks', and the bodies of the functions its
/// expressions define.
fn nested_bodies<'a>(statement: &'a Stmt, bodies: &mut Vec<&'a [Stmt]>) {
    match &statement.kind {
        StmtKind::Local { values, .. } | StmtKind::Return(values) => values.iter().for_each(|value| expression_bodies(value, bodies)),
        StmtKind::Assign { targets, values, .. } => targets.iter().chain(values).for_each(|value| expression_bodies(value, bodies)),
        StmtKind::Call(value) => expression_bodies(value, bodies),
        StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => bodies.push(&function.body),
        StmtKind::Block { body, .. } => bodies.push(body),
        StmtKind::While { condition, body, .. } | StmtKind::Repeat { condition, body, .. } => {
            expression_bodies(condition, bodies);
            bodies.push(body);
        }
        StmtKind::If { branches, else_branch } => {
            for branch in branches {
                expression_bodies(&branch.condition, bodies);
                bodies.push(&branch.body);
            }
            if let Some(branch) = else_branch { bodies.push(&branch.body); }
        }
        StmtKind::NumericFor { start, limit, step, body, .. } => {
            expression_bodies(start, bodies);
            expression_bodies(limit, bodies);
            if let Some(step) = step { expression_bodies(step, bodies); }
            bodies.push(body);
        }
        StmtKind::GenericFor { values, body, .. } => {
            values.iter().for_each(|value| expression_bodies(value, bodies));
            bodies.push(body);
        }
        StmtKind::Goto(_) | StmtKind::Label(_) | StmtKind::Break => {}
    }
}

/// The bodies of the functions `expression` defines, outside the ones nested in those.
fn expression_bodies<'a>(expression: &'a Expr, bodies: &mut Vec<&'a [Stmt]>) {
    match &expression.kind {
        ExprKind::Function(function) => bodies.push(&function.body),
        ExprKind::Parenthesized(value) | ExprKind::Field { base: value, .. } | ExprKind::Unary { value, .. } => expression_bodies(value, bodies),
        ExprKind::Index { base, index } => {
            expression_bodies(base, bodies);
            expression_bodies(index, bodies);
        }
        ExprKind::Binary { left, right, .. } => {
            expression_bodies(left, bodies);
            expression_bodies(right, bodies);
        }
        ExprKind::Table(fields) => for field in fields {
            if let TableFieldKind::Keyed(key) = &field.kind { expression_bodies(key, bodies); }
            expression_bodies(&field.value, bodies);
        },
        ExprKind::Call { callee: receiver, args } | ExprKind::MethodCall { receiver, args, .. } => {
            expression_bodies(receiver, bodies);
            args.iter().for_each(|argument| expression_bodies(argument, bodies));
        }
        ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_) | ExprKind::Name(_)
        | ExprKind::Vararg | ExprKind::Error => {}
    }
}

/// Reads the free names of a module chunk's statements as the table's fields.
struct Reading<'a> {
    bound: &'a BoundFile,
    /// The table's global name.
    table: &'a str,
    /// The table's fields that free names read and write.
    members: &'a HashSet<String>,
}

impl Reading<'_> {
    fn statements(&self, statements: &mut [Stmt]) {
        for statement in statements {
            self.statement(statement);
        }
    }

    fn statement(&self, statement: &mut Stmt) {
        match &mut statement.kind {
            StmtKind::Local { values, .. } | StmtKind::Return(values) => values.iter_mut().for_each(|value| self.expression(value)),
            StmtKind::Assign { targets, values, .. } => targets.iter_mut().chain(values).for_each(|value| self.expression(value)),
            StmtKind::Call(value) => self.expression(value),
            StmtKind::LocalFunction { function, .. } => self.statements(&mut function.body),
            StmtKind::FunctionDeclaration { target, function, .. } => {
                self.expression(target);
                self.statements(&mut function.body);
            }
            StmtKind::Block { body, .. } => self.statements(body),
            StmtKind::While { condition, body, .. } | StmtKind::Repeat { condition, body, .. } => {
                self.expression(condition);
                self.statements(body);
            }
            StmtKind::If { branches, else_branch } => {
                for branch in branches {
                    self.expression(&mut branch.condition);
                    self.statements(&mut branch.body);
                }
                if let Some(branch) = else_branch { self.statements(&mut branch.body); }
            }
            StmtKind::NumericFor { start, limit, step, body, .. } => {
                self.expression(start);
                self.expression(limit);
                if let Some(step) = step { self.expression(step); }
                self.statements(body);
            }
            StmtKind::GenericFor { values, body, .. } => {
                values.iter_mut().for_each(|value| self.expression(value));
                self.statements(body);
            }
            StmtKind::Goto(_) | StmtKind::Label(_) | StmtKind::Break => {}
        }
    }

    fn expression(&self, expression: &mut Expr) {
        match &mut expression.kind {
            ExprKind::Name(name) if free(self.bound, name) => {
                if self.members.contains(&name.text) {
                    // `X.name`: the name keeps its span, and `X` has an empty one before it.
                    let name = name.clone();
                    let at = Span::new(name.span.start, name.span.start);
                    let base = Expr { kind: ExprKind::Name(Name { text: self.table.to_owned(), span: at }), span: at };
                    expression.kind = ExprKind::Field { base: Box::new(base), name };
                } else {
                    match name.text.as_str() {
                        "_M" => name.text = self.table.to_owned(),
                        "_NAME" => expression.kind = ExprKind::String(self.table.as_bytes().to_vec()),
                        "_PACKAGE" => expression.kind = ExprKind::String(Vec::new()),
                        _ => {}
                    }
                }
            }
            ExprKind::Function(function) => self.statements(&mut function.body),
            ExprKind::Parenthesized(value) | ExprKind::Field { base: value, .. } | ExprKind::Unary { value, .. } => self.expression(value),
            ExprKind::Index { base, index } => {
                self.expression(base);
                self.expression(index);
            }
            ExprKind::Binary { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
            ExprKind::Table(fields) => for field in fields {
                if let TableFieldKind::Keyed(key) = &mut field.kind { self.expression(key); }
                self.expression(&mut field.value);
            },
            ExprKind::Call { callee: receiver, args } | ExprKind::MethodCall { receiver, args, .. } => {
                self.expression(receiver);
                args.iter_mut().for_each(|argument| self.expression(argument));
            }
            ExprKind::Nil | ExprKind::Boolean(_) | ExprKind::Number(_) | ExprKind::String(_) | ExprKind::Name(_)
            | ExprKind::Vararg | ExprKind::Error => {}
        }
    }
}
