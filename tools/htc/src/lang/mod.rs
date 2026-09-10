//! Compiler for the `htc` language: a small C-like language for the
//! HT66F0185.  See `docs/language.md` for the reference.

pub mod ast;
pub mod codegen;
pub mod lexer;
pub mod parser;
pub mod runtime;

use std::fmt;

pub use codegen::Output;

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: error: {}",
            self.file, self.line, self.col, self.message
        )
    }
}

impl std::error::Error for Diagnostic {}

/// Compile `htc` source text to assembly.
pub fn compile(file: &str, source: &str) -> Result<Output, Vec<Diagnostic>> {
    let diag = |line, col, message: String| Diagnostic {
        file: file.to_string(),
        line,
        col,
        message,
    };
    let toks = lexer::tokenize(source).map_err(|e| vec![diag(e.line, e.col, e.message)])?;
    let prog = parser::Parser::new(toks)
        .parse_program()
        .map_err(|e| vec![diag(e.line, e.col, e.message)])?;
    codegen::Gen::new().compile(&prog).map_err(|errs| {
        errs.into_iter()
            .map(|e| diag(e.line, e.col, e.message))
            .collect()
    })
}
