//! A native, source-preserving Lua and Luanalysis annotation frontend.
//!
//! This first slice deliberately accepts local declarations, assignments,
//! scalar literals, named table fields, property access, and `@type` annotations
//! containing primitives, unions, and records. Unsupported syntax is diagnosed;
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
    Error,
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
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeField {
    pub name: Name,
    pub ty: TypeExpr,
    pub span: Span,
}

#[cfg(test)]
mod tests;
