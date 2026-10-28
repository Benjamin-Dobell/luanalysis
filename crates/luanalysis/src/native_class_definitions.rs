//! Original checked class declaration transactions. The native core owns the
//! distinction between partial definition values and complete instance types.
use super::NativeRegistry;
use crate::native_named::{
    NamedDeclarationSource, NativeNamed, NativeNamedKey, RuntimeAttachmentKind,
};
use crate::{Checker, bind::BindingId};
use luanalysis_engine::{NamedTypeError, NativeClassDefinitionKey, NativeTypeDeclarationKey, Type};
use luanalysis_project::SourceRole;
use luanalysis_syntax::{NamedRecordKind, NamedTypeDeclarationKind, Span};

pub(super) struct NativeClassDefinitions {
    snapshot: u64,
    sources: Vec<NativeNamedKey>,
    /// Each source's directive, and the sources by initializer and by binding, so that
    /// finding the definition an expression initializes does not recompute every directive.
    index: std::cell::OnceCell<ClassDefinitionIndex>,
}
#[derive(Default)]
struct ClassDefinitionIndex {
    directives: Vec<Result<Option<ClassDefinitionDirective>, &'static str>>,
    by_source: std::collections::HashMap<NativeNamedKey, usize>,
    by_initializer: std::collections::HashMap<(usize, usize), usize>,
    by_binding: std::collections::HashMap<BindingId, usize>,
}
impl NativeClassDefinitions {
    pub(super) fn new(snapshot: u64, named: &NativeNamed) -> Self {
        Self {
            snapshot,
            sources: named
                .declarations()
                .iter()
                .filter(|source| {
                    matches!(
                        source.syntax.kind,
                        NamedTypeDeclarationKind::Record {
                            flavor: NamedRecordKind::Class,
                            ..
                        }
                    ) && source.syntax.attachment.is_some()
                })
                .map(|source| source.key)
                .collect(),
            index: Default::default(),
        }
    }
    /// Directives depend on the source role and the project's merged names, which are set up
    /// before checking starts.
    pub(super) fn invalidate(&mut self) {
        self.index.take();
    }
}
#[derive(Clone)]
pub(crate) struct ClassDefinitionDirective {
    pub key: NativeClassDefinitionKey,
    pub declaration: NativeTypeDeclarationKey,
    /// The local the class table is stored in; None for a global or a field (`X = {}`,
    /// `M.X = {}`), whose class owner is its spelling.
    pub binding: Option<BindingId>,
    pub initializer: Span,
    pub declaration_span: Span,
}
impl NativeRegistry {
    pub(super) fn class_definition_source(
        &self,
        key: NativeClassDefinitionKey,
    ) -> Option<&NamedDeclarationSource> {
        if key.snapshot != self.class_definitions.snapshot {
            return None;
        }
        self.named
            .declaration(*self.class_definitions.sources.get(key.index as usize)?)
    }
    fn class_definition_index(&self) -> &ClassDefinitionIndex {
        self.class_definitions.index.get_or_init(|| {
            let mut index = ClassDefinitionIndex::default();
            for (position, &key) in self.class_definitions.sources.iter().enumerate() {
                let directive = match self.named.declaration(key) {
                    Some(source) => self.compute_class_definition_directive(position, source),
                    None => Ok(None),
                };
                index.by_source.insert(key, position);
                if let Ok(Some(directive)) = &directive {
                    index.by_initializer.entry((directive.initializer.start, directive.initializer.end)).or_insert(position);
                    if let Some(binding) = directive.binding {
                        index.by_binding.entry(binding).or_insert(position);
                    }
                }
                index.directives.push(directive);
            }
            index
        })
    }
    pub(crate) fn class_definition_directive(
        &self,
        source: &NamedDeclarationSource,
    ) -> Result<Option<ClassDefinitionDirective>, &'static str> {
        let index = self.class_definition_index();
        match index.by_source.get(&source.key) {
            Some(&position) => index.directives[position].clone(),
            // Not a class with an attachment.
            None => Ok(None),
        }
    }
    fn compute_class_definition_directive(
        &self,
        index: usize,
        source: &NamedDeclarationSource,
    ) -> Result<Option<ClassDefinitionDirective>, &'static str> {
        if !matches!(
            source.syntax.kind,
            NamedTypeDeclarationKind::Record {
                flavor: NamedRecordKind::Class,
                ..
            }
        ) {
            return Ok(None);
        }
        let Some(span) = source.syntax.attachment else {
            return Ok(None);
        };
        if self.source_role() != SourceRole::Implementation {
            if self.is_ambient_local_attachment(span) || self.is_ambient_global_attachment(span) { return Ok(None); }
            return Err(
                "declaration placeholders require an external value contract, not checked runtime initialization",
            );
        }
        let attachment = self
            .named
            .attachment(span)
            .expect("original class attachment");
        if attachment
            .declarations
            .iter()
            .filter(|&&key| {
                self.named.declaration(key).is_some_and(|source| {
                    matches!(source.syntax.kind, NamedTypeDeclarationKind::Record { .. })
                })
            })
            .count()
            != 1
            || self.named.sole_owner(&source.syntax.name.text).is_none()
        {
            return Err("multiple named declaration owners");
        }
        // A bare local declares an annotated value, as `---@type C; local c`
        // does. It does not initialize a runtime class table. In particular a
        // generic class declaration here needs no runtime template machinery.
        if matches!(&attachment.kind, RuntimeAttachmentKind::Local { bindings } if bindings.len() == 1)
            && attachment.local_values.is_empty()
            && !attachment.has_explicit_type_annotation
        {
            return Ok(None);
        }
        // Generic definitions use the declared head's own/outer identity
        // mapper. The checked definition transaction validates supplied fields
        // against those rigid parameters; native instantiation maps both the
        // class reference and its partial value view at each actual use.
        if attachment.has_explicit_type_annotation {
            return Err(
                "combined class definition and explicit @type ownership is not implemented",
            );
        }
        let binding = match &attachment.kind {
            RuntimeAttachmentKind::Local { bindings } => {
                let [binding] = bindings.as_slice() else {
                    return Err("multiple local definition destinations");
                };
                Some(*binding)
            }
            // `---@class X` above `X = {}` or `M.X = {}`, as v1 and LuaLS read it.
            RuntimeAttachmentKind::Assignment { targets } => {
                let [target] = targets.as_slice() else {
                    return Err("multiple definition destinations");
                };
                if super::methods::class_path(target, &self.bound).is_none() {
                    return Err("a class table is stored in a local, a global or a field");
                }
                None
            }
            _ => return Err("a class table is stored in a local, a global or a field"),
        };
        let [value] = attachment.local_values.as_slice() else {
            return Err("class definition needs one initializer");
        };
        Ok(Some(ClassDefinitionDirective {
            key: NativeClassDefinitionKey {
                snapshot: self.snapshot,
                index: index.try_into().expect("source definition index fits u32"),
            },
            // A repeated `---@class X` defines the class it is merged into.
            declaration: self.canonical_declaration(NativeTypeDeclarationKey {
                snapshot: source.key.snapshot,
                index: source.key.index,
            }),
            binding,
            initializer: value.span,
            declaration_span: source.syntax.name.span,
        }))
    }
    /// The class whose definition table `key` names, when it is a class definition's local.
    pub(crate) fn own_definition_value(&self, key: luanalysis_engine::NativeExpressionKey, queries: &mut dyn luanalysis_engine::TypeQueries) -> Option<Type> {
        let mut expression: &luanalysis_syntax::Expr = &self.expressions().get(key).expression;
        while let luanalysis_syntax::ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }
        let luanalysis_syntax::ExprKind::Name(name) = &expression.kind else { return None };
        let directive = self.class_definition_for_binding(self.bound.binding_at(name.span.start)?)?;
        queries.named_declared_type(directive.declaration).ok()
    }
    fn class_definition_for_binding(&self, binding: BindingId) -> Option<ClassDefinitionDirective> {
        let index = self.class_definition_index();
        index.directives[*index.by_binding.get(&binding)?].clone().ok().flatten()
    }
    /// Whether the expression at `span` initializes a class definition.
    pub(crate) fn is_class_definition_initializer(&self, span: Span) -> bool {
        self.class_definition_for_initializer(span).is_some()
    }
    fn class_definition_for_initializer(&self, span: Span) -> Option<ClassDefinitionDirective> {
        let index = self.class_definition_index();
        index.directives[*index.by_initializer.get(&(span.start, span.end))?].clone().ok().flatten()
    }
}
impl Checker<'_> {
    pub(crate) fn finish_class_definition_pack(
        &mut self,
        span: Span,
        pack: luanalysis_engine::ValuePack,
    ) -> luanalysis_engine::ValuePack {
        if self.native.and_then(|native| native.class_definition_for_initializer(span)).is_none() {
            return pack;
        }
        // A checked class attachment has exactly one original destination.
        // Lua discards extra call results here; validate its actual first value.
        let actual = self.engine.pack_slot(&pack, 0);
        let value = self.finish_class_definition_initializer(span, actual);
        self.record_expression_type(span, &value);
        luanalysis_engine::ValuePack::single(value)
    }

    fn class_definition_instance(&mut self, directive: &ClassDefinitionDirective) -> Option<Type> {
        let class = match self.engine.named_declared_type(directive.declaration) {
            Ok(class) => class,
            Err(error) => {
                self.class_definition_error(error, directive.declaration_span);
                return None;
            }
        };
        Some(match self.captured_function {
            Some(context) => self.engine.instantiate_in_capture(&context, &class),
            None => class,
        })
    }
    pub(crate) fn class_definition_initializer_context(
        &mut self,
        binding: BindingId,
    ) -> Option<Type> {
        let directive = self.native?.class_definition_for_binding(binding)?;
        let class = self.class_definition_instance(&directive)?;
        match self.engine.class_definition_context(directive.key, &class) {
            Ok(context) => Some(context),
            Err(error) => {
                self.class_definition_error(error, directive.declaration_span);
                None
            }
        }
    }
    pub(crate) fn class_namespace_member_contract(&mut self, binding: BindingId, actual: &Type) -> Option<Type> {
        let directive = self.native?.class_definition_for_binding(binding)?;
        self.engine.class_definition_member_contract(directive.key, actual)
    }
    pub(crate) fn finish_class_definition_initializer(&mut self, span: Span, actual: Type) -> Type {
        let Some(directive) = self
            .native
            .and_then(|native| native.class_definition_for_initializer(span))
        else {
            return actual;
        };
        let Some(class) = self.class_definition_instance(&directive) else {
            return actual;
        };
        // A class declared in several places (LuaLS) is reopened on its module's value. A
        // declaration file's definition, such as the standard library's `string` table, is the
        // one that others extend, and keeps its own value.
        let reopened = self.native.is_some_and(|native| native.is_merged_declaration(directive.declaration)
            && native.source_role() != luanalysis_project::SourceRole::Declaration);
        let value = if reopened {
            self.engine.class_definition_reopen(directive.key, &class, &actual)
        } else {
            self.engine.class_definition_value(directive.key, &class, &actual)
        };
        match value
        {
            Ok(value) => value,
            // A field that fails its contract is reported once; the table is still the class's
            // definition, as a TypeScript class with an incompatible member is still the class.
            Err(error @ NamedTypeError::DefinitionField { .. }) if !reopened => {
                self.class_definition_error(error, span);
                self.engine.class_definition_value_recovered(directive.key, &class, &actual).unwrap_or(actual)
            }
            Err(error) => {
                self.class_definition_error(error, span);
                actual
            }
        }
    }
    fn class_definition_error(&mut self, error: NamedTypeError, span: Span) {
        match error {
            // Native declaration queries report these at the original member
            // or base. Repeated initializer demands must not add wrapper errors
            // at the class name and constructor on top of that diagnostic.
            NamedTypeError::IncompatibleOverride { .. }
            | NamedTypeError::IncompatibleBase { .. }
            | NamedTypeError::InvalidBase { .. }
            | NamedTypeError::CircularBase(_)
            | NamedTypeError::TypeArgumentConstraint { .. } => {},
            NamedTypeError::DefinitionField {
                error: luanalysis_engine::TableError::ValueType { value, expected },
                ..
            } => self.error_types(
                "TS2322",
                span,
                "Type '{}' is not assignable to class definition field type '{}'.",
                &[&value, &expected],
            ),
            NamedTypeError::DefinitionField { error, .. } => {
                let (template, types) = crate::native_tables::table_error_description(&error);
                self.error_types("TS2322", span, format!("Class definition field is incompatible: {template}"), &types.iter().collect::<Vec<_>>());
            }
            NamedTypeError::DefinitionInitializer { actual, .. } => self.error_types(
                "TS2322",
                span,
                "Class definition requires a table, found '{}'.",
                &[&actual],
            ),
            NamedTypeError::InvalidReceiver { actual, expected, .. } => self.error_types(
                "LUA2031", span, "Class definition is unsupported: receiver type '{}' does not satisfy '{}'.", &[&actual, &expected],
            ),
            NamedTypeError::UnsupportedReceiverType { actual } => self.error_types(
                "LUA2031", span, "Class definition is unsupported for receiver type '{}'.", &[&actual],
            ),
            NamedTypeError::DefinitionOwner { actual, .. } => self.error_types(
                "LUA2031", span, "Class definition owner is incompatible with type '{}'.", &[&actual],
            ),
            NamedTypeError::Table { error, .. } => {
                let (template, types) = crate::native_tables::table_error_description(&error);
                self.error_types("LUA2031", span, format!("Class definition is unsupported: {template}"), &types.iter().collect::<Vec<_>>());
            }
            other => {
                let (template, types) = crate::diagnostic_types::named_error_description(&other);
                self.error_types(
                    "LUA2031",
                    span,
                    format!("Class definition is unsupported: {template}"),
                    &types.iter().collect::<Vec<_>>(),
                );
            }
        }
    }
}
