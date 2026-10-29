//! `keyof T`, `T[K]` and `typeof value` in annotations.

use luanalysis_engine::{GlobalQueryError, Type};
use luanalysis_syntax::{Expr, ExprKind, Name, TypeExpr, TypeExprKind, TypeLiteral};

impl super::Checker<'_> {
    pub(super) fn indexed_access_annotation(&mut self, object: &TypeExpr, index: &TypeExpr) -> Type {
        let object_type = self.annotation(object);
        let index_type = self.annotation(index);
        if let Some(value) = self.engine.indexed_access_type(&object_type, &index_type) {
            return value;
        }
        // As TypeScript reports it: a missing name as a missing property, another key as an
        // invalid index.
        if let TypeExprKind::Literal(TypeLiteral::String(name)) = &index.value_syntax().kind {
            self.error_types(
                "TS2339", index.span,
                format!("Property '{}' does not exist on type '{{}}'.", String::from_utf8_lossy(name)),
                &[&object_type],
            );
        } else {
            self.error_types("TS2536", index.span, "Type '{}' cannot be used to index type '{}'.", &[&index_type, &object_type]);
        }
        self.engine.error_type()
    }

    /// `check extends extends and yes or no`, whose extends type and true branch see the `infer`
    /// type parameters that the extends type declares. As TypeScript does, the branches are
    /// lowered when first read, so that one may refer to the alias whose body this is.
    pub(super) fn conditional_annotation(&mut self, ty: &TypeExpr) -> Type {
        let TypeExprKind::Conditional { check, extends, yes, no } = &ty.kind else {
            unreachable!("a conditional type annotation")
        };
        let key = self.native.and_then(|native| native.types.annotation_at(ty.span));
        let infer: Vec<_> = key.zip(self.native).map(|(key, native)| native.types.declared_annotation_parameters(key)).unwrap_or_default()
            .into_iter()
            .map(|parameter| {
                let constraint = self.native.and_then(|native| native.types.parameter(parameter).constraint.as_ref()).map(|constraint| self.annotation(constraint));
                (parameter, constraint)
            })
            .collect();
        let [check_type, extends_type] = [check, extends].map(|ty| self.annotation(ty));
        let (Some(key), Some(native)) = (key, self.native) else {
            let branches = luanalysis_engine::ConditionalBranches::Lowered(self.annotation(yes), self.annotation(no));
            return self.engine.conditional_type(&check_type, &extends_type, &infer, &branches, &[]);
        };
        let context = crate::native_types::AnnotationContext {
            function: self.type_context,
            named: self.named_type_context,
            signature: self.signature_type_context,
        };
        let source = native.types.annotation(key).expect("registered conditional type");
        source.context.borrow_mut().get_or_insert(context);
        let lexical = self.signature_type_context.map(crate::native_named::NativeTypeContext::Signature)
            .or_else(|| self.named_type_context.map(crate::native_named::NativeTypeContext::NamedType))
            .or_else(|| self.type_context.map(crate::native_named::NativeTypeContext::Function));
        let mut outer = native.annotation_piece_outer_parameters(lexical, yes);
        for parameter in native.annotation_piece_outer_parameters(lexical, no) {
            if !outer.contains(&parameter) { outer.push(parameter); }
        }
        let branches = luanalysis_engine::ConditionalBranches::Written(key, context.encode());
        self.engine.conditional_type(&check_type, &extends_type, &infer, &branches, &outer)
    }

    /// TypeScript's `getConditionalFlowTypeOfType`: within the true branch of `T extends U and X
    /// or Y`, `T` is known to be a `U` (each enclosing conditional of `T` adds its extends type),
    /// so `T extends string and OnlyStr<T> or never` satisfies `OnlyStr`'s constraint.
    pub(super) fn conditional_flow_type(&mut self, ty: &TypeExpr, resolved: Type) -> Type {
        let (Type::NativeTypeParameter(parameter), TypeExprKind::Named(name), Some(native)) = (&resolved, &ty.kind, self.native) else {
            return resolved;
        };
        let parameter = *parameter;
        let mut constraints = Vec::new();
        for source in native.types.annotations() {
            let luanalysis_syntax::AnnotationDeclaration::Conditional(conditional) = &source.syntax else { continue };
            if !(conditional.yes.span.start <= ty.span.start && ty.span.end <= conditional.yes.span.end) { continue }
            let mut check = &conditional.check_type;
            while let TypeExprKind::Parenthesized(inner) = &check.kind { check = inner }
            if !matches!(&check.kind, TypeExprKind::Named(checked) if checked == name) { continue }
            // The written types are lowered again only to compare and constrain: what they report
            // was reported where the conditional type is.
            let mut scratch = native.checker(&mut *self.engine);
            scratch.type_context = self.type_context;
            scratch.named_type_context = self.named_type_context;
            scratch.signature_type_context = self.signature_type_context;
            if scratch.annotation(check) == Type::NativeTypeParameter(parameter) {
                constraints.push(scratch.annotation(&conditional.extends_type));
            }
        }
        match constraints.len() {
            0 => resolved,
            1 => self.engine.substitution_type(&resolved, &constraints[0]),
            _ => {
                let constraint = Type::Intersection(constraints);
                self.engine.substitution_type(&resolved, &constraint)
            }
        }
    }

    /// TypeScript's mapped type, `{ [P in K as N]?: T }`. Its template and `as` clause see `P`
    /// and are lowered when first read, as TypeScript reads them, so that one may refer to the
    /// alias whose body this is. Written `keyof X`, its keys make it homomorphic in `X`.
    pub(super) fn mapped_annotation(&mut self, mapped: &luanalysis_syntax::MappedType, span: luanalysis_syntax::Span) -> Type {
        use luanalysis_engine::MappedTypeModifier;
        let modifier = |modifier: luanalysis_syntax::MappedModifier| match modifier {
            luanalysis_syntax::MappedModifier::None => MappedTypeModifier::None,
            luanalysis_syntax::MappedModifier::Add => MappedTypeModifier::Add,
            luanalysis_syntax::MappedModifier::Plus => MappedTypeModifier::Plus,
            luanalysis_syntax::MappedModifier::Remove => MappedTypeModifier::Remove,
        };
        let key = self.native.and_then(|native| native.types.annotation_at(span));
        let declared = key.zip(self.native).and_then(|(key, native)| native.types.declared_annotation_parameters(key).first().copied());
        let (Some(key), Some(parameter), Some(native)) = (key, declared, self.native) else {
            unreachable!("every documentation comment's mapped type is registered")
        };
        let (constraint, keyof) = match &mapped.constraint.value_syntax().kind {
            TypeExprKind::KeyOf(of) => {
                let of = self.annotation(of);
                (self.engine.key_type(&of), Some(of))
            }
            _ => (self.annotation(&mapped.constraint), None),
        };
        let context = crate::native_types::AnnotationContext {
            function: self.type_context,
            named: self.named_type_context,
            signature: self.signature_type_context,
        };
        let source = native.types.annotation(key).expect("registered mapped type");
        source.context.borrow_mut().get_or_insert(context);
        let lexical = self.signature_type_context.map(crate::native_named::NativeTypeContext::Signature)
            .or_else(|| self.named_type_context.map(crate::native_named::NativeTypeContext::NamedType))
            .or_else(|| self.type_context.map(crate::native_named::NativeTypeContext::Function));
        let mut outer = native.annotation_piece_outer_parameters(lexical, &mapped.ty);
        if let Some(name) = &mapped.name {
            for parameter in native.annotation_piece_outer_parameters(lexical, name) {
                if !outer.contains(&parameter) { outer.push(parameter); }
            }
        }
        outer.retain(|&outer| outer != parameter);
        self.engine.mapped_type(&luanalysis_engine::MappedTypeRecipe {
            parameter,
            constraint,
            keyof,
            readonly: modifier(mapped.readonly),
            optional: modifier(mapped.optional),
            pieces: luanalysis_engine::MappedTypePieces::Written { key, context: context.encode(), has_name: mapped.name.is_some() },
            outer,
        })
    }

    /// The type parameter that `infer name` declares, which only a conditional type's extends
    /// type can.
    pub(super) fn infer_annotation(&mut self, name: &Name, span: luanalysis_syntax::Span) -> Type {
        if let Some(key) = self.native.and_then(|native| native.types.declared_parameter(name.span)) {
            return Type::NativeTypeParameter(key);
        }
        self.error("TS1338", span, "'infer' declarations are only permitted in the 'extends' clause of a conditional type.");
        self.engine.error_type()
    }

    /// The declared type of the value `path` names where it is written: the local in scope
    /// there or else the global, then each member of it. As in TypeScript, this is the
    /// declared type, not one narrowed where the annotation is.
    pub(super) fn type_query_annotation(&mut self, path: &[Name]) -> Type {
        let root = &path[0];
        let Some(mut ty) = self.declared_value_type(root) else {
            self.error("TS2304", root.span, format!("Cannot find name '{}'.", root.text));
            return self.engine.error_type();
        };
        // Editors show each name's type where it is written, as at a read.
        self.record_expression_type(root.span, &ty);
        let mut receiver_span = root.span;
        for member in &path[1..] {
            ty = self.native_named_property_read(&ty, member, receiver_span);
            self.record_expression_type(member.span, &ty);
            receiver_span = receiver_span.join(member.span);
        }
        ty
    }

    /// The declared type of a name or a member path of one (`M.f`), as `typeof` gives it,
    /// without reporting anything. None: another expression, or a name that is not declared.
    pub(super) fn declared_expression_type(&mut self, expression: &Expr) -> Option<Type> {
        match &expression.kind {
            ExprKind::Name(name) => self.declared_value_type(name),
            ExprKind::Field { base, name } => {
                let receiver = self.declared_expression_type(base)?;
                self.engine.property_type(&receiver, &name.text)
            }
            ExprKind::Parenthesized(inner) => self.declared_expression_type(inner),
            _ => None,
        }
    }

    fn declared_value_type(&mut self, name: &Name) -> Option<Type> {
        // The binder resolves the name as editors do. An `---@enum`'s names the variable
        // where its statement writes it, which that occurrence resolves.
        let binding = match self.bound.occurrence_at(name.span.start) {
            Some(occurrence) if occurrence.span == name.span => occurrence.binding,
            _ => self.bound.visible_binding(&name.text, name.span.start),
        };
        if let Some(binding) = binding {
            let declared = self.resolve_local(binding).map(|binding| binding.constraint.clone())?;
            // A local declared without a value or type has a type only where it is read.
            if self.engine.is_auto_variable_constraint(&declared) {
                return Some(Type::Any);
            }
            // A table has every member its stores give it, wherever they are.
            return Some(self.engine.runtime_table_schema(&declared).unwrap_or(declared));
        }
        let native = self.native?;
        if let Some(binding) = native.global_binding(&name.text) {
            let declared = match self.engine.global_binding_type(binding) {
                Ok(declared) => declared,
                // `---@type typeof G` on `G` itself.
                Err(GlobalQueryError::CircularQuery(_) | GlobalQueryError::TooDeep(_)) => {
                    self.error("TS2502", name.span, format!("'{}' is referenced directly or indirectly in its own type annotation.", name.text));
                    return Some(self.engine.error_type());
                }
                // Other failures are reported at the global's declarations.
                Err(_) => return Some(self.engine.error_type()),
            };
            return Some(self.engine.runtime_table_schema(&declared).unwrap_or(declared));
        }
        native.legacy_builtin_value(name)
    }
}
