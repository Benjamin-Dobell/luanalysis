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
    pub comments: Vec<Comment>,
    pub diagnostics: Vec<Diagnostic>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub span: Span,
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
        function: Function,
    },
    /// Retained as a list so multiple return packs can be implemented later.
    Return(Vec<Expr>),
    /// Lua permits call expressions as statements, but not arbitrary expressions.
    Call(Expr),
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
    Function(Function),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub params: Vec<Name>,
    pub body: Vec<Stmt>,
    pub annotations: FunctionAnnotations,
    pub span: Span,
    /// Original source between the closing parameter `)` and the final `end`.
    pub body_span: Span,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionAnnotations {
    pub params: Vec<ParamAnnotation>,
    pub returns: Option<TypeAnnotation>,
    pub type_annotation: Option<TypeAnnotation>,
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
    pub name: Name,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeAnnotation {
    /// A `@type A, B` annotation corresponds to successive declared variables.
    pub types: Vec<TypeExpr>,
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
    Union(Vec<TypeExpr>),
    Record(Vec<TypeField>),
    Parenthesized(Box<TypeExpr>),
    Function(FunctionType),
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionType {
    pub params: Vec<FunctionTypeParam>,
    pub returns: Vec<TypeExpr>,
    pub span: Span,
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
    pub name: Name,
    pub ty: TypeExpr,
    pub span: Span,
}

#[cfg(test)]
mod tests;
