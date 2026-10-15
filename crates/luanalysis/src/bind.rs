//! Lua lexical bindings and source occurrences, independent of type checking.
//!
//! Declarations become active at Lua's evaluation boundary. Names in ordinary
//! local initializers therefore resolve before the new declarations exist,
//! while a local function's name is available to its recursive body.

use luanalysis_syntax::{
    Expr, ExprKind, File, Function, Name, Span, Stmt, StmtKind, TableFieldKind,
};
use std::collections::HashMap;

/// A dense scope identity belonging to one BoundFile source snapshot.
/// IDs from another file or a later parse must not be used with this graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScopeId(pub usize);

/// A dense declaration identity belonging to one BoundFile source snapshot.
/// Repeated declarations of the same name always have distinct identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BindingId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    File,
    Block,
    Loop,
    Function,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    pub span: Span,
    pub bindings: Vec<BindingId>,
    pub children: Vec<ScopeId>,
    /// Lexical bindings needed as upvalues, including forwarding to inner closures.
    pub captures: Vec<BindingId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarationKind {
    Local,
    LocalFunction,
    Parameter,
    /// Lua's real first parameter in `function receiver:method(...)`.
    /// Its source declaration is the original colon, not an inserted AST name.
    ImplicitReceiver,
    LoopVariable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub id: BindingId,
    pub name: String,
    pub span: Span,
    pub scope: ScopeId,
    pub kind: DeclarationKind,
    /// Original byte offset after which this declaration is active.
    /// For parameters this is the start of the function body.
    pub activation: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccurrenceKind {
    Declaration,
    Read,
    Write,
    /// A named @param tag referring to a function parameter.
    Annotation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameOccurrence {
    pub name: String,
    pub span: Span,
    pub scope: ScopeId,
    /// None preserves an unresolved global or an unmatched @param name.
    pub binding: Option<BindingId>,
    pub kind: OccurrenceKind,
}

/// A standard Lua operation identified by lexical binding facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinIntrinsic {
    LuaRequire,
    LuaType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IntrinsicOccurrence {
    span: Span,
    intrinsic: BuiltinIntrinsic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundFile {
    pub scopes: Vec<Scope>,
    pub declarations: Vec<Declaration>,
    /// Original-source occurrences sorted by byte offset.
    pub occurrences: Vec<NameOccurrence>,
    intrinsic_occurrences: Vec<IntrinsicOccurrence>,
    modified_module_environment: bool,
    module_environment_mutations: Vec<Span>,
    modified_type_intrinsic: bool,
    modified_assert_intrinsic: bool,
    assert_intrinsic_mutations: Vec<Span>,
}

impl BoundFile {
    pub fn implicit_receiver(&self, function_span: Span) -> Option<BindingId> {
        let scope = self.scopes.iter().find(|scope| {
            scope.kind == ScopeKind::Function && scope.span == function_span
        })?;
        scope.bindings.iter().copied().find(|binding| {
            self.declaration(*binding).kind == DeclarationKind::ImplicitReceiver
        })
    }
    pub fn declaration(&self, binding: BindingId) -> &Declaration {
        &self.declarations[binding.0]
    }

    pub fn scope(&self, scope: ScopeId) -> &Scope {
        &self.scopes[scope.0]
    }

    /// Finds a declaration, read, write or annotation under an original byte offset.
    pub fn occurrence_at(&self, byte_offset: usize) -> Option<&NameOccurrence> {
        let after = self
            .occurrences
            .partition_point(|occurrence| occurrence.span.start <= byte_offset);
        after.checked_sub(1).and_then(|index| {
            let occurrence = &self.occurrences[index];
            (byte_offset < occurrence.span.end).then_some(occurrence)
        })
    }

    pub fn binding_at(&self, byte_offset: usize) -> Option<BindingId> {
        self.occurrence_at(byte_offset)
            .and_then(|occurrence| occurrence.binding)
    }

    /// Identifies a standard operation under an original name byte offset.
    ///
    /// Direct replacement of an initially standard value invalidates its
    /// candidate, including at earlier occurrences. Loader and type-operation
    /// facts are independent; the checker still selects actual value identity.
    pub fn intrinsic_at(&self, byte_offset: usize) -> Option<BuiltinIntrinsic> {
        let intrinsic = self.standard_candidate_at(byte_offset)?;
        match intrinsic {
            BuiltinIntrinsic::LuaRequire if self.modified_module_environment => None,
            BuiltinIntrinsic::LuaType if self.modified_type_intrinsic => None,
            intrinsic => Some(intrinsic),
        }
    }

    /// Lexical candidate only. Runtime value identity is selected at the
    /// original evaluation's native flow input, never from a whole-file flag.
    pub fn standard_candidate_at(&self, byte_offset: usize) -> Option<BuiltinIntrinsic> {
        let after = self
            .intrinsic_occurrences
            .partition_point(|occurrence| occurrence.span.start <= byte_offset);
        after.checked_sub(1).and_then(|index| {
            let occurrence = &self.intrinsic_occurrences[index];
            if byte_offset >= occurrence.span.end {
                return None;
            }
            Some(occurrence.intrinsic)
        })
    }

    /// The real lexical `_ENV` upvalue for an unresolved source name.
    /// None denotes the chunk's implicit environment upvalue.
    pub fn environment_binding_at(&self, byte_offset: usize) -> Option<BindingId> {
        let mut scope = self.scope_at(byte_offset);
        while let Some(current) = scope {
            let node = self.scope(current);
            if let Some(binding) = node.bindings.iter().rev().copied().find(|binding| {
                let declaration = self.declaration(*binding);
                declaration.name == "_ENV" && declaration.activation <= byte_offset
            }) {
                return Some(binding);
            }
            scope = node.parent;
        }
        None
    }

    /// Direct syntactic writes to the standard global module environment.
    /// Project hosts can combine this fact across files before resolving imports.
    pub fn has_modified_module_environment(&self) -> bool {
        self.modified_module_environment
    }
    pub fn module_environment_mutations(&self) -> &[Span] { &self.module_environment_mutations }

    /// Direct replacement of the initially standard `type` value. This is a
    /// lexical fact; the project still selects the actual environment identity.
    pub fn has_modified_type_intrinsic(&self) -> bool {
        self.modified_type_intrinsic
    }

    pub fn has_modified_assert_intrinsic(&self) -> bool {
        self.modified_assert_intrinsic
    }

    pub fn assert_intrinsic_mutations(&self) -> &[Span] {
        &self.assert_intrinsic_mutations
    }

    pub fn scope_at(&self, byte_offset: usize) -> Option<ScopeId> {
        if let Some(occurrence) = self.occurrence_at(byte_offset) {
            return Some(occurrence.scope);
        }
        self.scopes
            .iter()
            .filter(|scope| scope.span.start <= byte_offset && byte_offset < scope.span.end)
            .min_by_key(|scope| scope.span.end - scope.span.start)
            .map(|scope| scope.id)
    }

    pub fn references(&self, binding: BindingId) -> impl Iterator<Item = &NameOccurrence> {
        self.occurrences.iter().filter(move |occurrence| {
            occurrence.binding == Some(binding) && occurrence.kind != OccurrenceKind::Declaration
        })
    }
}

/// Resolves Lua names without constructing a TypeScript AST or invoking a checker.
pub fn bind(file: &File) -> BoundFile {
    let mut binder = Binder {
        file: BoundFile {
            scopes: Vec::new(),
            declarations: Vec::new(),
            occurrences: Vec::new(),
            intrinsic_occurrences: Vec::new(),
            modified_module_environment: false,
            module_environment_mutations: Vec::new(),
            modified_type_intrinsic: false,
            modified_assert_intrinsic: false,
            assert_intrinsic_mutations: Vec::new(),
        },
        active: Vec::new(),
    };
    binder.enter_scope(ScopeKind::File, file.span);
    binder.statements(&file.statements);
    binder.leave_scope();
    binder
        .file
        .occurrences
        .sort_by_key(|occurrence| (occurrence.span.start, occurrence.span.end));
    binder
        .file
        .intrinsic_occurrences
        .sort_by_key(|occurrence| (occurrence.span.start, occurrence.span.end));
    binder.file
}

struct ActiveScope {
    id: ScopeId,
    names: HashMap<String, BindingId>,
}

struct Binder {
    file: BoundFile,
    active: Vec<ActiveScope>,
}

impl Binder {
    fn current_scope(&self) -> ScopeId {
        self.active.last().expect("an active Lua scope").id
    }

    fn enter_scope(&mut self, kind: ScopeKind, span: Span) {
        let parent = self.active.last().map(|scope| scope.id);
        let id = ScopeId(self.file.scopes.len());
        self.file.scopes.push(Scope {
            id,
            parent,
            kind,
            span,
            bindings: Vec::new(),
            children: Vec::new(),
            captures: Vec::new(),
        });
        if let Some(parent) = parent {
            self.file.scopes[parent.0].children.push(id);
        }
        self.active.push(ActiveScope {
            id,
            names: HashMap::new(),
        });
    }

    fn leave_scope(&mut self) {
        self.active.pop().expect("an active Lua scope");
    }

    fn declare(&mut self, name: &Name, kind: DeclarationKind, activation: usize) {
        self.declare_at(&name.text, name.span, kind, activation);
    }

    fn declare_at(&mut self, name: &str, span: Span, kind: DeclarationKind, activation: usize) {
        let id = BindingId(self.file.declarations.len());
        let scope = self.current_scope();
        self.file.declarations.push(Declaration {
            id,
            name: name.to_owned(),
            span,
            scope,
            kind,
            activation,
        });
        self.file.scopes[scope.0].bindings.push(id);
        self.active
            .last_mut()
            .unwrap()
            .names
            .insert(name.to_owned(), id);
        self.occurrence_at(name, span, OccurrenceKind::Declaration, Some(id));
    }

    fn resolve(&self, name: &str) -> Option<BindingId> {
        self.active
            .iter()
            .rev()
            .find_map(|scope| scope.names.get(name).copied())
    }

    fn occurrence(&mut self, name: &Name, kind: OccurrenceKind, binding: Option<BindingId>) {
        self.occurrence_at(&name.text, name.span, kind, binding);
    }

    fn occurrence_at(&mut self, name: &str, span: Span, kind: OccurrenceKind, binding: Option<BindingId>) {
        let scope = self.current_scope();
        self.file.occurrences.push(NameOccurrence {
            name: name.to_owned(),
            span,
            scope,
            binding,
            kind,
        });
        if matches!(kind, OccurrenceKind::Read | OccurrenceKind::Write) {
            if let Some(binding) = binding.or_else(|| self.resolve("_ENV")) {
                let declaring_scope = self.file.declarations[binding.0].scope;
                let mut capture_scope = scope;
                while capture_scope != declaring_scope {
                    let node = &mut self.file.scopes[capture_scope.0];
                    if node.kind == ScopeKind::Function && !node.captures.contains(&binding) {
                        node.captures.push(binding);
                    }
                    capture_scope = node
                        .parent
                        .expect("resolved bindings are in enclosing scopes");
                }
            }
        }
    }

    fn name(&mut self, name: &Name, kind: OccurrenceKind) {
        let binding = self.resolve(&name.text);
        if kind == OccurrenceKind::Read
            && binding.is_none()
            && self.resolve("_ENV").is_none()
        {
            let intrinsic = match name.text.as_str() {
                "require" => Some(BuiltinIntrinsic::LuaRequire),
                "type" => Some(BuiltinIntrinsic::LuaType),
                _ => None,
            };
            if let Some(intrinsic) = intrinsic {
                self.file.intrinsic_occurrences.push(IntrinsicOccurrence {
                    span: name.span,
                    intrinsic,
                });
            }
        }
        self.occurrence(name, kind, binding);
    }

    fn statements(&mut self, statements: &[Stmt]) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local { names, values, .. } => {
                    for value in values {
                        self.expression(value);
                    }
                    for name in names {
                        self.declare(name, DeclarationKind::Local, statement.span.end);
                    }
                }
                StmtKind::Assign {
                    targets, values, ..
                } => {
                    for target in targets {
                        self.target(target);
                    }
                    for value in values {
                        self.expression(value);
                    }
                }
                StmtKind::Block { body, body_span } => {
                    self.enter_scope(ScopeKind::Block, *body_span);
                    self.statements(body);
                    self.leave_scope();
                }
                StmtKind::LocalFunction { name, function } => {
                    self.declare(name, DeclarationKind::LocalFunction, name.span.end);
                    self.function(function, None);
                }
                StmtKind::FunctionDeclaration {
                    target, function, method_span
                } => {
                    self.target(target);
                    self.function(function, *method_span);
                }
                StmtKind::Return(values) => {
                    for value in values {
                        self.expression(value);
                    }
                }
                StmtKind::Call(expression) => self.expression(expression),
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.expression(&branch.condition);
                        self.enter_scope(ScopeKind::Block, branch.body_span);
                        self.statements(&branch.body);
                        self.leave_scope();
                    }
                    if let Some(branch) = else_branch {
                        self.enter_scope(ScopeKind::Block, branch.body_span);
                        self.statements(&branch.body);
                        self.leave_scope();
                    }
                }
                StmtKind::While {
                    condition,
                    body,
                    body_span,
                } => {
                    self.expression(condition);
                    self.enter_scope(ScopeKind::Loop, *body_span);
                    self.statements(body);
                    self.leave_scope();
                }
                StmtKind::Repeat {
                    body,
                    condition,
                    body_span,
                } => {
                    self.enter_scope(ScopeKind::Loop, body_span.join(condition.span));
                    self.statements(body);
                    self.expression(condition);
                    self.leave_scope();
                }
                StmtKind::NumericFor {
                    name,
                    start,
                    limit,
                    step,
                    body,
                    body_span,
                } => {
                    self.expression(start);
                    self.expression(limit);
                    if let Some(step) = step {
                        self.expression(step);
                    }
                    self.enter_scope(ScopeKind::Loop, *body_span);
                    self.declare(name, DeclarationKind::LoopVariable, body_span.start);
                    self.statements(body);
                    self.leave_scope();
                }
                StmtKind::GenericFor {
                    names,
                    values,
                    body,
                    body_span,
                } => {
                    for value in values {
                        self.expression(value);
                    }
                    self.enter_scope(ScopeKind::Loop, *body_span);
                    for name in names {
                        self.declare(name, DeclarationKind::LoopVariable, body_span.start);
                    }
                    self.statements(body);
                    self.leave_scope();
                }
                StmtKind::Break => {}
            }
        }
    }

    fn target(&mut self, expression: &Expr) {
        if self.modifies_module_environment(expression) {
            self.file.modified_module_environment = true;
            self.file.module_environment_mutations.push(expression.span);
        }
        if self.modifies_standard_intrinsic(expression, "type") {
            self.file.modified_type_intrinsic = true;
        }
        if self.modifies_standard_intrinsic(expression, "assert") {
            self.file.modified_assert_intrinsic = true;
            self.file.assert_intrinsic_mutations.push(expression.span);
        }
        match &expression.kind {
            ExprKind::Name(name) => self.name(name, OccurrenceKind::Write),
            ExprKind::Field { base, .. } => self.expression(base),
            ExprKind::Index { base, index } => {
                self.expression(base);
                self.expression(index);
            }
            ExprKind::Parenthesized(inner) => self.target(inner),
            _ => self.expression(expression),
        }
    }

    fn modifies_module_environment(&self, expression: &Expr) -> bool {
        let mut keys = Vec::new();
        let Some(root) = access_path(expression, &mut keys) else {
            return false;
        };
        self.global_path_modifies_module_environment(root, &keys)
    }

    fn modifies_standard_intrinsic(&self, expression: &Expr, intrinsic: &str) -> bool {
        let mut keys = Vec::new();
        let Some(root) = access_path(expression, &mut keys) else { return false; };
        self.global_path_modifies_standard_intrinsic(root, &keys, intrinsic)
    }

    fn global_path_modifies_standard_intrinsic(&self, root: &Name, keys: &[Option<&[u8]>], intrinsic: &str) -> bool {
        if self.resolve("_ENV").is_some() || self.resolve(&root.text).is_some() {
            return false;
        }
        if root.text == intrinsic { return keys.is_empty(); }
        match root.text.as_str() {
            "_ENV" | "_G" if keys.is_empty() => true,
            "_G" | "_ENV" => matches!(keys, [None]) || matches!(keys, [Some(key)] if *key == intrinsic.as_bytes()),
            _ => false,
        }
    }

    fn call_modifies_standard_intrinsic(&self, callee: &Expr, args: &[Expr], intrinsic: &str) -> bool {
        let mut keys = Vec::new();
        let Some(root) = access_path(callee, &mut keys) else { return false; };
        if self.resolve("_ENV").is_some() || self.resolve(&root.text).is_some() {
            return false;
        }
        let rawset = matches!((root.text.as_str(), keys.as_slice()),
            ("rawset", []) | ("_G" | "_ENV", [Some(b"rawset")]));
        if !rawset || args.len() < 2 { return false; }
        let mut target_keys = Vec::new();
        let Some(target) = access_path(&args[0], &mut target_keys) else { return false; };
        target_keys.push(string_key(&args[1]));
        self.global_path_modifies_standard_intrinsic(target, &target_keys, intrinsic)
    }

    fn global_path_modifies_module_environment(&self, root: &Name, keys: &[Option<&[u8]>]) -> bool {
        // An explicit local _ENV redirects unresolved names as well as writes.
        if self.resolve("_ENV").is_some() || self.resolve(&root.text).is_some() {
            return false;
        }
        match root.text.as_str() {
            "require" => keys.is_empty(),
            "package" => modifies_package(keys),
            "_ENV" if keys.is_empty() => true,
            "_G" | "_ENV" => match keys.first() {
                Some(None) => true,
                Some(Some(b"require")) => keys.len() == 1,
                Some(Some(b"package")) => modifies_package(&keys[1..]),
                _ => false,
            },
            _ => false,
        }
    }

    fn call_modifies_module_environment(&self, callee: &Expr, args: &[Expr]) -> bool {
        let mut callee_keys = Vec::new();
        let Some(callee_root) = access_path(callee, &mut callee_keys) else {
            return false;
        };
        if self.resolve("_ENV").is_some() || self.resolve(&callee_root.text).is_some() {
            return false;
        }
        let (global, members) = match (callee_root.text.as_str(), callee_keys.as_slice()) {
            ("_G" | "_ENV", [Some(global), members @ ..]) => (*global, members),
            (global, members) => (global.as_bytes(), members),
        };
        match (global, members) {
            (b"rawset", []) if args.len() >= 2 => {
                let mut target_keys = Vec::new();
                let Some(target_root) = access_path(&args[0], &mut target_keys) else {
                    return false;
                };
                target_keys.push(string_key(&args[1]));
                self.global_path_modifies_module_environment(target_root, &target_keys)
            }
            (b"table", [Some(b"insert" | b"remove")]) if !args.is_empty() => {
                self.modifies_module_environment(&args[0])
            }
            (b"table", [Some(b"move")]) if args.len() >= 4 => {
                self.modifies_module_environment(args.get(4).unwrap_or(&args[0]))
            }
            _ => false,
        }
    }

    fn expression(&mut self, expression: &Expr) {
        match &expression.kind {
            ExprKind::Name(name) => self.name(name, OccurrenceKind::Read),
            ExprKind::Parenthesized(inner) => self.expression(inner),
            ExprKind::Field { base, .. } => self.expression(base),
            ExprKind::Index { base, index } => {
                self.expression(base);
                self.expression(index);
            }
            ExprKind::Unary { value, .. } => self.expression(value),
            ExprKind::Binary { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
            ExprKind::Table(fields) => {
                for field in fields {
                    if let TableFieldKind::Keyed(key) = &field.kind {
                        self.expression(key);
                    }
                    self.expression(&field.value);
                }
            }
            ExprKind::Function(function) => self.function(function, None),
            ExprKind::Call { callee, args } => {
                if self.call_modifies_module_environment(callee, args) {
                    self.file.modified_module_environment = true;
                    self.file.module_environment_mutations.push(expression.span);
                }
                if self.call_modifies_standard_intrinsic(callee, args, "type") {
                    self.file.modified_type_intrinsic = true;
                }
                if self.call_modifies_standard_intrinsic(callee, args, "assert") {
                    self.file.modified_assert_intrinsic = true;
                    self.file.assert_intrinsic_mutations.push(expression.span);
                }
                self.expression(callee);
                for argument in args {
                    self.expression(argument);
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expression(receiver);
                for argument in args {
                    self.expression(argument);
                }
            }
            ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Vararg
            | ExprKind::Error => {}
        }
    }

    fn function(&mut self, function: &Function, receiver: Option<Span>) {
        self.enter_scope(ScopeKind::Function, function.span);
        if let Some(span) = receiver {
            self.declare_at("self", span, DeclarationKind::ImplicitReceiver, function.body_span.start);
        }
        for parameter in &function.params {
            self.declare(
                parameter,
                DeclarationKind::Parameter,
                function.body_span.start,
            );
        }
        for annotation in &function.annotations.params {
            let binding = self
                .active
                .last()
                .unwrap()
                .names
                .get(&annotation.name.text)
                .copied();
            self.occurrence(&annotation.name, OccurrenceKind::Annotation, binding);
        }
        self.statements(&function.body);
        self.leave_scope();
    }
}

/// A missing key denotes a runtime-computed key, not a guessed field spelling.
fn access_path<'a>(expression: &'a Expr, keys: &mut Vec<Option<&'a [u8]>>) -> Option<&'a Name> {
    match &expression.kind {
        ExprKind::Name(name) => Some(name),
        ExprKind::Parenthesized(inner) => access_path(inner, keys),
        ExprKind::Field { base, name } => {
            let root = access_path(base, keys)?;
            keys.push(Some(name.text.as_bytes()));
            Some(root)
        }
        ExprKind::Index { base, index } => {
            let root = access_path(base, keys)?;
            keys.push(string_key(index));
            Some(root)
        }
        _ => None,
    }
}

fn string_key(expression: &Expr) -> Option<&[u8]> {
    match &expression.kind {
        ExprKind::String(bytes) => Some(bytes),
        ExprKind::Parenthesized(inner) => string_key(inner),
        _ => None,
    }
}

pub(crate) fn modifies_package(keys: &[Option<&[u8]>]) -> bool {
    matches!(
        keys.first(),
        None | Some(None)
            | Some(Some(
                b"path" | b"cpath" | b"loaded" | b"preload" | b"searchers" | b"loaders"
            ))
    )
}
