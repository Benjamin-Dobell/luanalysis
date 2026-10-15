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
mod completion_metadata;
#[cfg(test)]
mod completion_metadata_tests;
#[cfg(test)]
mod documentation_reference_tests;

pub use lexer::{Lexed, Token, TokenKind, lex};
pub use parser::parse;

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
    /// Explicit @field<Owner> augmentations do not declare another named type.
    pub type_extensions: Vec<TypeExtension>,
    /// Canonical original @overload heads. Documentation/function associations
    /// reference these nodes instead of cloning signature-local generic scopes.
    pub overload_annotations: Vec<OverloadAnnotation>,
    /// Original documentation links. These do not introduce annotation types
    /// or runtime reads; the source identities remain available to editor hosts.
    pub documentation_references: Vec<DocumentationReference>,
    /// Written contracts on original return lists, separate from function docs.
    pub return_annotations: Vec<ReturnAnnotation>,
    pub comments: Vec<Comment>,
    pub diagnostics: Vec<Diagnostic>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpressionAssertionKind {
    Type,
    Not,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpressionAssertion {
    pub kind: ExpressionAssertionKind,
    pub annotation: TypeAnnotation,
    pub expression: Span,
    pub span: Span,
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
        base: Option<TypeExpr>,
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
    pub explicit_owner: Option<Name>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldVisibility {
    Public,
    Protected,
    Private,
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
        start: Box<Expr>,
        limit: Box<Expr>,
        step: Option<Box<Expr>>,
        body: Vec<Stmt>,
        body_span: Span,
    },
    GenericFor {
        names: Vec<Name>,
        values: Vec<Expr>,
        body: Vec<Stmt>,
        body_span: Span,
    },
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct VarargAnnotation {
    pub ty: TypeExpr,
    pub span: Span,
    /// Present for function-type `...: T`, absent for `@vararg` / `vararg T`.
    pub ellipsis_span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenericParameter {
    pub name: Name,
    pub constraint: Option<TypeExpr>,
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
    Union(Vec<TypeExpr>),
    Record(Vec<TypeField>),
    Parenthesized(Box<TypeExpr>),
    Function(FunctionType),
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionType {
    pub generics: Vec<GenericParameter>,
    /// Absent for bare `fun`; `fun()` has an explicit zero-parameter header.
    pub parameters_span: Option<Span>,
    pub params: Vec<FunctionTypeParam>,
    pub vararg: Option<Box<VarargAnnotation>>,
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
pub struct TypeField {
    pub key: TypeFieldKey,
    pub ty: TypeExpr,
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
mod table_type_tests;

#[cfg(test)]
mod declaration_tests;

#[cfg(test)]
mod function_documentation_tests;

#[cfg(test)]
mod trailing_type_tests;
