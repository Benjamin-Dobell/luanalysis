//! A native, source-preserving Lua and Luanalysis annotation frontend.
//!
//! This first slice deliberately accepts local declarations, assignments,
//! scalar literals, named table fields, property access, lexical blocks,
//! functions, calls, returns, and Luanalysis function/type annotations.
//! Unsupported syntax is diagnosed;
//! it is never translated to TypeScript or treated as successfully checked.

mod lexer;
mod parser;
mod types;
mod documentation;
mod navigation;
mod completion_metadata;
#[cfg(test)]
mod completion_metadata_tests;
#[cfg(test)]
mod documentation_reference_tests;
#[cfg(test)]
mod diagnostic_origin_tests;

pub use lexer::{Lexed, Token, TokenKind, identifier_continue, identifier_start, is_identifier, lex};
pub use parser::{parse, parse_with_checkpoint, parse_with_options};
/// Whether a backtick's text could name a generic that captures a type by name (`\`T\``).
pub(crate) fn is_capture_name(text: &str) -> bool {
    text.split('.').all(is_identifier)
}

pub use types::{DECLARED_TYPES, DECLARED_TYPE_NAMES, emmylua_builtin_type};
pub use documentation::{Documentation, documentation_above, documentation_at};
pub use navigation::{AnnotationDeclaration, ConditionalDeclaration, InferDeclaration, MappedDeclaration, NilLaterUnion, TypeName, TypeNameKind, annotation_types, nil_later_unions, referenced_type_names, type_names, type_query_roots};

/// Half-open UTF-8 byte offsets in the original Lua source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn join(self, other: Self) -> Self {
        Self::new(self.start.min(other.start), self.end.max(other.end))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub origin: DiagnosticOrigin,
}

/// The language whose source grammar or metadata produced a diagnostic.
/// Annotation failures retain their original nodes without invalidating the
/// surrounding Lua grammar; both kinds remain reported to the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticOrigin {
    LuaSyntax,
    Annotation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentKind {
    Line,
    Long,
    Documentation,
    Shebang,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub span: Span,
    pub text: String,
    pub kind: CommentKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub statements: Vec<Stmt>,
    /// Inline documentation describes an original expression without changing
    /// Lua syntax, lvalue identity or multiple-result adjustment.
    pub expression_assertions: Vec<ExpressionAssertion>,
    /// Documentation declarations are independent from runtime statements and
    /// remain available when standalone, at EOF, or grouped with other tags.
    pub type_declarations: Vec<NamedTypeDeclaration>,
    /// Bodyless documentation functions own original signatures independently
    /// from runtime Lua functions and their bodies.
    pub function_declarations: Vec<DocFunctionDeclaration>,
    /// Explicit @field Owner.member augmentations do not declare another named type.
    pub type_extensions: Vec<TypeExtension>,
    /// Canonical original @overload heads. Documentation/function associations
    /// reference these nodes instead of cloning signature-local generic scopes.
    pub overload_annotations: Vec<OverloadAnnotation>,
    /// `---@operator` declarations of the named types in their documentation groups.
    pub operator_annotations: Vec<OperatorAnnotation>,
    /// Original documentation links. These do not introduce annotation types
    /// or runtime reads; the source identities remain available to editor hosts.
    pub documentation_references: Vec<DocumentationReference>,
    /// Written contracts on original return lists, separate from function docs.
    pub return_annotations: Vec<ReturnAnnotation>,
    /// The names of locals that EmmyLua Analyzer's `---@readonly` declares: their tables'
    /// fields are not assigned.
    pub readonly_locals: Vec<Span>,
    /// Explicit type arguments of calls, `f--[[@<T>]](x)` (EmmyLua Analyzer's spelling of
    /// TypeScript's `f<T>(x)`), by the span of the call they apply to.
    pub call_type_arguments: Vec<CallTypeArguments>,
    /// LuaLS's `---@private`, `---@protected`, `---@package` and `---@public` above a statement
    /// that stores a member, `function T.f()` or `T.x = v`: that member's visibility.
    pub member_visibilities: Vec<MemberVisibility>,
    /// LuaLS's and EmmyLua Analyzer's `---@diagnostic disable-next-line: codes` and its kin
    /// (`ParseOptions::emmylua`): the source whose diagnostics they disable.
    pub diagnostic_suppressions: Vec<DiagnosticSuppression>,
    /// EmmyLua Analyzer's `---@cast obj.value +T`: from the cast to the end of its block, a
    /// store to that field path may also be a `T`.
    pub field_widenings: Vec<FieldWidening>,
    /// `---@namespace N`, as EmmyLua Analyzer writes it: the file's named types are declared as
    /// `N.Name`, and a type name it writes is `N.Name` where one is declared.
    pub namespace: Option<String>,
    /// EmmyLua Analyzer's `---@using N`: a type name the file writes may also be `N.Name`.
    pub usings: Vec<String>,
    pub comments: Vec<Comment>,
    pub diagnostics: Vec<Diagnostic>,
    pub span: Span,
    /// How the file's annotations were read, as its comments are read again.
    pub options: ParseOptions,
}

/// [`File::diagnostic_suppressions`]: no diagnostic beginning in `span` is reported, or, when
/// `codes` lists some, none of theirs. A code is Luanalysis's (`TS2322`) or the LuaLS and
/// EmmyLua Analyzer name of the check it corresponds to (`assign-type-mismatch`).
#[derive(Clone, Debug, PartialEq)]
pub struct DiagnosticSuppression {
    pub span: Span,
    pub codes: Vec<String>,
}

impl File {
    /// Whether a `---@diagnostic` directive disables the diagnostic `code` that begins at `at`.
    pub fn suppresses(&self, code: &str, at: usize) -> bool {
        self.diagnostic_suppressions.iter().any(|suppression| {
            (suppression.span.start..suppression.span.end).contains(&at)
                && (suppression.codes.is_empty() || suppression.codes.iter().any(|name| name == code || diagnostic_codes(name).contains(&code)))
        })
    }
}

/// The Luanalysis codes of a LuaLS or EmmyLua Analyzer diagnostic name.
fn diagnostic_codes(name: &str) -> &'static [&'static str] {
    match name {
        "undefined-global" | "undefined-doc-name" | "type-not-found" => &["TS2304"],
        "undefined-field" | "inject-field" => &["TS2339", "TS2551", "TS7053"],
        "assign-type-mismatch" | "cast-local-type" | "return-type-mismatch" | "missing-return-value" | "redundant-return-value" => &["TS2322"],
        "param-type-mismatch" | "param-type-not-match" => &["TS2345", "TS2769"],
        "missing-parameter" | "redundant-parameter" => &["TS2554", "TS2555", "TS2556"],
        "missing-return" => &["TS2355", "TS2366"],
        "need-check-nil" => &["TS18047", "TS18048", "TS2531", "TS2532", "TS2533"],
        "duplicate-index" => &["TS1117"],
        "access-invisible" | "invisible" => &["TS2341", "TS2445"],
        "call-non-callable" => &["TS2349"],
        "generic-constraint-mismatch" => &["TS2344", "TS2345"],
        "syntax-error" | "doc-syntax-error" | "luadoc-miss-type-name" => &["LUA1001"],
        _ => &[],
    }
}

/// [`File::field_widenings`]: a store to `path` within `scope` may also be a `ty`.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldWidening {
    pub path: Vec<String>,
    pub ty: TypeAnnotation,
    pub scope: Span,
}

/// Which annotation syntax is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParseOptions {
    /// EmmyLua Analyzer Rust's own forms, opt-in: `A - B` and `T - ?`, `sync fun(...)`,
    /// `---@return_overload`, `T?` outside results and tuple elements (otherwise reported: a
    /// parameter's or field's is then its name's) and signature tags above a local whose value
    /// is no function. It does not decay optionals.
    pub emmylua: bool,
    /// `T?` is `nil | T` everywhere, as EmmyLua Analyzer and lua-language-server read it, rather
    /// than a value that may be absent: a `T?` result may then be nil, and a `T?` parameter is a
    /// nil-admitting one. The checker reads absence as nil too, so such a final result or
    /// parameter may also be left out.
    pub decay_optionals: bool,
}

impl File {
    pub fn has_lua_syntax_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.origin == DiagnosticOrigin::LuaSyntax)
    }

    /// A LuaLS definition file's `---@meta` tag, and the module it provides to `require`
    /// when it names one. `---@meta _` provides none.
    pub fn meta(&self) -> Option<Option<&str>> {
        self.comments.iter().find_map(|comment| {
            let tag = comment.text.strip_prefix("---")?.trim_start_matches(['-', ' ', '\t']).strip_prefix("@meta")?;
            if tag.starts_with(|ch: char| !ch.is_whitespace()) {
                return None;
            }
            Some(tag.split_whitespace().next().filter(|name| *name != "_"))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpressionAssertionKind {
    Type,
    Not,
    /// EmmyLua Analyzer's `---@cast x +T`: the type with `T` added.
    Add,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpressionAssertion {
    pub kind: ExpressionAssertionKind,
    pub annotation: TypeAnnotation,
    pub expression: Span,
    pub span: Span,
}

/// A visibility tag above a statement, for the member its target stores.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemberVisibility {
    pub target: Span,
    pub visibility: FieldVisibilityAnnotation,
}

/// `--[[@<A, B>]]` between a callee and its arguments: the call's type arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct CallTypeArguments {
    pub annotation: TypeAnnotation,
    pub call: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NamedTypeDeclarationId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct NamedTypeDeclaration {
    pub id: NamedTypeDeclarationId,
    pub name: Name,
    pub generics: Vec<GenericParameter>,
    pub kind: NamedTypeDeclarationKind,
    pub span: Span,
    /// Entire contiguous documentation group, including associated fields.
    pub group_span: Span,
    /// Original following runtime statement, when directly attached. An alias
    /// never acquires runtime declaration semantics through this metadata.
    pub attachment: Option<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NamedTypeDeclarationKind {
    Alias {
        target: TypeExpr,
    },
    Record {
        flavor: NamedRecordKind,
        /// `---@class A: B, C`: the record's supers, as LuaLS and EmmyLua Analyzer list them.
        bases: Vec<TypeExpr>,
        fields: Vec<FieldDeclaration>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedRecordKind {
    Shape,
    Class,
    Module,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDeclaration {
    pub key: TypeFieldKey,
    pub ty: TypeExpr,
    pub visibility: Option<FieldVisibilityAnnotation>,
    /// `---@field readonly name T`: the field cannot be assigned.
    pub readonly: Option<Span>,
    /// `---@field name? T`: the field may be absent, as well as nil.
    pub optional: bool,
    pub explicit_owner: Option<Name>,
    /// Original `<Owner>` spelling, retained for deprecated syntax tooling.
    pub deprecated_owner_span: Option<Span>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocFunctionDeclarationId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct DocFunctionDeclaration {
    pub id: DocFunctionDeclarationId,
    pub name: Name,
    pub owner: Option<DocFunctionOwner>,
    /// An ambiguous contextual owner or ownerless colon is retained for
    /// recovery, and must not publish an ordinary global declaration.
    pub invalid_owner: bool,
    /// Original `:` before the member name; implicit self is semantic metadata,
    /// never an invented source parameter.
    pub method_colon: Option<Span>,
    pub signature: FunctionType,
    pub annotations: FunctionAnnotations,
    pub span: Span,
    pub tag_span: Span,
    pub group_span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocFunctionOwner {
    Explicit(Name),
    Contextual(NamedTypeDeclarationId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldVisibility {
    Public,
    Protected,
    Private,
    /// LuaLS: visible in the file that declares it.
    Package,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldVisibilityAnnotation {
    pub kind: FieldVisibility,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeExtension {
    pub owner: Name,
    pub fields: Vec<FieldDeclaration>,
    pub span: Span,
    pub group_span: Span,
    pub attachment: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentationReference {
    pub target: Name,
    pub member: Option<Name>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReturnAnnotation {
    pub statement: Span,
    pub annotation: TypeAnnotation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// The names enter scope after ALL initializers have been evaluated.
    Local {
        names: Vec<Name>,
        /// Each name's Lua 5.4 attribute: `const` or `close` (`local f <close> = ...`).
        attributes: Vec<Option<Name>>,
        values: Vec<Expr>,
        annotation: Option<TypeAnnotation>,
    },
    Assign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
        annotation: Option<TypeAnnotation>,
    },
    Block {
        body: Vec<Stmt>,
        body_span: Span,
    },
    /// The name is in scope inside its own function body, unlike `local f = ...`.
    LocalFunction {
        name: Name,
        function: Box<Function>,
    },
    /// A Lua named definition assigns its name/field target. Colon definitions
    /// additionally bind an implicit receiver; explicit params stay source-only.
    FunctionDeclaration {
        target: Box<Expr>,
        function: Box<Function>,
        method_span: Option<Span>,
    },
    /// Retained as a list so multiple return packs can be implemented later.
    Return(Vec<Expr>),
    /// Lua permits call expressions as statements, but not arbitrary expressions.
    Call(Expr),
    If {
        branches: Vec<IfBranch>,
        else_branch: Option<ElseBranch>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
        body_span: Span,
    },
    /// The body's local declarations remain visible in the `until` condition.
    Repeat {
        body: Vec<Stmt>,
        condition: Expr,
        body_span: Span,
    },
    NumericFor {
        name: Name,
        /// The variable's written type: LuaLS's `---@type T` above the loop. Boxed, as a statement's
        /// other parts are, so that it does not enlarge every statement.
        name_type: Option<Box<TypeExpr>>,
        start: Box<Expr>,
        limit: Box<Expr>,
        step: Option<Box<Expr>>,
        body: Vec<Stmt>,
        body_span: Span,
    },
    GenericFor {
        names: Vec<Name>,
        /// Each variable's written type, by its name: `---@param name T` above the loop, as
        /// Luanalysis v1 and LuaLS read it.
        name_types: Vec<Option<TypeExpr>>,
        values: Vec<Expr>,
        body: Vec<Stmt>,
        body_span: Span,
    },
    /// `goto name` (Lua 5.2 and later): jumps to the visible label `name` of the same function.
    Goto(Name),
    /// `::name::`: a label, visible in its block and the blocks nested in it.
    Label(Name),
    Break,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfBranch {
    pub condition: Expr,
    pub body: Vec<Stmt>,
    pub span: Span,
    pub body_span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ElseBranch {
    pub body: Vec<Stmt>,
    pub span: Span,
    pub body_span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

/// An expression of a parsed file that is shared, not copied: the handle keeps the file's
/// immutable tree alive and reads the expression in place. A copy of an expression copies its
/// whole subtree, and a function expression's body with it.
#[derive(Clone)]
pub struct SharedExpr {
    tree: std::sync::Arc<File>,
    expression: std::ptr::NonNull<Expr>,
}

// SAFETY: a `SharedExpr` only reads its expression, in a tree that is never mutated while an
// `Arc` shares it, and the tree is `Send` and `Sync`.
unsafe impl Send for SharedExpr where File: Send + Sync {}
// SAFETY: as for `Send`: shared access only reads.
unsafe impl Sync for SharedExpr where File: Send + Sync {}

impl SharedExpr {
    /// The expression `expression` of `tree`.
    ///
    /// # Safety
    ///
    /// `expression` must be an expression of `tree`'s own syntax, reached from `&**tree`, not a
    /// copy of one or an expression of another tree.
    pub unsafe fn new(tree: &std::sync::Arc<File>, expression: &Expr) -> Self {
        Self { tree: std::sync::Arc::clone(tree), expression: std::ptr::NonNull::from(expression) }
    }

    /// The file the expression is in.
    pub fn tree(&self) -> &std::sync::Arc<File> {
        &self.tree
    }
}

impl std::ops::Deref for SharedExpr {
    type Target = Expr;

    fn deref(&self) -> &Expr {
        // SAFETY: `new`'s contract: the expression is in `tree`, which this handle keeps alive
        // and which nothing mutates while shared.
        unsafe { self.expression.as_ref() }
    }
}

impl std::borrow::Borrow<Expr> for SharedExpr {
    fn borrow(&self) -> &Expr {
        self
    }
}

impl std::fmt::Debug for SharedExpr {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(formatter)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Nil,
    Boolean(bool),
    /// Original spelling retained, including hexadecimal forms.
    Number(String),
    /// Lua strings are byte sequences, including escaped non-UTF-8 bytes.
    String(Vec<u8>),
    Name(Name),
    Parenthesized(Box<Expr>),
    Table(Vec<TableField>),
    Field {
        base: Box<Expr>,
        name: Name,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Unary {
        op: UnaryOperator,
        op_span: Span,
        value: Box<Expr>,
    },
    Binary {
        op: BinaryOperator,
        op_span: Span,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Function(Box<Function>),
    /// An original `...` expression; arity/expansion belongs to the pack adapter.
    Vararg,
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    MethodCall {
        receiver: Box<Expr>,
        name: Name,
        args: Vec<Expr>,
    },
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOperator {
    Not,
    Length,
    Negate,
    BitwiseNot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Power,
    Concat,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub params: Vec<Name>,
    /// The exact source `...` token in the runtime parameter list, if present.
    pub vararg: Option<Span>,
    pub body: Vec<Stmt>,
    pub annotations: FunctionAnnotations,
    pub span: Span,
    /// Original source between the closing parameter `)` and the final `end`.
    pub body_span: Span,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionAnnotations {
    pub overloads: Vec<OverloadReference>,
    pub generics: Vec<GenericParameter>,
    pub params: Vec<ParamAnnotation>,
    pub vararg: Option<Box<VarargAnnotation>>,
    pub returns: Option<TypeAnnotation>,
    pub type_annotation: Option<TypeAnnotation>,
    /// EmmyLua Analyzer's `---@return_overload` rows, each one result list a call may return.
    /// Once a comment's tags are read they are its result contract, the union of the rows.
    pub return_overloads: Vec<TypeAnnotation>,
    /// EmmyLua Analyzer's `---@readonly` on a local: its table's fields are not assigned.
    pub readonly: Option<Span>,
    /// LuaLS's `---@private`, `---@protected`, `---@package` or `---@public`, which the statement
    /// it documents gives the member it stores (`File::member_visibilities`).
    pub visibility: Option<FieldVisibilityAnnotation>,
    /// EmmyLua Analyzer's `---@return_cast x T`: the predicate its first result is.
    pub return_cast: Option<Box<TypeExpr>>,
    /// LuaLS's and EmmyLua Analyzer's `---@module "name"`: the local it documents is that module.
    pub module: Option<Box<ModuleAnnotation>>,
}

/// [`FunctionAnnotations::module`]: the module's name, as `require` takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleAnnotation {
    pub name: String,
    /// The `@module` tag.
    pub tag_span: Span,
    /// The name's string.
    pub span: Span,
}

impl FunctionAnnotations {
    /// Whether any part of the function's signature is written, rather than inferred.
    pub fn types_signature(&self) -> bool {
        !self.overloads.is_empty() || !self.params.is_empty() || self.vararg.is_some()
            || self.returns.is_some() || self.type_annotation.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VarargAnnotation {
    pub ty: TypeExpr,
    pub span: Span,
    /// Present for function-type `...: T`, absent for `@vararg` / `vararg T`.
    pub ellipsis_span: Option<Span>,
    /// `...: T...` and `---@param ... T...` (EmmyLua Analyzer's variadic generics, TypeScript's
    /// `...args: T`): the varargs are the sequence `T`, a pack type parameter or a tuple, rather
    /// than each a `T`.
    pub spread: Option<Span>,
}

impl VarargAnnotation {
    /// Whether these varargs are the sequence `name`: `...: T...` or `...: infer T...`.
    pub fn spreads(&self, name: &str) -> bool {
        self.spread.is_some() && match &self.ty.kind {
            TypeExprKind::Named(spread) => spread == name,
            TypeExprKind::Infer { name: spread, .. } => spread.text == name,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenericParameter {
    pub name: Name,
    /// EmmyLua Analyzer's `---@generic const T`, TypeScript's `<const T>`: literal arguments
    /// keep their literal types.
    pub constant: bool,
    pub constraint: Option<TypeExpr>,
    /// `T = Default`: the type an omitted or uninferred argument takes.
    pub default: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamAnnotation {
    pub name: Name,
    pub ty: TypeExpr,
    pub optional: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableField {
    pub kind: TableFieldKind,
    pub value: Expr,
    /// Written declaration contract on this original constructor field. This
    /// describes the stored member, independently from a cast on its value.
    pub annotation: Option<TypeAnnotation>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TableFieldKind {
    Named(Name),
    /// The original key expression; numeric/string normalization is semantic.
    Keyed(Box<Expr>),
    /// One-based ordinal among only the constructor's sequence fields.
    Sequence {
        index: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeAnnotation {
    /// A `@type A, B` annotation corresponds to successive declared variables.
    pub types: Vec<TypeExpr>,
    /// Advanced alternative/variadic packs refer to this annotation's `types`.
    /// Ordinary scalar unions and simple fixed lists retain `None`.
    pub packs: Option<Box<TypePacks>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeExprKind {
    Named(String),
    Literal(TypeLiteral),
    /// Suggested editor text; it contributes no values to its union type.
    CompletionHint(String),
    /// TypeScript's template literal type, `` `prefix${T}suffix` ``: the strings made of `texts`
    /// around each of `types`; `texts` has one more entry than `types`.
    TemplateLiteral { texts: Vec<String>, types: Vec<TypeExpr> },
    Union(Vec<TypeExpr>),
    /// `A & B`: what is each of its types at once. It binds more tightly than `|`.
    Intersection(Vec<TypeExpr>),
    Record(Vec<TypeField>),
    Parenthesized(Box<TypeExpr>),
    Function(FunctionType),
    /// A sole boolean result that narrows a named parameter when it is true, or with
    /// `asserts`, a function that returns only when the parameter is of `ty`, or when there is
    /// no `ty`, truthy.
    TypePredicate {
        parameter: Name,
        ty: Option<Box<TypeExpr>>,
        asserts: Option<Span>,
    },
    /// `keyof T`: the union of T's keys.
    KeyOf(Box<TypeExpr>),
    /// EmmyLua Analyzer's conditional type, `Check extends Constraint and Yes or No`
    /// (TypeScript's `Check extends Constraint ? Yes : No`).
    Conditional {
        check: Box<TypeExpr>,
        extends: Box<TypeExpr>,
        yes: Box<TypeExpr>,
        no: Box<TypeExpr>,
    },
    /// `infer P` in a conditional type's constraint: a type parameter its true branch can use.
    /// `infer P extends C` constrains it.
    Infer { name: Name, constraint: Option<Box<TypeExpr>> },
    /// TypeScript's (and EmmyLua Analyzer's) mapped type, `{ [P in K as N]?: T }`: a property for
    /// each key `P` of `K`, named `N` when there is an `as` clause, of type `T`.
    Mapped(Box<MappedType>),
    /// `T[K]`: the type of T's values at keys of type K.
    IndexedAccess {
        object: Box<TypeExpr>,
        index: Box<TypeExpr>,
    },
    /// `typeof name.member`: the declared type of a value in scope.
    TypeQuery(Vec<Name>),
    Array {
        element: Box<TypeExpr>,
        suffix_span: Span,
    },
    Generic {
        base: Name,
        args: Vec<TypeExpr>,
        args_span: Span,
    },
    Error,
}

impl TypeExpr {
    /// The original value-bearing node after dropping only completion metadata.
    /// Parentheses remain visible to syntax-sensitive overload ordering.
    pub fn value_syntax(&self) -> &Self {
        let mut current = self;
        while let TypeExprKind::Union(parts) = &current.kind {
            if !parts.iter().any(Self::is_completion_only) { break; }
            let mut values = parts.iter().filter(|part| !part.is_completion_only());
            let Some(value) = values.next() else { break };
            if values.next().is_some() { break; }
            current = value;
        }
        current
    }

    /// Syntax-only classification for metadata that needs a companion type.
    pub fn is_completion_only(&self) -> bool {
        match &self.kind {
            TypeExprKind::CompletionHint(_) => true,
            TypeExprKind::Parenthesized(inner) => inner.is_completion_only(),
            TypeExprKind::Union(parts) => parts.iter().all(Self::is_completion_only),
            _ => false,
        }
    }

    /// The types written directly in this one, in source order.
    pub fn children(&self) -> Vec<&TypeExpr> {
        match &self.kind {
            TypeExprKind::Parenthesized(element) | TypeExprKind::Array { element, .. }
            | TypeExprKind::TypePredicate { ty: Some(element), .. } | TypeExprKind::KeyOf(element)
            | TypeExprKind::Infer { constraint: Some(element), .. } => vec![element],
            TypeExprKind::IndexedAccess { object, index } => vec![object, index],
            TypeExprKind::Conditional { check, extends, yes, no } => vec![check, extends, yes, no],
            TypeExprKind::Mapped(mapped) => [&mapped.constraint].into_iter().chain(&mapped.name).chain([&mapped.ty]).collect(),
            TypeExprKind::Union(types) | TypeExprKind::Intersection(types) | TypeExprKind::Generic { args: types, .. }
            | TypeExprKind::TemplateLiteral { types, .. } => types.iter().collect(),
            TypeExprKind::Record(fields) => fields
                .iter()
                .flat_map(|field| match &field.key {
                    TypeFieldKey::Indexed(key) => vec![key, &field.ty],
                    _ => vec![&field.ty],
                })
                .collect(),
            TypeExprKind::Function(function) => function
                .generics
                .iter()
                .flat_map(|parameter| parameter.constraint.iter().chain(&parameter.default))
                .chain(function.params.iter().filter_map(|param| param.ty.as_ref()))
                .chain(function.vararg.iter().map(|vararg| &vararg.ty))
                .chain(&function.returns)
                .collect(),
            TypeExprKind::Named(_) | TypeExprKind::Literal(_) | TypeExprKind::CompletionHint(_)
            | TypeExprKind::TypePredicate { ty: None, .. } | TypeExprKind::Infer { constraint: None, .. } | TypeExprKind::TypeQuery(_)
            | TypeExprKind::Error => Vec::new(),
        }
    }

    /// [`TypeExpr::children`], to change them.
    pub fn children_mut(&mut self) -> Vec<&mut TypeExpr> {
        match &mut self.kind {
            TypeExprKind::Parenthesized(element) | TypeExprKind::Array { element, .. }
            | TypeExprKind::TypePredicate { ty: Some(element), .. } | TypeExprKind::KeyOf(element)
            | TypeExprKind::Infer { constraint: Some(element), .. } => vec![element],
            TypeExprKind::IndexedAccess { object, index } => vec![object, index],
            TypeExprKind::Conditional { check, extends, yes, no } => vec![check, extends, yes, no],
            TypeExprKind::Mapped(mapped) => [&mut mapped.constraint].into_iter().chain(&mut mapped.name).chain([&mut mapped.ty]).collect(),
            TypeExprKind::Union(types) | TypeExprKind::Intersection(types) | TypeExprKind::Generic { args: types, .. }
            | TypeExprKind::TemplateLiteral { types, .. } => types.iter_mut().collect(),
            TypeExprKind::Record(fields) => fields
                .iter_mut()
                .flat_map(|field| match &mut field.key {
                    TypeFieldKey::Indexed(key) => vec![&mut **key, &mut field.ty],
                    _ => vec![&mut field.ty],
                })
                .collect(),
            TypeExprKind::Function(function) => function
                .generics
                .iter_mut()
                .flat_map(|parameter| parameter.constraint.iter_mut().chain(&mut parameter.default))
                .chain(function.params.iter_mut().filter_map(|param| param.ty.as_mut()))
                .chain(function.vararg.iter_mut().map(|vararg| &mut vararg.ty))
                .chain(&mut function.returns)
                .collect(),
            TypeExprKind::Named(_) | TypeExprKind::Literal(_) | TypeExprKind::CompletionHint(_)
            | TypeExprKind::TypePredicate { ty: None, .. } | TypeExprKind::Infer { constraint: None, .. } | TypeExprKind::TypeQuery(_)
            | TypeExprKind::Error => Vec::new(),
        }
    }

    /// Whether a function type in this type spreads `name` as its varargs, `fun(...: T...)` or
    /// `fun(...: infer T...)`: whether `name` is a sequence of values there.
    pub fn spreads(&self, name: &str) -> bool {
        let mut pending = vec![self];
        while let Some(ty) = pending.pop() {
            if let TypeExprKind::Function(function) = &ty.kind
                && function.vararg.as_ref().is_some_and(|vararg| vararg.spreads(name))
            {
                return true;
            }
            pending.extend(ty.children());
        }
        false
    }

    /// The `infer P` declarations of the conditional type whose extends type this is, in source
    /// order: those written in it, except in the extends type of a conditional type nested in
    /// it, whose own they are. A name may be declared more than once.
    pub fn infer_declarations(&self) -> Vec<InferDeclaration> {
        let mut names = Vec::new();
        let mut pending = vec![self];
        while let Some(ty) = pending.pop() {
            match &ty.kind {
                TypeExprKind::Infer { name, constraint } => {
                    names.push(InferDeclaration { name: name.clone(), constraint: constraint.as_deref().cloned() });
                    pending.extend(constraint.as_deref());
                }
                TypeExprKind::Conditional { check, yes, no, .. } => pending.extend([no, yes, check].map(|ty| ty.as_ref())),
                _ => pending.extend(ty.children().into_iter().rev()),
            }
        }
        names
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionType {
    pub generics: Vec<GenericParameter>,
    /// Absent for bare `fun`; `fun()` has an explicit zero-parameter header.
    pub parameters_span: Option<Span>,
    pub params: Vec<FunctionTypeParam>,
    pub vararg: Option<Box<VarargAnnotation>>,
    /// Original colon and result syntax. None leaves the result unconstrained;
    /// a written `: void` still establishes an explicit result contract.
    pub returns_span: Option<Span>,
    pub returns: Vec<TypeExpr>,
    /// Pack indices refer to `returns`, without duplicating type nodes.
    pub packs: Option<Box<TypePacks>>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OverloadDeclarationId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct OverloadReference {
    pub id: OverloadDeclarationId,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverloadAnnotation {
    pub id: OverloadDeclarationId,
    pub signature: Box<FunctionType>,
    pub span: Span,
    pub tag_span: Span,
    pub group_span: Span,
    /// Neutral documentation candidates. The registry selects semantic owner;
    /// multiple candidates are not silently resolved by source order.
    pub named_candidates: Vec<NamedTypeDeclarationId>,
    /// Written into a documented local's type as one of its call signatures, so no function or
    /// declaration owns it.
    pub in_type: bool,
}

/// A Lua operator a named type declares with `---@operator`. `call` is a class
/// `@overload`, and `==` always compares identity, so neither appears here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LuaOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Power,
    Concat,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    Negate,
    BitwiseNot,
    Length,
    Less,
    LessEqual,
}

impl LuaOperator {
    /// The metamethod name, without its `__` prefix, as `---@operator` spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Subtract => "sub",
            Self::Multiply => "mul",
            Self::Divide => "div",
            Self::FloorDivide => "idiv",
            Self::Modulo => "mod",
            Self::Power => "pow",
            Self::Concat => "concat",
            Self::BitwiseAnd => "band",
            Self::BitwiseOr => "bor",
            Self::BitwiseXor => "bxor",
            Self::ShiftLeft => "shl",
            Self::ShiftRight => "shr",
            Self::Negate => "unm",
            Self::BitwiseNot => "bnot",
            Self::Length => "len",
            Self::Less => "lt",
            Self::LessEqual => "le",
        }
    }

    pub fn unary(self) -> bool {
        matches!(self, Self::Negate | Self::BitwiseNot | Self::Length)
    }
}

/// `---@operator add(Other): Result`: the declared type's `+` with a right operand
/// assignable to `Other` gives `Result`. Its signature is `fun(other: Other): Result`,
/// or `fun(): Result` for a unary operator.
#[derive(Clone, Debug, PartialEq)]
pub struct OperatorAnnotation {
    pub operator: LuaOperator,
    pub signature: Box<FunctionType>,
    pub span: Span,
    pub tag_span: Span,
    pub group_span: Span,
    /// The named declarations in the same documentation group, as for `@overload`.
    pub named_candidates: Vec<NamedTypeDeclarationId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeLiteral {
    Number(String),
    String(Vec<u8>),
    Boolean(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypePacks {
    pub alternatives: Vec<TypePack>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypePack {
    /// Fixed slot indices in the containing canonical `types`/`returns` vector.
    pub fixed: std::ops::Range<usize>,
    pub variadic: Option<VariadicTypeIndex>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariadicTypeIndex {
    pub type_index: usize,
    pub ellipsis_span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionTypeParam {
    pub name: Name,
    pub ty: Option<TypeExpr>,
    pub optional: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MappedType {
    pub readonly: MappedModifier,
    pub parameter: Name,
    /// `K`: the keys `P` ranges over.
    pub constraint: TypeExpr,
    /// `as N`
    pub name: Option<TypeExpr>,
    pub optional: MappedModifier,
    /// `T`: each property's type.
    pub ty: TypeExpr,
}

/// A mapped type's `readonly` or `?`: absent, added (`+` or bare) or removed (`-`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappedModifier {
    None,
    Add,
    /// `+readonly`, `+?`: the same as `Add`, written so.
    Plus,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeField {
    pub key: TypeFieldKey,
    pub ty: TypeExpr,
    /// `{ readonly name: T }`: the field cannot be assigned.
    pub readonly: Option<Span>,
    /// `{ name?: T }`: the field may be absent, as in TypeScript and EmmyLua Analyzer.
    pub optional: Option<Span>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeFieldKey {
    Named(Name),
    /// Original literal or index-domain type, including numeric/byte-string keys.
    Indexed(Box<TypeExpr>),
    /// One-based ordinal among bare tuple-like record type fields.
    Sequence {
        index: usize,
    },
}

#[cfg(test)]
mod assertion_tests;
#[cfg(test)]
mod runtime_primitive_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod generic_tests;

#[cfg(test)]
mod pack_tests;
#[cfg(test)]
mod luals_dialect_tests;

#[cfg(test)]
mod table_type_tests;

#[cfg(test)]
mod declaration_tests;

#[cfg(test)]
mod function_documentation_tests;

#[cfg(test)]
mod trailing_type_tests;
