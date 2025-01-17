use nu_protocol::{
    ast::{Block, Call, Expr, Expression, ImportPatternMember, Pipeline, Statement},
    engine::StateWorkingSet,
    span, DeclId, Span, SyntaxShape, Type,
};

use crate::{
    lex, lite_parse,
    parser::{
        check_name, garbage, garbage_statement, parse_block_expression, parse_import_pattern,
        parse_internal_call, parse_signature, parse_string,
    },
    ParseError,
};

pub fn parse_def_predecl(working_set: &mut StateWorkingSet, spans: &[Span]) {
    let name = working_set.get_span_contents(spans[0]);

    if name == b"def" && spans.len() >= 4 {
        let (name_expr, ..) = parse_string(working_set, spans[1]);
        let name = name_expr.as_string();

        working_set.enter_scope();
        // FIXME: because parse_signature will update the scope with the variables it sees
        // we end up parsing the signature twice per def. The first time is during the predecl
        // so that we can see the types that are part of the signature, which we need for parsing.
        // The second time is when we actually parse the body itworking_set.
        // We can't reuse the first time because the variables that are created during parse_signature
        // are lost when we exit the scope below.
        let (sig, ..) = parse_signature(working_set, spans[2]);
        let signature = sig.as_signature();
        working_set.exit_scope();

        if let (Some(name), Some(mut signature)) = (name, signature) {
            signature.name = name;
            let decl = signature.predeclare();

            working_set.add_decl(decl);
        }
    }
}

pub fn parse_def(
    working_set: &mut StateWorkingSet,
    spans: &[Span],
) -> (Statement, Option<ParseError>) {
    let mut error = None;
    let name = working_set.get_span_contents(spans[0]);

    if name == b"def" {
        let def_decl_id = working_set
            .find_decl(b"def")
            .expect("internal error: missing def command");

        let mut call = Box::new(Call {
            head: spans[0],
            decl_id: def_decl_id,
            positional: vec![],
            named: vec![],
        });

        let call = if let Some(name_span) = spans.get(1) {
            let (name_expr, err) = parse_string(working_set, *name_span);
            error = error.or(err);

            let name = name_expr.as_string();
            call.positional.push(name_expr);

            if let Some(sig_span) = spans.get(2) {
                working_set.enter_scope();
                let (sig, err) = parse_signature(working_set, *sig_span);
                error = error.or(err);

                let signature = sig.as_signature();

                call.positional.push(sig);

                if let Some(block_span) = spans.get(3) {
                    let (block, err) = parse_block_expression(
                        working_set,
                        &SyntaxShape::Block(Some(vec![])),
                        *block_span,
                    );
                    error = error.or(err);

                    let block_id = block.as_block();

                    call.positional.push(block);

                    if let (Some(name), Some(mut signature), Some(block_id)) =
                        (name, signature, block_id)
                    {
                        let decl_id = working_set
                            .find_decl(name.as_bytes())
                            .expect("internal error: predeclaration failed to add definition");

                        let declaration = working_set.get_decl_mut(decl_id);

                        signature.name = name;

                        *declaration = signature.into_block_command(block_id);
                    }
                } else {
                    let err_span = Span {
                        start: sig_span.end,
                        end: sig_span.end,
                    };

                    error = error
                        .or_else(|| Some(ParseError::MissingPositional("block".into(), err_span)));
                }
                working_set.exit_scope();

                call
            } else {
                let err_span = Span {
                    start: name_span.end,
                    end: name_span.end,
                };

                error = error
                    .or_else(|| Some(ParseError::MissingPositional("parameters".into(), err_span)));

                call
            }
        } else {
            let err_span = Span {
                start: spans[0].end,
                end: spans[0].end,
            };

            error = error.or_else(|| {
                Some(ParseError::MissingPositional(
                    "definition name".into(),
                    err_span,
                ))
            });

            call
        };

        (
            Statement::Pipeline(Pipeline::from_vec(vec![Expression {
                expr: Expr::Call(call),
                span: span(spans),
                ty: Type::Unknown,
                custom_completion: None,
            }])),
            error,
        )
    } else {
        (
            garbage_statement(spans),
            Some(ParseError::UnknownState(
                "Expected structure: def <name> [] {}".into(),
                span(spans),
            )),
        )
    }
}

pub fn parse_alias(
    working_set: &mut StateWorkingSet,
    spans: &[Span],
) -> (Statement, Option<ParseError>) {
    let name = working_set.get_span_contents(spans[0]);

    if name == b"alias" {
        if let Some((span, err)) = check_name(working_set, spans) {
            return (
                Statement::Pipeline(Pipeline::from_vec(vec![garbage(*span)])),
                Some(err),
            );
        }

        if let Some(decl_id) = working_set.find_decl(b"alias") {
            let (call, call_span, _) =
                parse_internal_call(working_set, spans[0], &spans[1..], decl_id);

            if spans.len() >= 4 {
                let alias_name = working_set.get_span_contents(spans[1]);

                let alias_name = if alias_name.starts_with(b"\"")
                    && alias_name.ends_with(b"\"")
                    && alias_name.len() > 1
                {
                    alias_name[1..(alias_name.len() - 1)].to_vec()
                } else {
                    alias_name.to_vec()
                };
                let _equals = working_set.get_span_contents(spans[2]);

                let replacement = spans[3..].to_vec();

                //println!("{:?} {:?}", alias_name, replacement);

                working_set.add_alias(alias_name, replacement);
            }

            return (
                Statement::Pipeline(Pipeline::from_vec(vec![Expression {
                    expr: Expr::Call(call),
                    span: call_span,
                    ty: Type::Unknown,
                    custom_completion: None,
                }])),
                None,
            );
        }
    }

    (
        garbage_statement(spans),
        Some(ParseError::UnknownState(
            "internal error: alias statement unparseable".into(),
            span(spans),
        )),
    )
}

pub fn parse_module(
    working_set: &mut StateWorkingSet,
    spans: &[Span],
) -> (Statement, Option<ParseError>) {
    // TODO: Currently, module is closing over its parent scope (i.e., defs in the parent scope are
    // visible and usable in this module's scope). We might want to disable that. How?

    let mut error = None;
    let bytes = working_set.get_span_contents(spans[0]);

    // parse_def() equivalent
    if bytes == b"module" && spans.len() >= 3 {
        let (module_name_expr, err) = parse_string(working_set, spans[1]);
        error = error.or(err);

        let module_name = module_name_expr
            .as_string()
            .expect("internal error: module name is not a string");

        // parse_block_expression() equivalent
        let block_span = spans[2];
        let block_bytes = working_set.get_span_contents(block_span);
        let mut start = block_span.start;
        let mut end = block_span.end;

        if block_bytes.starts_with(b"{") {
            start += 1;
        } else {
            return (
                garbage_statement(spans),
                Some(ParseError::Expected("block".into(), block_span)),
            );
        }

        if block_bytes.ends_with(b"}") {
            end -= 1;
        } else {
            error = error.or_else(|| {
                Some(ParseError::Unclosed(
                    "}".into(),
                    Span {
                        start: end,
                        end: end + 1,
                    },
                ))
            });
        }

        let block_span = Span { start, end };

        let source = working_set.get_span_contents(block_span);

        let (output, err) = lex(source, start, &[], &[]);
        error = error.or(err);

        working_set.enter_scope();

        // Do we need block parameters?

        let (output, err) = lite_parse(&output);
        error = error.or(err);

        // We probably don't need $it

        // we're doing parse_block() equivalent
        // let (mut output, err) = parse_block(working_set, &output, false);

        for pipeline in &output.block {
            if pipeline.commands.len() == 1 {
                parse_def_predecl(working_set, &pipeline.commands[0].parts);
            }
        }

        let mut exports: Vec<(Vec<u8>, DeclId)> = vec![];

        let block: Block = output
            .block
            .iter()
            .map(|pipeline| {
                if pipeline.commands.len() == 1 {
                    // this one here is doing parse_statement() equivalent
                    // let (stmt, err) = parse_statement(working_set, &pipeline.commands[0].parts);
                    let name = working_set.get_span_contents(pipeline.commands[0].parts[0]);

                    let (stmt, err) = match name {
                        // TODO: Here we can add other stuff that's alowed for modules
                        b"def" => {
                            let (stmt, err) = parse_def(working_set, &pipeline.commands[0].parts);

                            if err.is_none() {
                                let decl_name =
                                    working_set.get_span_contents(pipeline.commands[0].parts[1]);

                                let decl_id = working_set
                                    .find_decl(decl_name)
                                    .expect("internal error: failed to find added declaration");

                                // TODO: Later, we want to put this behind 'export'
                                exports.push((decl_name.into(), decl_id));
                            }

                            (stmt, err)
                        }
                        _ => (
                            garbage_statement(&pipeline.commands[0].parts),
                            Some(ParseError::Expected(
                                "def".into(),
                                pipeline.commands[0].parts[0],
                            )),
                        ),
                    };

                    if error.is_none() {
                        error = err;
                    }

                    stmt
                } else {
                    error = Some(ParseError::Expected("not a pipeline".into(), block_span));
                    garbage_statement(spans)
                }
            })
            .into();

        let block = block.with_exports(exports);

        working_set.exit_scope();

        let block_id = working_set.add_module(&module_name, block);

        let block_expr = Expression {
            expr: Expr::Block(block_id),
            span: block_span,
            ty: Type::Block,
            custom_completion: None,
        };

        let module_decl_id = working_set
            .find_decl(b"module")
            .expect("internal error: missing module command");

        let call = Box::new(Call {
            head: spans[0],
            decl_id: module_decl_id,
            positional: vec![module_name_expr, block_expr],
            named: vec![],
        });

        (
            Statement::Pipeline(Pipeline::from_vec(vec![Expression {
                expr: Expr::Call(call),
                span: span(spans),
                ty: Type::Unknown,
                custom_completion: None,
            }])),
            error,
        )
    } else {
        (
            garbage_statement(spans),
            Some(ParseError::UnknownState(
                "Expected structure: module <name> {}".into(),
                span(spans),
            )),
        )
    }
}

pub fn parse_use(
    working_set: &mut StateWorkingSet,
    spans: &[Span],
) -> (Statement, Option<ParseError>) {
    let mut error = None;
    let bytes = working_set.get_span_contents(spans[0]);

    // TODO: Currently, this directly imports the module's definitions into the current scope.
    // Later, we want to put them behind the module's name and add selective importing
    if bytes == b"use" && spans.len() >= 2 {
        let (module_name_expr, err) = parse_string(working_set, spans[1]);
        error = error.or(err);

        let (import_pattern, err) = parse_import_pattern(working_set, spans[1]);
        error = error.or(err);

        let exports = if let Some(block_id) = working_set.find_module(&import_pattern.head) {
            // TODO: Since we don't use the Block at all, we might just as well create a separate
            // Module that holds only the exports, without having Blocks in the way.
            working_set.get_block(block_id).exports.clone()
        } else {
            return (
                garbage_statement(spans),
                Some(ParseError::ModuleNotFound(spans[1])),
            );
        };

        let exports = if import_pattern.members.is_empty() {
            exports
                .into_iter()
                .map(|(name, id)| {
                    let mut new_name = import_pattern.head.to_vec();
                    new_name.push(b'.');
                    new_name.extend(&name);
                    (new_name, id)
                })
                .collect()
        } else {
            match &import_pattern.members[0] {
                ImportPatternMember::Glob { .. } => exports,
                ImportPatternMember::Name { name, span } => {
                    let new_exports: Vec<(Vec<u8>, usize)> =
                        exports.into_iter().filter(|x| &x.0 == name).collect();

                    if new_exports.is_empty() {
                        error = error.or(Some(ParseError::ExportNotFound(*span)))
                    }

                    new_exports
                }
                ImportPatternMember::List { names } => {
                    let mut output = vec![];

                    for (name, span) in names {
                        let mut new_exports: Vec<(Vec<u8>, usize)> = exports
                            .iter()
                            .filter_map(|x| if &x.0 == name { Some(x.clone()) } else { None })
                            .collect();

                        if new_exports.is_empty() {
                            error = error.or(Some(ParseError::ExportNotFound(*span)))
                        } else {
                            output.append(&mut new_exports)
                        }
                    }

                    output
                }
            }
        };

        // Extend the current scope with the module's exports
        working_set.activate_overlay(exports);

        // Create the Use command call
        let use_decl_id = working_set
            .find_decl(b"use")
            .expect("internal error: missing use command");

        let call = Box::new(Call {
            head: spans[0],
            decl_id: use_decl_id,
            positional: vec![module_name_expr],
            named: vec![],
        });

        (
            Statement::Pipeline(Pipeline::from_vec(vec![Expression {
                expr: Expr::Call(call),
                span: span(spans),
                ty: Type::Unknown,
                custom_completion: None,
            }])),
            error,
        )
    } else {
        (
            garbage_statement(spans),
            Some(ParseError::UnknownState(
                "Expected structure: use <name>".into(),
                span(spans),
            )),
        )
    }
}

pub fn parse_let(
    working_set: &mut StateWorkingSet,
    spans: &[Span],
) -> (Statement, Option<ParseError>) {
    let name = working_set.get_span_contents(spans[0]);

    if name == b"let" {
        if let Some((span, err)) = check_name(working_set, spans) {
            return (
                Statement::Pipeline(Pipeline::from_vec(vec![garbage(*span)])),
                Some(err),
            );
        }

        if let Some(decl_id) = working_set.find_decl(b"let") {
            let (call, call_span, err) =
                parse_internal_call(working_set, spans[0], &spans[1..], decl_id);

            // Update the variable to the known type if we can.
            if err.is_none() {
                let var_id = call.positional[0]
                    .as_var()
                    .expect("internal error: expected variable");
                let rhs_type = call.positional[1].ty.clone();

                working_set.set_variable_type(var_id, rhs_type);
            }

            return (
                Statement::Pipeline(Pipeline::from_vec(vec![Expression {
                    expr: Expr::Call(call),
                    span: call_span,
                    ty: Type::Unknown,
                    custom_completion: None,
                }])),
                err,
            );
        }
    }
    (
        garbage_statement(spans),
        Some(ParseError::UnknownState(
            "internal error: let statement unparseable".into(),
            span(spans),
        )),
    )
}
