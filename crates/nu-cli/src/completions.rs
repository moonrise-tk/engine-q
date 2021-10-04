use std::{cell::RefCell, rc::Rc};

use nu_engine::eval_block;
use nu_parser::{flatten_block, parse};
use nu_protocol::{
    engine::{EngineState, EvaluationContext, Stack, StateWorkingSet},
    Value,
};
use reedline::Completer;

const SEP: char = std::path::MAIN_SEPARATOR;

pub struct NuCompleter {
    engine_state: Rc<RefCell<EngineState>>,
}

impl NuCompleter {
    pub fn new(engine_state: Rc<RefCell<EngineState>>) -> Self {
        Self { engine_state }
    }
}

impl Completer for NuCompleter {
    fn complete(&self, line: &str, pos: usize) -> Vec<(reedline::Span, String)> {
        let engine_state = self.engine_state.borrow();
        let mut working_set = StateWorkingSet::new(&*engine_state);
        let offset = working_set.next_span_start();
        let pos = offset + pos;
        let (output, _err) = parse(&mut working_set, Some("completer"), line.as_bytes(), false);

        let flattened = flatten_block(&working_set, &output);

        // println!("flattened: {:?}", flattened);

        for flat in flattened {
            if pos >= flat.0.start && pos <= flat.0.end {
                match &flat.1 {
                    nu_parser::FlatShape::Custom(custom_completion) => {
                        let prefix = working_set.get_span_contents(flat.0).to_vec();

                        let (block, ..) =
                            parse(&mut working_set, None, custom_completion.as_bytes(), false);
                        let context = EvaluationContext {
                            engine_state: self.engine_state.clone(),
                            stack: Stack::default(),
                        };
                        let result = eval_block(&context, &block, Value::nothing());

                        let v: Vec<_> = match result {
                            Ok(Value::List { vals, .. }) => vals
                                .into_iter()
                                .map(move |x| {
                                    let s = x.as_string().expect("FIXME");

                                    (
                                        reedline::Span {
                                            start: flat.0.start - offset,
                                            end: flat.0.end - offset,
                                        },
                                        s,
                                    )
                                })
                                .filter(|x| x.1.as_bytes().starts_with(&prefix))
                                .collect(),
                            _ => vec![],
                        };

                        return v;
                    }
                    nu_parser::FlatShape::External | nu_parser::FlatShape::InternalCall => {
                        let prefix = working_set.get_span_contents(flat.0);
                        let results = working_set.find_commands_by_prefix(prefix);

                        return results
                            .into_iter()
                            .map(move |x| {
                                (
                                    reedline::Span {
                                        start: flat.0.start - offset,
                                        end: flat.0.end - offset,
                                    },
                                    String::from_utf8_lossy(&x).to_string(),
                                )
                            })
                            .collect();
                    }
                    nu_parser::FlatShape::Filepath | nu_parser::FlatShape::GlobPattern => {
                        let prefix = working_set.get_span_contents(flat.0);
                        let prefix = String::from_utf8_lossy(prefix).to_string();

                        let results = file_path_completion(flat.0, &prefix);

                        return results
                            .into_iter()
                            .map(move |x| {
                                (
                                    reedline::Span {
                                        start: x.0.start - offset,
                                        end: x.0.end - offset,
                                    },
                                    x.1,
                                )
                            })
                            .collect();
                    }
                    _ => {}
                }
            }
        }

        vec![]
    }
}

fn file_path_completion(
    span: nu_protocol::Span,
    partial: &str,
) -> Vec<(nu_protocol::Span, String)> {
    use std::path::{is_separator, Path};

    let (base_dir_name, partial) = {
        // If partial is only a word we want to search in the current dir
        let (base, rest) = partial.rsplit_once(is_separator).unwrap_or((".", partial));
        // On windows, this standardizes paths to use \
        let mut base = base.replace(is_separator, &SEP.to_string());

        // rsplit_once removes the separator
        base.push(SEP);
        (base, rest)
    };

    let base_dir = nu_path::expand_path(&base_dir_name);
    // This check is here as base_dir.read_dir() with base_dir == "" will open the current dir
    // which we don't want in this case (if we did, base_dir would already be ".")
    if base_dir == Path::new("") {
        return Vec::new();
    }

    if let Ok(result) = base_dir.read_dir() {
        result
            .filter_map(|entry| {
                entry.ok().and_then(|entry| {
                    let mut file_name = entry.file_name().to_string_lossy().into_owned();
                    if matches(partial, &file_name) {
                        let mut path = format!("{}{}", base_dir_name, file_name);
                        if entry.path().is_dir() {
                            path.push(SEP);
                            file_name.push(SEP);
                        }

                        Some((span, path))
                    } else {
                        None
                    }
                })
            })
            .collect()
    } else {
        Vec::new()
    }
}

fn matches(partial: &str, from: &str) -> bool {
    from.to_ascii_lowercase()
        .starts_with(&partial.to_ascii_lowercase())
}