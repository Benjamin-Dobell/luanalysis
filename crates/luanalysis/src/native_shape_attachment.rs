//! Source-only attachment directives. Initializer/member typing and freshness
//! remain native queries; no pre-evaluated type is kept in this index.
use super::NativeRegistry;
use crate::Checker;
use crate::bind::BindingId;
use crate::native_named::{NamedDeclarationSource, RuntimeAttachmentKind};
use luanalysis_engine::{NativeTypeDeclarationKey, Type};
use luanalysis_syntax::{NamedRecordKind, NamedTypeDeclarationKind, Span};

/// The file's shape attachments by binding and by initializer, so that finding the shape a
/// local or a table constructor is attached to does not recompute every directive.
#[derive(Default)]
pub(super) struct ShapeAttachmentIndex {
    directives: Vec<ShapeAttachmentDirective>,
    by_binding: std::collections::HashMap<BindingId, usize>,
    by_initializer: std::collections::HashMap<(usize, usize), usize>,
}

#[derive(Clone)]
pub(super) struct ShapeAttachmentDirective {
    pub declaration: NativeTypeDeclarationKey,
    pub declaration_span: Span,
    pub binding: BindingId,
    pub initializer: Option<Span>,
    pub explicit: bool,
}

impl NativeRegistry {
    pub(super) fn shape_attachment_directive(
        &self,
        source: &NamedDeclarationSource,
    ) -> Result<Option<ShapeAttachmentDirective>, &'static str> {
        if !matches!(
            source.syntax.kind,
            NamedTypeDeclarationKind::Record {
                flavor: NamedRecordKind::Shape,
                ..
            }
        ) {
            return Ok(None);
        }
        let Some(span) = source.syntax.attachment else {
            return Ok(None);
        };
        let attachment = self
            .named
            .attachment(span)
            .expect("original runtime attachment");
        let record_owners = attachment
            .declarations
            .iter()
            .filter(|&&key| {
                self.named.declaration(key).is_some_and(|source| {
                    matches!(source.syntax.kind, NamedTypeDeclarationKind::Record { .. })
                })
            })
            .count();
        if record_owners != 1 || self.named.candidates(&source.syntax.name.text).len() != 1 {
            return Err("multiple named declaration owners");
        }
        let RuntimeAttachmentKind::Local { bindings } = &attachment.kind else {
            return Err("only a single local shape attachment is supported");
        };
        let [binding] = bindings.as_slice() else {
            return Err("multiple local attachment destinations");
        };
        if !attachment.has_explicit_type_annotation {
            if !source.syntax.generics.is_empty() {
                return Err("generic shape templates need an explicit instance @type");
            }
            let [value] = attachment.local_values.as_slice() else {
                return Err("shape attachment needs one literal initializer");
            };
            if !value.table_literal {
                return Err("implicit shape attachment needs a table literal initializer");
            }
        }
        Ok(Some(ShapeAttachmentDirective {
            declaration: NativeTypeDeclarationKey {
                snapshot: source.key.snapshot,
                index: source.key.index,
            },
            declaration_span: source.syntax.name.span,
            binding: *binding,
            initializer: attachment.local_values.first().map(|value| value.span),
            explicit: attachment.has_explicit_type_annotation,
        }))
    }
    fn shape_attachment_index(&self) -> &ShapeAttachmentIndex {
        self.shape_attachments.get_or_init(|| {
            let mut index = ShapeAttachmentIndex::default();
            for source in self.named.declarations() {
                let Ok(Some(directive)) = self.shape_attachment_directive(source) else { continue };
                let position = index.directives.len();
                index.by_binding.entry(directive.binding).or_insert(position);
                if let Some(initializer) = directive.initializer {
                    index.by_initializer.entry((initializer.start, initializer.end)).or_insert(position);
                }
                index.directives.push(directive);
            }
            index
        })
    }
    pub(super) fn shape_for_binding(&self, binding: BindingId) -> Option<ShapeAttachmentDirective> {
        let index = self.shape_attachment_index();
        Some(index.directives[*index.by_binding.get(&binding)?].clone())
    }
    pub(super) fn shape_for_initializer(&self, span: Span) -> Option<ShapeAttachmentDirective> {
        let index = self.shape_attachment_index();
        Some(index.directives[*index.by_initializer.get(&(span.start, span.end))?].clone())
    }
}

impl Checker<'_> {
    pub(crate) fn shape_binding_constraint(
        &mut self,
        binding: BindingId,
        explicit: Option<&Type>,
        annotation_span: Option<Span>,
    ) -> Option<Type> {
        let directive = self.native?.shape_for_binding(binding)?;
        if let Some(explicit) = explicit {
            if self.engine.named_object_declaration(explicit) == Some(directive.declaration) {
                return Some(explicit.clone());
            }
            self.error("LUA2031",directive.declaration_span,
                "The explicit @type must resolve to the attached shape; combined constraint ownership is not implemented for a different canonical type.");
            return Some(explicit.clone());
        }
        let target = self
            .engine
            .named_declared_type(directive.declaration)
            .ok()?;
        let target = match self.captured_function {
            Some(context) => self.engine.instantiate_in_capture(&context, &target),
            None => target,
        };
        if directive.explicit {
            self.error(
                "LUA2031",
                annotation_span.unwrap_or(directive.declaration_span),
                "The attached shape has no explicit type for this local destination.",
            );
        }
        Some(target)
    }
}
