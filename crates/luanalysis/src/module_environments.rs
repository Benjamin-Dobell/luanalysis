//! Lua 5.1's `module(name, ...)`, and Luanalysis v1's `---@module Name`, which names the type of
//! the table it makes.
//!
//! A top-level `module("X")` statement makes the global table `X` the rest of its chunk's global
//! environment. A free name the chunk assigns after it, in its own statements or in the functions
//! it defines there (`name = v`, `function name() end`), is then a field of `X`, not a global, and
//! a free name that is one of those fields, or a `---@field` of the table's documented type,
//! reads that field. Other free names read the globals, as `module("X", package.seeall)` lets
//! them and as v1 read them either way. `_M`, `_NAME` and `_PACKAGE` are the table, its name and
//! its package prefix (`"a."` of `"a.b"`), as `module` sets them.
//!
//! The checker reads such a chunk as the Lua that says so: `X = {}` before the call, and `X.name`
//! for each of those names. The table's type is the class `X`, as v1 read it, unless the call's
//! record (or a `---@module X` in the chunk) documents it. The fields are then the class's
//! members, which other files see through the global `X` and its type, as they see those of a
//! class table they assign field by field. The tree keeps every original span; a name it
//! supplies, which no source text spells there, has the span of the text it stands for or an
//! empty one, so that renaming leaves it alone.
//!
//! A dotted name, `module("a.b")`, stores the table in the field `b` of the global table `a`,
//! which `module` makes when there is none. As TypeScript declares the namespaces of
//! `namespace a.b {}`, the checker declares the class `a` with the field `b` of the class `a.b`,
//! and the global `a` of the class `a`. Classes of one name merge across the project, so `a` has
//! the fields of every module under it, and the members of `module("a")`'s own chunk. The table
//! itself is the global `a.b`, a name no source spells, as `package.loaded["a.b"]` holds it.
//!
//! `module(...)` is named by the `require` that loads its chunk: it is read as `module("name")`
//! with the first of the module names the project requires the file by, in `canonical` order.
//! When there is none, the table is anonymous: its global is a name no source spells, and its
//! type has no name.
use crate::bind::BoundFile;
use crate::collections::HashSet;
use luanalysis_syntax::{
    Expr, ExprKind, FieldDeclaration, File, ModuleEnvironment, Name, NamedRecordKind, NamedTypeDeclaration,
    NamedTypeDeclarationId, NamedTypeDeclarationKind, Span, Stmt, StmtKind, TableFieldKind, TypeAnnotation, TypeExpr,
    TypeExprKind, TypeFieldKey,
};

/// Reads `file`'s top-level `module("name")` and `module(...)` statements as the module docs
/// describe, and lists them in `File::module_environments`. `source` is the file's text, and
/// `loader_names` the names `require` loads it by, in `canonical` order.
pub(crate) fn read(file: &mut File, source: &str, loader_names: &[String]) {
    if !file.statements.iter().any(|statement| module_call(statement).is_some()) {
        return;
    }
    let bound = crate::bind::bind(file);
    let loaded = loader_names.iter().find(|name| module_name(name));
    let calls: Vec<Call> = file.statements.iter().enumerate()
        .filter_map(|(index, statement)| {
            let (callee, named) = module_call(statement)?;
            if !free(&bound, callee) { return None }
            let (module, written) = match named {
                Named::Written { name, string } => (Some(name), Some(string)),
                Named::Loader => (loaded.cloned(), None),
            };
            // A local named as the table's root would take the names meant for the table.
            if let Some(module) = &module
                && let root = module.split('.').next().unwrap_or_default()
                && bound.declarations.iter().any(|declaration| declaration.name == root)
            {
                return None;
            }
            let table = match &module {
                Some(module) => module.clone(),
                None => anonymous(source),
            };
            Some(Call { index, module, table, written })
        })
        .collect();
    let mut environments = Vec::with_capacity(calls.len());
    for (position, call) in calls.iter().enumerate() {
        let span = file.statements[call.index].span;
        let end = calls.get(position + 1).map_or(file.statements.len(), |next| next.index);
        let mut members = HashSet::default();
        for statement in &file.statements[call.index + 1..end] {
            stored_names(statement, &bound, &mut members);
        }
        for declaration in &file.type_declarations {
            if call.documented_by(declaration, span)
                && let NamedTypeDeclarationKind::Record { fields, .. } = &declaration.kind
            {
                members.extend(fields.iter().filter_map(|field| match &field.key {
                    TypeFieldKey::Named(field) => Some(field.text.clone()),
                    _ => None,
                }));
            }
        }
        let reading = Reading { bound: &bound, table: &call.table, module: call.module.as_deref(), members: &members };
        for statement in &mut file.statements[call.index + 1..end] {
            reading.statement(statement);
        }
        let range = Span::new(span.end, calls.get(position + 1).map_or(file.span.end, |next| file.statements[next.index].span.start));
        let mut members: Vec<String> = members.into_iter().collect();
        members.sort_unstable();
        environments.push(ModuleEnvironment { name: call.table.clone(), loader: call.written.is_none(), span: range, members });
    }
    if calls.iter().any(|call| call.written.is_none()) {
        file.loader_names = loader_names.to_vec();
    }
    // The table's declarations, from the last so that the earlier statements keep their places.
    let written = file.type_declarations.len();
    for call in calls.into_iter().rev() {
        let statements = call.declare(file, source, written);
        file.statements.splice(call.index..call.index, statements);
    }
    file.module_environments = environments;
}

/// Sorts the module names `require` loads a file by, the one that names its `module(...)` table
/// first: the fewest parts (`pkg` before `pkg.init`), then the shortest, then by text.
pub(crate) fn canonical(names: &mut Vec<String>) {
    names.sort_unstable_by(|left, right| {
        (left.split('.').count(), left.len(), left).cmp(&(right.split('.').count(), right.len(), right))
    });
    names.dedup();
}

/// Whether `receiver` is the table of a module's field the checker reads a free name as: the
/// `X`, with an empty span, of `X.name`. Its stores are the chunk's variables.
pub(crate) fn supplied_table(receiver: &Expr) -> bool {
    matches!(receiver.kind, ExprKind::Name(_)) && receiver.span.start == receiver.span.end
}

/// How a `module` call names its table.
enum Named {
    /// `module("name")`, by the string at `string`.
    Written { name: String, string: Span },
    /// `module(...)`, by the loader.
    Loader,
}

/// A top-level `module` call statement the checker reads.
struct Call {
    /// Its statement's index.
    index: usize,
    /// The module's name, `a.b`; none for an anonymous `module(...)`.
    module: Option<String>,
    /// The global holding the table.
    table: String,
    /// The span of the string naming it, for `module("name")`.
    written: Option<Span>,
}

impl Call {
    /// Whether a record declaration documents the table: one attached to the call, or a
    /// `---@module` of its name anywhere in the chunk (of any name, for `module(...)`), as v1 read
    /// one.
    fn documented_by(&self, declaration: &NamedTypeDeclaration, call: Span) -> bool {
        match &declaration.kind {
            NamedTypeDeclarationKind::Record { flavor, .. } => declaration.attachment == Some(call)
                || *flavor == NamedRecordKind::Module
                    && (self.written.is_none() || self.module.as_deref() == Some(declaration.name.text.as_str())),
            NamedTypeDeclarationKind::Alias { .. } => false,
        }
    }

    /// The statements declaring the table and the tables it is in, which go before the call, after
    /// declaring their types in `file`, whose first `written` type declarations are its own.
    fn declare(&self, file: &mut File, source: &str, written: usize) -> Vec<Stmt> {
        let call = file.statements[self.index].span;
        // Empty spans inside the call's `module`, which no other statement or expression has.
        let at = |offset: usize| Span::new(call.start + offset, call.start + offset);
        let components: Vec<&str> = self.module.as_deref().map_or_else(Vec::new, |module| module.split('.').collect());
        // Where the string spells each component, `a` and `b` in `"a.b"`.
        let spelled = self.written.zip(self.module.as_deref()).and_then(|(string, module)| written_components(source, string, module));
        let spelling = |count: usize| spelled.as_ref().map(|spans: &Vec<Span>| Span::new(spans[0].start, spans[count - 1].end));
        let mut documented = None;
        for declaration in &mut file.type_declarations[..written] {
            if self.documented_by(declaration, call) {
                declaration.attachment = Some(at(0));
                documented.get_or_insert_with(|| declaration.name.text.clone());
            }
        }
        let mut statements = Vec::new();
        if let Some(module) = &self.module {
            // The table's class, as v1 named it, or one of the module's name for the documented
            // class of another name to be the type of the field that holds the table.
            let name = Name { text: module.clone(), span: spelling(components.len()).unwrap_or(at(4)) };
            match &documented {
                None => declare_record(file, name, Vec::new(), Vec::new(), Some(at(0))),
                Some(documented) if documented != module => {
                    let base = TypeExpr { kind: TypeExprKind::Named(documented.clone()), span: at(4) };
                    declare_record(file, name, vec![base], Vec::new(), None);
                }
                Some(_) => {}
            }
            // Each table it is in, with the field of the next.
            for count in 1..components.len() {
                let owner = Name { text: components[..count].join("."), span: spelling(count).unwrap_or(at(4)) };
                let field = Name { text: components[count].to_owned(), span: spelled.as_ref().map_or(at(5), |spans| spans[count]) };
                let ty = TypeExpr { kind: TypeExprKind::Named(components[..=count].join(".")), span: at(5) };
                let field = FieldDeclaration {
                    key: TypeFieldKey::Named(field.clone()), ty, visibility: None, readonly: None, optional: false,
                    explicit_owner: None, deprecated_owner_span: None, span: field.span,
                };
                declare_record(file, owner, Vec::new(), vec![field], None);
            }
            // `---@type a` and `a = a`: the global `a`, which every module under it declares so.
            if components.len() > 1 {
                let root = components[0].to_owned();
                let target = spelling(1).unwrap_or(at(2));
                let annotation = TypeAnnotation { types: vec![TypeExpr { kind: TypeExprKind::Named(root.clone()), span: at(2) }], packs: None, span: at(2) };
                statements.push(Stmt {
                    kind: StmtKind::Assign {
                        targets: vec![Expr { kind: ExprKind::Name(Name { text: root.clone(), span: target }), span: target }],
                        values: vec![Expr { kind: ExprKind::Name(Name { text: root, span: at(3) }), span: at(3) }],
                        annotation: Some(annotation),
                    },
                    span: at(2),
                });
            }
        }
        // `X = {}`: the target spans the name in `module("X")`, so renaming the global `X` renames
        // it there; the global of a dotted or anonymous table is spelled nowhere.
        let target = match (components.len(), self.written) {
            (1, Some(string)) => spelling(1).unwrap_or(Span::new(string.start, string.start)),
            _ => at(1),
        };
        statements.push(Stmt {
            kind: StmtKind::Assign {
                targets: vec![Expr { kind: ExprKind::Name(Name { text: self.table.clone(), span: target }), span: target }],
                values: vec![Expr { kind: ExprKind::Table(Vec::new()), span: at(0) }],
                annotation: None,
            },
            span: at(0),
        });
        statements
    }
}

/// Declares a class-like record `name` of a module in `file`, attached to the statement at
/// `attachment`.
fn declare_record(file: &mut File, name: Name, bases: Vec<TypeExpr>, fields: Vec<FieldDeclaration>, attachment: Option<Span>) {
    let id = NamedTypeDeclarationId(file.type_declarations.len().try_into().expect("named declaration index fits u32"));
    file.type_declarations.push(NamedTypeDeclaration {
        id,
        span: name.span,
        group_span: name.span,
        name,
        generics: Vec::new(),
        kind: NamedTypeDeclarationKind::Record { flavor: NamedRecordKind::Module, bases, fields },
        attachment,
    });
}

/// The global of an anonymous `module(...)` table: a name no source spells, of the chunk's own.
fn anonymous(source: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    source.hash(&mut hasher);
    format!("module(...) {:016x}", hasher.finish())
}

/// The callee and name of a call statement `module("name", ...)` whose name is a module name,
/// or `module(...)`.
fn module_call(statement: &Stmt) -> Option<(&Name, Named)> {
    let StmtKind::Call(Expr { kind: ExprKind::Call { callee, args }, .. }) = &statement.kind else { return None };
    let ExprKind::Name(callee) = &callee.kind else { return None };
    if callee.text != "module" { return None }
    match args.first()? {
        Expr { kind: ExprKind::String(name), span } => {
            let name = std::str::from_utf8(name).ok().filter(|name| module_name(name))?;
            Some((callee, Named::Written { name: name.to_owned(), string: *span }))
        }
        Expr { kind: ExprKind::Vararg, .. } => Some((callee, Named::Loader)),
        _ => None,
    }
}

/// Whether `name` is one `module` stores as a path of tables: Lua names joined by dots.
fn module_name(name: &str) -> bool {
    name.split('.').all(luanalysis_syntax::is_identifier)
}

/// The span of each of `name`'s components where the string at `string` spells it, `a` and `b`
/// in `"a.b"` or `[[a.b]]`; none when it spells it otherwise (with escapes).
fn written_components(source: &str, string: Span, name: &str) -> Option<Vec<Span>> {
    let written = source.get(string.start..string.end)?;
    let delimiter = match written.as_bytes().first() {
        Some(b'"' | b'\'') => 1,
        Some(b'[') => written.bytes().skip(1).take_while(|&byte| byte == b'=').count() + 2,
        _ => return None,
    };
    let start = string.start + delimiter;
    if string.end.checked_sub(delimiter) != Some(start + name.len()) || source.get(start..start + name.len()) != Some(name) {
        return None;
    }
    let mut offset = start;
    Some(name.split('.').map(|component| {
        let span = Span::new(offset, offset + component.len());
        offset = span.end + 1;
        span
    }).collect())
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
    /// The global holding the table.
    table: &'a str,
    /// The module's name; none for an anonymous `module(...)`.
    module: Option<&'a str>,
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
                    let module = self.module.unwrap_or_default();
                    match name.text.as_str() {
                        "_M" => name.text = self.table.to_owned(),
                        "_NAME" => expression.kind = ExprKind::String(module.as_bytes().to_vec()),
                        "_PACKAGE" => {
                            let package = module.rfind('.').map_or("", |dot| &module[..=dot]);
                            expression.kind = ExprKind::String(package.as_bytes().to_vec());
                        }
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
