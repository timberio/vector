use std::{iter::Peekable, str::CharIndices};

use crate::lexer::StringLiteral::Escaped;

pub type Tok<'input> = Token<&'input str>;
pub type SpannedResult<'input, Loc> = Result<Spanned<'input, Loc>, Error>;
pub type Spanned<'input, Loc> = (Loc, Tok<'input>, Loc);

#[derive(Clone, PartialEq, Debug)]
pub enum Token<S> {
    LRule,
    RRule,
    LBracket,
    RBracket,
    Colon,
    LParen,
    RParen,
    Dot,
    Comma,
    Null,
    True,
    False,

    Sign(S),

    IntegerLiteral(i64),
    FloatLiteral(f64),
    StringLiteral(StringLiteral<S>),
    Identifier(S),
    ExtendedIdentifier(S),
    InvalidToken(char),
}

#[derive(thiserror::Error, Clone, Debug, PartialEq)]
pub enum Error {
    #[error("invalid literal")]
    Literal { start: usize },

    #[error("invalid numeric literal")]
    NumericLiteral(String),
}

pub struct Lexer<'input> {
    input: &'input str,
    chars: Peekable<CharIndices<'input>>,
}

// -----------------------------------------------------------------------------
// lexing iterator
// -----------------------------------------------------------------------------

impl<'input> Iterator for Lexer<'input> {
    type Item = SpannedResult<'input, usize>;

    fn next(&mut self) -> Option<Self::Item> {
        use Token::*;

        loop {
            if let Some((start, ch)) = self.bump() {
                let result = match ch {
                    '%' if self.test_peek(|ch| ch == '{') => {
                        self.bump();
                        Some(Ok(self.token(start, LRule)))
                    }
                    '}' => Some(Ok(self.token(start, RRule))),
                    '[' => Some(Ok(self.token(start, LBracket))),
                    ']' => Some(Ok(self.token(start, RBracket))),
                    '(' => Some(Ok(self.token(start, LParen))),
                    ')' => Some(Ok(self.token(start, RParen))),

                    '.' if self.test_peek(is_digit) => Some(self.numeric_literal(start)),
                    '.' => Some(Ok(self.token(start, Dot))),
                    ':' => Some(Ok(self.token(start, Colon))),
                    ',' => Some(Ok(self.token(start, Comma))),

                    '"' => Some(self.string_literal(start, |s| StringLiteral(Escaped(s)))),

                    '+' => Some(Ok(self.token(start, Sign("+")))),
                    '-' => Some(Ok(self.token(start, Sign("-")))),
                    ch if is_ident_start(ch) => Some(Ok(self.identifier(start))),
                    ch if is_digit(ch) => Some(self.numeric_literal(start)),

                    ch if ch.is_whitespace() => continue,

                    ch => Some(Ok(self.token(start, InvalidToken(ch)))),
                };

                return result;
            } else {
                return None;
            }
        }
    }
}

// -----------------------------------------------------------------------------
// lexing helpers
// -----------------------------------------------------------------------------

impl<'input> Lexer<'input> {
    pub fn new(input: &'input str) -> Lexer<'input> {
        Self {
            input,
            chars: input.char_indices().peekable(),
        }
    }

    fn bump(&mut self) -> Option<(usize, char)> {
        self.chars.next()
    }

    fn peek(&mut self) -> Option<(usize, char)> {
        self.chars.peek().copied()
    }

    fn take_while<F>(&mut self, start: usize, mut keep_going: F) -> (usize, &'input str)
    where
        F: FnMut(char) -> bool,
    {
        self.take_until(start, |c| !keep_going(c))
    }

    fn take_until<F>(&mut self, start: usize, mut terminate: F) -> (usize, &'input str)
    where
        F: FnMut(char) -> bool,
    {
        while let Some((end, ch)) = self.peek() {
            if terminate(ch) {
                return (end, self.slice(start, end));
            } else {
                self.bump();
            }
        }

        let loc = self.next_index();

        (loc, self.slice(start, loc))
    }

    fn test_peek<F>(&mut self, mut test: F) -> bool
    where
        F: FnMut(char) -> bool,
    {
        self.peek().map_or(false, |(_, ch)| test(ch))
    }

    fn slice(&self, start: usize, end: usize) -> &'input str {
        &self.input[start..end]
    }

    fn next_index(&mut self) -> usize {
        self.peek().as_ref().map_or(self.input.len(), |l| l.0)
    }

    fn token(&mut self, start: usize, token: Token<&'input str>) -> Spanned<'input, usize> {
        let end = self.next_index();
        self.token2(start, end, token)
    }

    fn token2(
        &mut self,
        start: usize,
        end: usize,
        token: Token<&'input str>,
    ) -> Spanned<'input, usize> {
        (start, token, end)
    }

    fn string_literal(
        &mut self,
        start: usize,
        tok: impl Fn(&'input str) -> Tok<'input>,
    ) -> SpannedResult<'input, usize> {
        let content_start = self.next_index();

        loop {
            let scan_start = self.next_index();
            self.take_until(scan_start, |c| c == '"' || c == '\\');

            match self.bump() {
                Some((_, '\\')) => self.bump(),
                Some((end, '\"')) => {
                    let content = self.slice(content_start, end);
                    let token = tok(content);
                    let end = self.next_index();

                    return Ok((start, token, end));
                }
                _ => break,
            };
        }

        Err(Error::Literal { start })
    }

    fn identifier(&mut self, start: usize) -> Spanned<'input, usize> {
        use Token::*;

        let (end, ident) = self.take_while(start, is_ident_continue);

        let token = match ident {
            "true" => True,
            "false" => False,
            "null" => Null,

            _ if ident.contains('@') || ident.contains('-') => ExtendedIdentifier(ident),
            _ => Identifier(ident),
        };

        (start, token, end)
    }

    fn numeric_literal(&mut self, start: usize) -> SpannedResult<'input, usize> {
        let mut is_float = false;
        let (end, num) = self.take_while(start, |ch| {
            is_digit(ch) || {
                let is_float_symbol = is_float_literal_symbol(ch);
                if is_float_symbol {
                    is_float = true;
                }
                is_float_symbol
            }
        });

        if is_float || num.starts_with('.') {
            num.parse()
                .map_err(|_| Error::NumericLiteral(num.to_string()))
                .map(|n| (start, Token::FloatLiteral(n), end))
        } else {
            num.parse()
                .map_err(|_| Error::NumericLiteral(num.to_string()))
                .map(|n| (start, Token::IntegerLiteral(n), end))
        }
    }
}

fn is_float_literal_symbol(ch: char) -> bool {
    matches!(ch, 'e' | 'E' | '-' | '+' | '.')
}

fn is_ident_start(ch: char) -> bool {
    matches!(ch, '$' | '@' | '_' | 'a'..='z' | 'A'..='Z')
}

fn is_ident_continue(ch: char) -> bool {
    match ch {
        '0'..='9' => true,
        '-' => true,
        ch => is_ident_start(ch),
    }
}

fn is_digit(ch: char) -> bool {
    ch.is_digit(10)
}

fn unescape_string_literal(mut s: &str) -> String {
    let mut string = String::with_capacity(s.len());
    while let Some(i) = s.bytes().position(|b| b == b'\\') {
        let c = match s.as_bytes()[i + 1] {
            b'\'' => '\'',
            b'"' => '"',
            b'\\' => '\\',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            _ => unimplemented!("invalid escape"),
        };

        string.push_str(&s[..i]);
        string.push(c);
        s = &s[i + 2..];
    }

    string.push_str(s);
    string
}

#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub enum StringLiteral<S> {
    Escaped(S),
}

impl StringLiteral<&str> {
    pub fn unescape(&self) -> String {
        match self {
            StringLiteral::Escaped(s) => unescape_string_literal(s),
        }
    }
}

pub struct FloatingPointLiteral<'input> {
    pub integral: Option<&'input str>,
    pub fraction: Option<&'input str>,
    pub exponent: Option<Exponent<'input>>,
}

pub struct Exponent<'input> {
    pub sign: Option<&'input str>,
    pub value: &'input str,
}

#[allow(dead_code)] // used by generated lalrpop parser
impl<'input> FloatingPointLiteral<'input> {
    pub fn parse(&self) -> f64 {
        let mut fp = String::new();
        fp.push_str(self.integral.unwrap_or_default());
        if let Some(f) = &self.fraction {
            fp.push('.');
            fp.push_str(f);
        }

        if let Some(exp) = &self.exponent {
            fp.push('e');
            fp.push_str(exp.sign.unwrap_or_default());
            fp.push_str(exp.value);
        }

        fp.parse().map_err(|_| Error::NumericLiteral(fp)).unwrap()
    }
}
