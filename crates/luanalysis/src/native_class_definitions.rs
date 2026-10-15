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
        }
    }
}
pub(super) struct ClassDefinitionDirective {
    pub key: NativeClassDefinitionKey,
    pub declaration: NativeTypeDeclarationKey,
    pub binding: BindingId,
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
    pub(super) fn class_definition_directive(
        &self,
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
            return Err(
                "declaration placeholders require an external value contract, not checked runtime initialization",
            );
        }
        if !source.syntax.generics.is_empty() {
            return Err("generic runtime class templates are not implemented");
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
            || self.named.candidates(&source.syntax.name.text).len() != 1
        {
            return Err("multiple named declaration owners");
        }
        if attachment.has_explicit_type_annotation {
            return Err(
                "combined class definition and explicit @type ownership is not implemented",
            );
        }
        let RuntimeAttachmentKind::Local { bindings } = &attachment.kind else {
            return Err("only a single local checked class definition is implemented");
        };
        let [binding] = bindings.as_slice() else {
            return Err("multiple local definition destinations");
        };
        let [value] = attachment.local_values.as_slice() else {
            return Err("class definition needs one literal initializer");
        };
        if !value.table_literal {
            return Err("class definition needs a table literal initializer");
        }
        let index = self
            .class_definitions
            .sources
            .iter()
            .position(|key| *key == source.key)
            .expect("original definition allocated before demands");
        Ok(Some(ClassDefinitionDirective {
            key: NativeClassDefinitionKey {
                snapshot: self.snapshot,
                index: index.try_into().expect("source definition index fits u32"),
            },
            declaration: NativeTypeDeclarationKey {
                snapshot: source.key.snapshot,
                index: source.key.index,
            },
            binding: *binding,
            initializer: value.span,
            declaration_span: source.syntax.name.span,
        }))
    }
    fn class_definition_for_binding(&self, binding: BindingId) -> Option<ClassDefinitionDirective> {
        self.class_definitions.sources.iter().find_map(|&key| {
            self.class_definition_directive(self.named.declaration(key)?)
                .ok()
                .flatten()
                .filter(|directive| directive.binding == binding)
        })
    }
    fn class_definition_for_initializer(&self, span: Span) -> Option<ClassDefinitionDirective> {
        self.class_definitions.sources.iter().find_map(|&key| {
            self.class_definition_directive(self.named.declaration(key)?)
                .ok()
                .flatten()
                .filter(|directive| directive.initializer == span)
        })
    }
}
impl Checker<'_> {
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
        match self
            .engine
            .class_definition_value(directive.key, &class, &actual)
        {
            Ok(value) => value,
            Err(error) => {
                self.class_definition_error(error, span);
                actual
            }
        }
    }
    fn class_definition_error(&mut self, error: NamedTypeError, span: Span) {
        match error {
            NamedTypeError::DefinitionField {
                error: luanalysis_engine::TableError::ValueType { value, expected },
                ..
            } => self.error(
                "TS2322",
                span,
                format!(
                    "Type '{}' is not assignable to class definition field type '{}'.",
                    crate::describe(&value),
                    crate::describe(&expected)
                ),
            ),
            NamedTypeError::DefinitionField { error, .. } => self.error(
                "TS2322",
                span,
                format!("Class definition field is incompatible: {error:?}"),
            ),
            NamedTypeError::DefinitionInitializer { actual, .. } => self.error(
                "TS2322",
                span,
                format!(
                    "Class definition requires a table, found '{}'.",
                    crate::describe(&actual)
                ),
            ),
            other => self.error(
                "LUA2031",
                span,
                format!("Class definition is unsupported: {other:?}"),
            ),
        }
    }
}
