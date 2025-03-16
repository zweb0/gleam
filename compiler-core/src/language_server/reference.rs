use std::collections::HashMap;

use ecow::EcoString;
use lsp_types::Location;

use crate::{
    analyse,
    ast::{
        self, ArgNames, CustomType, Definition, Function, ModuleConstant, Pattern,
        RecordConstructor, SrcSpan, TypedExpr, TypedModule, visit::Visit,
    },
    build::Located,
    type_::{
        ModuleInterface, ModuleValueConstructor, Type, ValueConstructor, ValueConstructorVariant,
        error::{Named, VariableOrigin},
    },
};

use super::{
    compiler::ModuleSourceInformation, rename::RenameTarget, src_span_to_lsp_range, url_from_path,
};

pub enum Referenced {
    LocalVariable {
        definition_location: SrcSpan,
        location: SrcSpan,
        origin: Option<VariableOrigin>,
    },
    ModuleValue {
        module: EcoString,
        name: EcoString,
        location: SrcSpan,
        name_kind: Named,
        target_kind: RenameTarget,
    },
    ModuleType {
        module: EcoString,
        name: EcoString,
        location: SrcSpan,
        target_kind: RenameTarget,
    },
}

pub fn reference_for_ast_node(
    found: Located<'_>,
    current_module: &EcoString,
) -> Option<Referenced> {
    match found {
        Located::Expression(TypedExpr::Var {
            constructor:
                ValueConstructor {
                    variant:
                        ValueConstructorVariant::LocalVariable {
                            location: definition_location,
                            origin,
                        },
                    ..
                },
            location,
            ..
        }) => Some(Referenced::LocalVariable {
            definition_location: *definition_location,
            location: *location,
            origin: Some(origin.clone()),
        }),
        Located::Pattern(Pattern::Variable {
            location, origin, ..
        }) => Some(Referenced::LocalVariable {
            definition_location: *location,
            location: *location,
            origin: Some(origin.clone()),
        }),
        Located::Pattern(Pattern::VarUsage {
            constructor,
            location,
            ..
        }) => constructor
            .as_ref()
            .and_then(|constructor| match &constructor.variant {
                ValueConstructorVariant::LocalVariable {
                    location: definition_location,
                    origin,
                } => Some(Referenced::LocalVariable {
                    definition_location: *definition_location,
                    location: *location,
                    origin: Some(origin.clone()),
                }),
                _ => None,
            }),
        Located::Pattern(Pattern::Assign { location, .. }) => Some(Referenced::LocalVariable {
            definition_location: *location,
            location: *location,
            origin: None,
        }),
        Located::Arg(arg) => match &arg.names {
            ArgNames::Named { location, .. }
            | ArgNames::NamedLabelled {
                name_location: location,
                ..
            } => Some(Referenced::LocalVariable {
                definition_location: *location,
                location: *location,
                origin: None,
            }),
            ArgNames::Discard { .. } | ArgNames::LabelledDiscard { .. } => None,
        },
        Located::Expression(TypedExpr::Var {
            constructor:
                ValueConstructor {
                    variant:
                        ValueConstructorVariant::ModuleConstant { module, .. }
                        | ValueConstructorVariant::ModuleFn { module, .. },
                    ..
                },
            name,
            location,
            ..
        }) => Some(Referenced::ModuleValue {
            module: module.clone(),
            name: name.clone(),
            location: *location,
            name_kind: Named::Function,
            target_kind: RenameTarget::Unqualified,
        }),

        Located::Expression(TypedExpr::ModuleSelect {
            module_name,
            label,
            constructor: ModuleValueConstructor::Fn { .. } | ModuleValueConstructor::Constant { .. },
            location,
            field_start,
            ..
        }) => Some(Referenced::ModuleValue {
            module: module_name.clone(),
            name: label.clone(),

            location: SrcSpan::new(*field_start, location.end),
            name_kind: Named::Function,
            target_kind: RenameTarget::Qualified,
        }),
        Located::ModuleStatement(
            Definition::Function(Function {
                name: Some((location, name)),
                ..
            })
            | Definition::ModuleConstant(ModuleConstant {
                name,
                name_location: location,
                ..
            }),
        ) => Some(Referenced::ModuleValue {
            module: current_module.clone(),
            name: name.clone(),
            location: *location,
            name_kind: Named::Function,
            target_kind: RenameTarget::Definition,
        }),
        Located::Expression(TypedExpr::Var {
            constructor:
                ValueConstructor {
                    variant: ValueConstructorVariant::Record { module, name, .. },
                    ..
                },
            location,
            ..
        }) => Some(Referenced::ModuleValue {
            module: module.clone(),
            name: name.clone(),
            location: *location,
            name_kind: Named::CustomTypeVariant,
            target_kind: RenameTarget::Unqualified,
        }),
        Located::Expression(TypedExpr::ModuleSelect {
            module_name,
            label,
            constructor: ModuleValueConstructor::Record { .. },
            location,
            field_start,
            ..
        }) => Some(Referenced::ModuleValue {
            module: module_name.clone(),
            name: label.clone(),
            location: SrcSpan::new(*field_start, location.end),
            name_kind: Named::CustomTypeVariant,
            target_kind: RenameTarget::Qualified,
        }),
        Located::VariantConstructorDefinition(RecordConstructor {
            name,
            name_location,
            ..
        }) => Some(Referenced::ModuleValue {
            module: current_module.clone(),
            name: name.clone(),
            location: *name_location,
            name_kind: Named::CustomTypeVariant,
            target_kind: RenameTarget::Definition,
        }),
        Located::Pattern(Pattern::Constructor {
            constructor: analyse::Inferred::Known(constructor),
            module: module_select,
            name_location: location,
            ..
        }) => Some(Referenced::ModuleValue {
            module: constructor.module.clone(),
            name: constructor.name.clone(),
            location: *location,
            name_kind: Named::CustomTypeVariant,
            target_kind: if module_select.is_some() {
                RenameTarget::Qualified
            } else {
                RenameTarget::Unqualified
            },
        }),
        Located::Annotation { ast, type_ } => match type_.as_ref() {
            Type::Named { module, name, .. } => {
                let target_kind = match ast {
                    ast::TypeAst::Constructor(constructor) => {
                        if constructor.module.is_some() {
                            RenameTarget::Qualified
                        } else {
                            RenameTarget::Unqualified
                        }
                    }
                    ast::TypeAst::Fn(_)
                    | ast::TypeAst::Var(_)
                    | ast::TypeAst::Tuple(_)
                    | ast::TypeAst::Hole(_) => RenameTarget::Unqualified,
                };
                Some(Referenced::ModuleType {
                    module: module.clone(),
                    name: name.clone(),
                    location: ast.location(),
                    target_kind,
                })
            }
            Type::Fn { .. } | Type::Var { .. } | Type::Tuple { .. } => None,
        },
        Located::ModuleStatement(Definition::CustomType(CustomType {
            name,
            name_location,
            ..
        })) => Some(Referenced::ModuleType {
            module: current_module.clone(),
            name: name.clone(),
            location: *name_location,
            target_kind: RenameTarget::Definition,
        }),
        _ => None,
    }
}

pub fn find_module_references(
    module_name: EcoString,
    name: EcoString,
    modules: &im::HashMap<EcoString, ModuleInterface>,
    sources: &HashMap<EcoString, ModuleSourceInformation>,
    layer: ast::Layer,
) -> Vec<Location> {
    let mut reference_locations = Vec::new();

    for module in modules.values() {
        if module.name == module_name || module.references.imported_modules.contains(&module_name) {
            let Some(source_information) = sources.get(&module.name) else {
                continue;
            };

            find_references_in_module(
                &module_name,
                &name,
                module,
                source_information,
                &mut reference_locations,
                layer,
            );
        }
    }

    reference_locations
}

fn find_references_in_module(
    module_name: &EcoString,
    name: &EcoString,
    module: &ModuleInterface,
    source_information: &ModuleSourceInformation,
    reference_locations: &mut Vec<Location>,
    layer: ast::Layer,
) {
    let reference_map = match layer {
        ast::Layer::Value => &module.references.value_references,
        ast::Layer::Type => &module.references.type_references,
    };

    let Some(references) = reference_map.get(&(module_name.clone(), name.clone())) else {
        return;
    };

    let Some(uri) = url_from_path(source_information.path.as_str()) else {
        return;
    };

    for reference in references {
        reference_locations.push(Location {
            uri: uri.clone(),
            range: src_span_to_lsp_range(reference.location, &source_information.line_numbers),
        });
    }
}

pub fn find_variable_references(
    module: &TypedModule,
    definition_location: SrcSpan,
) -> Vec<SrcSpan> {
    let mut finder = FindVariableReferences {
        references: Vec::new(),
        definition_location,
    };
    finder.visit_typed_module(module);
    finder.references
}

struct FindVariableReferences {
    references: Vec<SrcSpan>,
    definition_location: SrcSpan,
}

impl<'ast> Visit<'ast> for FindVariableReferences {
    fn visit_typed_function(&mut self, fun: &'ast ast::TypedFunction) {
        if fun.full_location().contains(self.definition_location.start) {
            ast::visit::visit_typed_function(self, fun);
        }
    }

    fn visit_typed_expr_var(
        &mut self,
        location: &'ast SrcSpan,
        constructor: &'ast ValueConstructor,
        _name: &'ast EcoString,
    ) {
        match constructor.variant {
            ValueConstructorVariant::LocalVariable {
                location: definition_location,
                ..
            } if definition_location == self.definition_location => self.references.push(*location),
            _ => {}
        }
    }

    fn visit_typed_clause_guard_var(
        &mut self,
        location: &'ast SrcSpan,
        _name: &'ast EcoString,
        _type_: &'ast std::sync::Arc<Type>,
        definition_location: &'ast SrcSpan,
    ) {
        if *definition_location == self.definition_location {
            self.references.push(*location)
        }
    }

    fn visit_typed_pattern_var_usage(
        &mut self,
        location: &'ast SrcSpan,
        _name: &'ast EcoString,
        constructor: &'ast Option<ValueConstructor>,
        _type_: &'ast std::sync::Arc<Type>,
    ) {
        let variant = match constructor {
            Some(constructor) => &constructor.variant,
            None => return,
        };
        match variant {
            ValueConstructorVariant::LocalVariable {
                location: definition_location,
                ..
            } if *definition_location == self.definition_location => {
                self.references.push(*location)
            }
            _ => {}
        }
    }
}
