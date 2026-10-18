//! Original Lua syntax weights for Bun's file-checking cost estimate.
//!
//! Bun counts identifier expressions and weights original type nodes separately
//! from source bytes. Lua's identifier-expression equivalents are the binder's
//! read/write occurrences: local/parameter declarations, documentation names,
//! and dotted property names are not expressions. Annotation types come from
//! their original source spans, not from the number of metadata associations.
//! Function-type roots are also represented directly by `FunctionType` for
//! overloads and bodyless declarations; count those roots once alongside the
//! `TypeExpr` roots. This maps Lua syntax to the same estimate, rather than
//! claiming Lua and TypeScript construct identical HIR nodes.

use crate::bind::{BoundFile, OccurrenceKind};
use luanalysis_syntax::{
    Expr, ExprKind, File, Function, FunctionAnnotations, FunctionType, GenericParameter,
    NamedTypeDeclarationKind, Span, Stmt, StmtKind, TableFieldKind, TypeAnnotation, TypeExpr,
    TypeExprKind, TypeFieldKey,
};
use crate::collections::HashSet;

pub(crate) fn checking_cost(file: &File, bound: &BoundFile, type_node_cost: usize) -> usize {
    let identifiers = bound
        .occurrences
        .iter()
        .filter(|occurrence| {
            matches!(
                occurrence.kind,
                OccurrenceKind::Read | OccurrenceKind::Write
            )
        })
        .count();
    let mut types = OriginalTypes::default();
    types.statements(&file.statements);
    for assertion in &file.expression_assertions {
        types.annotation(&assertion.annotation);
    }
    for declaration in &file.type_declarations {
        types.generics(&declaration.generics);
        match &declaration.kind {
            NamedTypeDeclarationKind::Alias { target } => types.ty(target),
            NamedTypeDeclarationKind::Record { base, fields, .. } => {
                if let Some(base) = base {
                    types.ty(base);
                }
                for field in fields {
                    types.key(&field.key);
                    types.ty(&field.ty);
                }
            }
        }
    }
    for declaration in &file.function_declarations {
        types.signature(&declaration.signature);
        types.function_annotations(&declaration.annotations);
    }
    for extension in &file.type_extensions {
        for field in &extension.fields {
            types.key(&field.key);
            types.ty(&field.ty);
        }
    }
    for overload in &file.overload_annotations {
        types.signature(&overload.signature);
    }
    for returned in &file.return_annotations {
        types.annotation(&returned.annotation);
    }
    identifiers + types.spans.len() * type_node_cost + 1
}

#[derive(Default)]
struct OriginalTypes {
    spans: HashSet<(usize, usize)>,
}

impl OriginalTypes {
    fn insert(&mut self, span: Span) -> bool {
        self.spans.insert((span.start, span.end))
    }

    fn ty(&mut self, ty: &TypeExpr) {
        if !self.insert(ty.span) {
            return;
        }
        match &ty.kind {
            TypeExprKind::Union(parts) | TypeExprKind::Intersection(parts) => {
                for part in parts {
                    self.ty(part);
                }
            }
            TypeExprKind::Record(fields) => {
                for field in fields {
                    self.key(&field.key);
                    self.ty(&field.ty);
                }
            }
            TypeExprKind::Parenthesized(inner) | TypeExprKind::Array { element: inner, .. }
            | TypeExprKind::TypePredicate { ty: inner, .. } => {
                self.ty(inner);
            }
            TypeExprKind::Generic { args, .. } => {
                for argument in args {
                    self.ty(argument);
                }
            }
            TypeExprKind::Function(signature) => self.signature_contents(signature),
            TypeExprKind::Named(_)
            | TypeExprKind::Literal(_)
            | TypeExprKind::CompletionHint(_)
            | TypeExprKind::Error => {}
        }
    }

    fn key(&mut self, key: &TypeFieldKey) {
        if let TypeFieldKey::Indexed(ty) = key {
            self.ty(ty);
        }
    }

    fn generics(&mut self, generics: &[GenericParameter]) {
        for generic in generics {
            if let Some(constraint) = &generic.constraint {
                self.ty(constraint);
            }
        }
    }

    fn annotation(&mut self, annotation: &TypeAnnotation) {
        // Pack alternatives reference these canonical nodes by index.
        for ty in &annotation.types {
            self.ty(ty);
        }
    }

    fn signature(&mut self, signature: &FunctionType) {
        if self.insert(signature.span) {
            self.signature_contents(signature);
        }
    }

    fn signature_contents(&mut self, signature: &FunctionType) {
        self.generics(&signature.generics);
        for parameter in &signature.params {
            if let Some(ty) = &parameter.ty {
                self.ty(ty);
            }
        }
        if let Some(vararg) = &signature.vararg {
            self.ty(&vararg.ty);
        }
        for returned in &signature.returns {
            self.ty(returned);
        }
    }

    fn function_annotations(&mut self, annotations: &FunctionAnnotations) {
        // Overload references point into File::overload_annotations, visited
        // once above; they do not introduce additional original type syntax.
        self.generics(&annotations.generics);
        for parameter in &annotations.params {
            self.ty(&parameter.ty);
        }
        if let Some(vararg) = &annotations.vararg {
            self.ty(&vararg.ty);
        }
        if let Some(returns) = &annotations.returns {
            self.annotation(returns);
        }
        if let Some(annotation) = &annotations.type_annotation {
            self.annotation(annotation);
        }
    }

    fn function(&mut self, function: &Function) {
        self.function_annotations(&function.annotations);
        self.statements(&function.body);
    }

    fn statements(&mut self, statements: &[Stmt]) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Local {
                    values, annotation, ..
                } => {
                    if let Some(annotation) = annotation {
                        self.annotation(annotation);
                    }
                    self.expressions(values);
                }
                StmtKind::Assign {
                    targets,
                    values,
                    annotation,
                } => {
                    if let Some(annotation) = annotation {
                        self.annotation(annotation);
                    }
                    self.expressions(targets);
                    self.expressions(values);
                }
                StmtKind::LocalFunction { function, .. } => self.function(function),
                StmtKind::FunctionDeclaration {
                    target, function, ..
                } => {
                    self.expression(target);
                    self.function(function);
                }
                StmtKind::Block { body, .. } => self.statements(body),
                StmtKind::Return(values) => self.expressions(values),
                StmtKind::GenericFor { values, body, .. } => {
                    self.expressions(values);
                    self.statements(body);
                }
                StmtKind::Call(expression) => self.expression(expression),
                StmtKind::If {
                    branches,
                    else_branch,
                } => {
                    for branch in branches {
                        self.expression(&branch.condition);
                        self.statements(&branch.body);
                    }
                    if let Some(branch) = else_branch {
                        self.statements(&branch.body);
                    }
                }
                StmtKind::While {
                    condition, body, ..
                }
                | StmtKind::Repeat {
                    condition, body, ..
                } => {
                    self.expression(condition);
                    self.statements(body);
                }
                StmtKind::NumericFor {
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    self.expression(start);
                    self.expression(limit);
                    if let Some(step) = step {
                        self.expression(step);
                    }
                    self.statements(body);
                }
                StmtKind::Break | StmtKind::Goto(_) | StmtKind::Label(_) => {}
            }
        }
    }

    fn expressions(&mut self, expressions: &[Expr]) {
        for expression in expressions {
            self.expression(expression);
        }
    }

    fn expression(&mut self, expression: &Expr) {
        match &expression.kind {
            ExprKind::Parenthesized(inner) | ExprKind::Unary { value: inner, .. } => {
                self.expression(inner);
            }
            ExprKind::Field { base, .. } => self.expression(base),
            ExprKind::Index { base, index }
            | ExprKind::Binary {
                left: base,
                right: index,
                ..
            } => {
                self.expression(base);
                self.expression(index);
            }
            ExprKind::Table(fields) => {
                for field in fields {
                    if let TableFieldKind::Keyed(key) = &field.kind {
                        self.expression(key);
                    }
                    if let Some(annotation) = &field.annotation {
                        self.annotation(annotation);
                    }
                    self.expression(&field.value);
                }
            }
            ExprKind::Function(function) => self.function(function),
            ExprKind::Call { callee, args } => {
                self.expression(callee);
                self.expressions(args);
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expression(receiver);
                self.expressions(args);
            }
            ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Name(_)
            | ExprKind::Vararg
            | ExprKind::Error => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(source: &str, weight: usize) -> usize {
        let file = luanalysis_syntax::parse(source);
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        checking_cost(&file, &crate::bind::bind(&file), weight)
    }

    #[test]
    fn source_bytes_do_not_replace_identifier_and_type_costs() {
        let original = "local value='short'\nlocal result=value\n";
        let comments = format!("--[[{}]]\n{original}", "comment ".repeat(16_384));
        let literal = format!(
            "local value='{}'\nlocal result=value\n",
            "literal ".repeat(16_384)
        );
        assert_eq!(cost(original, 8), 2);
        assert_eq!(cost(&comments, 8), cost(original, 8));
        assert_eq!(cost(&literal, 8), cost(original, 8));
        assert_eq!(
            cost("local value=source.field\nvalue=source[other]\n", 8),
            5
        );
        assert_eq!(cost("local value=source:dotted(other)\n", 8), 3);
        assert_eq!(cost("local a,b,c\nlocal function f(x,y) end\n", 8), 1);
    }

    #[test]
    fn declaration_only_types_and_nested_type_syntax_have_weight() {
        let source = "---@alias Lookup {[string]: number|nil}\n";
        // Record, index-domain string, union, number, nil.
        assert_eq!(cost(source, 1), 6);
        assert_eq!(cost(source, 8), 41);
        assert_eq!(cost(source, 0), 1);
        assert_eq!(cost("---@type number\nlocal value=1\n", 8), 9);
        assert_eq!(cost("---@alias Values table<string,number[]>\n", 1), 5);
    }

    #[test]
    fn every_original_annotation_location_contributes() {
        for source in [
            "---@class Box<T : table>\n---@field [string] number\n\n",
            "---@class Box : Parent\n\n",
            "---@field Box.value number\n",
            "---@function identity(value: number): number\n",
            "---@overload fun(value:number):number\n---@param value number\n---@return number\nlocal function identity(value) return value end\n",
            "---@generic T : table\n---@param value T\n---@vararg number\n---@return T\nlocal function identity(value,...) return value end\n",
            "---@type fun(value:number):number\nlocal function identity(value) return value end\n",
            "local value={\n---@type number\nfield=1,\n}\n",
            "local value=--[[---@type number]] source\n",
            "local function result()\n---@type number\nreturn 1\nend\n",
        ] {
            assert!(cost(source, 1) > cost(source, 0), "{source}");
        }
    }

    #[test]
    fn metadata_associations_do_not_count_original_types_again() {
        let mut file = luanalysis_syntax::parse(
            "---@class Box\n---@field value number\n\n---@function Box.read(value:number):number\n---@overload fun(value:string):string\n",
        );
        assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
        let bound = crate::bind::bind(&file);
        let original = checking_cost(&file, &bound, 8);
        file.type_declarations
            .extend(file.type_declarations.clone());
        file.function_declarations
            .extend(file.function_declarations.clone());
        file.overload_annotations
            .extend(file.overload_annotations.clone());
        assert_eq!(checking_cost(&file, &bound, 8), original);
    }
}
