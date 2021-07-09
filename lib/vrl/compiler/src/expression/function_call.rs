use crate::expression::{levenstein, ExpressionError, FunctionArgument, Noop};
use crate::function::{ArgumentList, Parameter};
use crate::parser::{Ident, Node};
use crate::{value::Kind, Context, Expression, Function, Resolved, Span, State, TypeDef};
use diagnostic::{DiagnosticError, Label, Note, Urls};
use std::fmt;
use tracing::{span, Level};

#[derive(Clone)]
pub struct FunctionCall {
    abort_on_error: bool,
    expr: Box<dyn Expression>,
    maybe_fallible_arguments: bool,

    // used for enhancing runtime error messages (using abort-instruction).
    //
    // TODO: have span store line/col details to further improve this.
    span: Span,

    // Used for pretty-printing function call.
    //
    // This allows us to keep the arguments non-cloneable.
    arguments_fmt: Vec<String>,
    arguments_dbg: Vec<String>,

    // used for equality check
    ident: &'static str,
}

impl FunctionCall {
    pub fn new(
        call_span: Span,
        ident: Node<Ident>,
        abort_on_error: bool,
        arguments: Vec<Node<FunctionArgument>>,
        funcs: &[Box<dyn Function>],
        state: &State,
    ) -> Result<Self, Error> {
        let (ident_span, ident) = ident.take();

        // Check if function exists.
        let function = match funcs.iter().find(|f| f.identifier() == ident.as_ref()) {
            Some(function) => function,
            None => {
                let idents = funcs
                    .iter()
                    .map(|func| func.identifier())
                    .collect::<Vec<_>>();

                return Err(Error::Undefined {
                    ident_span,
                    ident: ident.clone(),
                    idents,
                });
            }
        };

        // Check function arity.
        if arguments.len() > function.parameters().len() {
            let arguments_span = {
                let start = arguments.first().unwrap().span().start();
                let end = arguments.last().unwrap().span().end();

                Span::new(start, end)
            };

            return Err(Error::WrongNumberOfArgs {
                arguments_span,
                max: function.parameters().len(),
            });
        }

        // Keeps track of positional argument indices.
        //
        // Used to map a positional argument to its keyword. Keyword arguments
        // can be used in any order, and don't count towards the index of
        // positional arguments.
        let mut index = 0;
        let mut list = ArgumentList::default();

        let arguments_fmt = arguments
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>();

        let arguments_dbg = arguments
            .iter()
            .map(|arg| format!("{:?}", arg))
            .collect::<Vec<_>>();

        let mut maybe_fallible_arguments = false;
        for node in arguments {
            let (argument_span, argument) = node.take();

            let parameter = match argument.keyword() {
                // positional argument
                None => {
                    index += 1;
                    function.parameters().get(index - 1)
                }

                // keyword argument
                Some(k) => function
                    .parameters()
                    .iter()
                    .enumerate()
                    .find(|(_, param)| param.keyword == k)
                    .map(|(pos, param)| {
                        if pos == index {
                            index += 1;
                        }

                        param
                    }),
            }
            .ok_or_else(|| Error::UnknownKeyword {
                keyword_span: argument.keyword_span().expect("exists"),
                ident_span,
                keywords: function.parameters().iter().map(|p| p.keyword).collect(),
            })?;

            // Check if the argument is of the expected type.
            let expr_kind = argument.type_def(state).kind();
            let param_kind = parameter.kind();

            if !param_kind.intersects(expr_kind) {
                return Err(Error::InvalidArgumentKind {
                    function_ident: function.identifier(),
                    abort_on_error,
                    arguments_fmt,
                    parameter: *parameter,
                    got: expr_kind,
                    argument,
                    argument_span,
                });
            } else if !param_kind.contains(expr_kind) {
                maybe_fallible_arguments = true;
            }

            // Check if the argument is infallible.
            if argument.type_def(state).is_fallible() {
                return Err(Error::FallibleArgument {
                    expr_span: argument.span(),
                });
            }

            list.insert(parameter.keyword, argument.into_inner());
        }

        // Check missing required arguments.
        function
            .parameters()
            .iter()
            .enumerate()
            .filter(|(_, p)| p.required)
            .filter(|(_, p)| !list.keywords().contains(&p.keyword))
            .try_for_each(|(i, p)| -> Result<_, _> {
                Err(Error::MissingArgument {
                    call_span,
                    keyword: p.keyword,
                    position: i,
                })
            })?;

        let expr = function
            .compile(list)
            .map_err(|error| Error::Compilation { call_span, error })?;

        // Asking for an infallible function to abort on error makes no sense.
        // We consider this an error at compile-time, because it makes the
        // resulting program incorrectly convey this function call might fail.
        if abort_on_error && !maybe_fallible_arguments && !expr.type_def(state).is_fallible() {
            return Err(Error::AbortInfallible {
                ident_span,
                abort_span: Span::new(ident_span.end(), ident_span.end() + 1),
            });
        }

        Ok(Self {
            abort_on_error,
            expr,
            maybe_fallible_arguments,
            span: call_span,
            arguments_fmt,
            arguments_dbg,
            ident: function.identifier(),
        })
    }

    pub fn noop() -> Self {
        let expr = Box::new(Noop) as _;

        Self {
            abort_on_error: false,
            expr,
            maybe_fallible_arguments: false,
            span: Span::default(),
            arguments_fmt: vec![],
            arguments_dbg: vec![],
            ident: "noop",
        }
    }
}

impl Expression for FunctionCall {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        span!(Level::ERROR, "remap", vrl_position = &self.span.start()).in_scope(|| {
            self.expr.resolve(ctx).map_err(|err| match err {
                ExpressionError::Abort { .. } => {
                    panic!("abort errors must only be defined by `abort` statement")
                }
                ExpressionError::Error {
                    message,
                    mut labels,
                    notes,
                } => {
                    labels.push(Label::primary(message.clone(), self.span));

                    ExpressionError::Error {
                        message: format!(
                            r#"function call error for "{}" at ({}:{}): {}"#,
                            self.ident,
                            self.span.start(),
                            self.span.end(),
                            message
                        ),
                        labels,
                        notes,
                    }
                }
            })
        })
    }

    fn type_def(&self, state: &State) -> TypeDef {
        let mut type_def = self.expr.type_def(state);

        // If one of the arguments only partially matches the function type
        // definition, then we mark the entire function as fallible.
        //
        // This allows for progressive type-checking, by handling any potential
        // type error the function throws, instead of having to enforce
        // exact-type invariants for individual arguments.
        //
        // That is, this program triggers the `InvalidArgumentKind` error:
        //
        //     slice(10, 1)
        //
        // This is because `slice` expects either a string or an array, but it
        // receives an integer. The concept of "progressive type checking" does
        // not apply in this case, because this call can never succeed.
        //
        // However, given these example events:
        //
        //     { "foo": "bar" }
        //     { "foo": 10.5 }
        //
        // If we were to run the same program, but against the `foo` field:
        //
        //     slice(.foo, 1)
        //
        // In this situation, progressive type checking _does_ make sense,
        // because we can't know at compile-time what the eventual value of
        // `.foo` will be. We mark `.foo` as "any", which includes the "array"
        // and "string" types, so the program can now be made infallible by
        // handling any potential type error the function returns:
        //
        //     slice(.foo, 1) ?? []
        //
        // Note that this rule doesn't just apply to "any" kind (in fact, "any"
        // isn't a kind, it's simply a term meaning "all possible VRL values"),
        // but it applies whenever there's an _intersection_ but not an exact
        // _match_ between two types.
        //
        // Here's another example to demonstrate this:
        //
        //     { "foo": "foobar" }
        //     { "foo": ["foo", "bar"] }
        //     { "foo": 10.5 }
        //
        //     foo = slice(.foo, 1) ?? .foo
        //     .foo = upcase(foo) ?? foo
        //
        // This would result in the following outcomes:
        //
        //     { "foo": "OOBAR" }
        //     { "foo": ["bar", "baz"] }
        //     { "foo": 10.5 }
        //
        // For the first event, both the `slice` and `upcase` functions succeed.
        // For the second event, only the `slice` function succeeds.
        // For the third event, both functions fail.
        //
        if self.maybe_fallible_arguments {
            type_def.fallible = true;
        }

        if self.abort_on_error {
            type_def.fallible = false;
        }

        type_def
    }
}

impl fmt::Display for FunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.ident.fmt(f)?;
        f.write_str("(")?;

        let mut iter = self.arguments_fmt.iter().peekable();
        while let Some(arg) = iter.next() {
            f.write_str(arg)?;

            if iter.peek().is_some() {
                f.write_str(", ")?;
            }
        }

        f.write_str(")")
    }
}

impl fmt::Debug for FunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FunctionCall(")?;
        self.ident.fmt(f)?;

        f.write_str("(")?;

        let mut iter = self.arguments_dbg.iter().peekable();
        while let Some(arg) = iter.next() {
            f.write_str(arg)?;

            if iter.peek().is_some() {
                f.write_str(", ")?;
            }
        }

        f.write_str("))")
    }
}

impl PartialEq for FunctionCall {
    fn eq(&self, other: &Self) -> bool {
        self.ident == other.ident
    }
}

// -----------------------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Error {
    #[error("call to undefined function")]
    Undefined {
        ident_span: Span,
        ident: Ident,
        idents: Vec<&'static str>,
    },

    #[error("wrong number of function arguments")]
    WrongNumberOfArgs { arguments_span: Span, max: usize },

    #[error("unknown function argument keyword")]
    UnknownKeyword {
        keyword_span: Span,
        ident_span: Span,
        keywords: Vec<&'static str>,
    },

    #[error("missing function argument")]
    MissingArgument {
        call_span: Span,
        keyword: &'static str,
        position: usize,
    },

    #[error("function compilation error: error[E{}] {}", error.code(), error)]
    Compilation {
        call_span: Span,
        error: Box<dyn DiagnosticError>,
    },

    #[error("can't abort infallible function")]
    AbortInfallible { ident_span: Span, abort_span: Span },

    #[error("invalid argument type")]
    InvalidArgumentKind {
        function_ident: &'static str,
        abort_on_error: bool,
        arguments_fmt: Vec<String>,
        parameter: Parameter,
        got: Kind,
        argument: FunctionArgument,
        argument_span: Span,
    },

    #[error("fallible argument")]
    FallibleArgument { expr_span: Span },
}

impl DiagnosticError for Error {
    fn code(&self) -> usize {
        use Error::*;

        match self {
            Undefined { .. } => 105,
            WrongNumberOfArgs { .. } => 106,
            UnknownKeyword { .. } => 108,
            Compilation { .. } => 610,
            MissingArgument { .. } => 107,
            AbortInfallible { .. } => 620,
            InvalidArgumentKind { .. } => 110,
            FallibleArgument { .. } => 630,
        }
    }

    fn labels(&self) -> Vec<Label> {
        use Error::*;

        match self {
            Undefined {
                ident_span,
                ident,
                idents,
            } => {
                let mut vec = vec![Label::primary("undefined function", ident_span)];
                let ident_chars = ident.as_ref().chars().collect::<Vec<_>>();

                if let Some((idx, _)) = idents
                    .iter()
                    .map(|possible| {
                        let possible_chars = possible.chars().collect::<Vec<_>>();
                        levenstein::distance(&ident_chars, &possible_chars)
                    })
                    .enumerate()
                    .min_by_key(|(_, score)| *score)
                {
                    {
                        let guessed: &str = idents[idx];
                        vec.push(Label::context(
                            format!(r#"did you mean "{}"?"#, guessed),
                            ident_span,
                        ));
                    }
                }

                vec
            }

            WrongNumberOfArgs {
                arguments_span,
                max,
            } => {
                let arg = if *max == 1 { "argument" } else { "arguments" };

                vec![
                    Label::primary("too many function arguments", arguments_span),
                    Label::context(
                        format!("this function takes a maximum of {} {}", max, arg),
                        arguments_span,
                    ),
                ]
            }

            UnknownKeyword {
                keyword_span,
                ident_span,
                keywords,
            } => vec![
                Label::primary("unknown keyword", keyword_span),
                Label::context(
                    format!(
                        "this function accepts the following keywords: {}",
                        keywords
                            .iter()
                            .map(|k| format!(r#""{}""#, k))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    ident_span,
                ),
            ],

            Compilation { call_span, error } => error
                .labels()
                .into_iter()
                .map(|mut label| {
                    label.span = *call_span;
                    label
                })
                .collect(),

            MissingArgument {
                call_span,
                keyword,
                position,
            } => {
                vec![Label::primary(
                    format!(
                        r#"required argument missing: "{}" (position {})"#,
                        keyword, position
                    ),
                    call_span,
                )]
            }

            AbortInfallible {
                ident_span,
                abort_span,
            } => {
                vec![
                    Label::primary("this function can't fail", ident_span),
                    Label::context("remove this abort-instruction", abort_span),
                ]
            }

            InvalidArgumentKind {
                parameter,
                got,
                argument,
                argument_span,
                ..
            } => {
                let keyword = parameter.keyword;
                let expected = parameter.kind();
                let expr_span = argument.span();

                // TODO: extract this out into a helper
                let kind_str = |kind: &Kind| {
                    if kind.is_any() {
                        kind.to_string()
                    } else if !kind.is_many() {
                        format!(r#"the exact type {}"#, kind)
                    } else {
                        format!("one of {}", kind)
                    }
                };

                vec![
                    Label::primary(
                        format!("this expression resolves to {}", kind_str(got)),
                        expr_span,
                    ),
                    Label::context(
                        format!(
                            r#"but the parameter "{}" expects {}"#,
                            keyword,
                            kind_str(&expected)
                        ),
                        argument_span,
                    ),
                ]
            }

            FallibleArgument { expr_span } => vec![
                Label::primary("this expression can fail", expr_span),
                Label::context(
                    "handle the error before passing it in as an argument",
                    expr_span,
                ),
            ],
        }
    }

    fn notes(&self) -> Vec<Note> {
        use Error::*;

        match self {
            WrongNumberOfArgs { .. } => vec![Note::SeeDocs(
                "function arguments".to_owned(),
                Urls::expression_docs_url("#arguments"),
            )],
            AbortInfallible { .. } | FallibleArgument { .. } => vec![Note::SeeErrorDocs],
            InvalidArgumentKind {
                function_ident,
                abort_on_error,
                arguments_fmt,
                parameter,
                argument,
                ..
            } => {
                // TODO: move this into a generic helper function
                let guard = match parameter.kind() {
                    Kind::Bytes => format!("string!({})", argument),
                    Kind::Integer => format!("int!({})", argument),
                    Kind::Float => format!("float!({})", argument),
                    Kind::Boolean => format!("bool!({})", argument),
                    Kind::Object => format!("object!({})", argument),
                    Kind::Array => format!("array!({})", argument),
                    Kind::Timestamp => format!("timestamp!({})", argument),
                    _ => return vec![],
                };

                let coerce = match parameter.kind() {
                    Kind::Bytes => Some(format!(r#"to_string({}) ?? "default""#, argument)),
                    Kind::Integer => Some(format!("to_int({}) ?? 0", argument)),
                    Kind::Float => Some(format!("to_float({}) ?? 0", argument)),
                    Kind::Boolean => Some(format!("to_bool({}) ?? false", argument)),
                    Kind::Timestamp => Some(format!("to_timestamp({}) ?? now()", argument)),
                    _ => None,
                };

                let args = {
                    let mut args = String::new();
                    let mut iter = arguments_fmt.iter().peekable();
                    while let Some(arg) = iter.next() {
                        args.push_str(arg);
                        if iter.peek().is_some() {
                            args.push_str(", ");
                        }
                    }

                    args
                };

                let abort = if *abort_on_error { "!" } else { "" };

                let mut notes = vec![];

                let call = format!("{}{}({})", function_ident, abort, args);

                notes.append(&mut Note::solution(
                    "ensuring an appropriate type at runtime",
                    vec![format!("{} = {}", argument, guard), call.clone()],
                ));

                if let Some(coerce) = coerce {
                    notes.append(&mut Note::solution(
                        "coercing to an appropriate type and specifying a default value as a fallback in case coercion fails",
                        vec![format!("{} = {}", argument, coerce), call],
                    ))
                }

                notes.push(Note::SeeErrorDocs);

                notes
            }

            Compilation { error, .. } => error.notes(),

            _ => vec![],
        }
    }
}
