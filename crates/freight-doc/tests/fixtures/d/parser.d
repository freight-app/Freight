/++
 + Simple recursive-descent expression parser.
 +/
module parser;

/++
 + Token kinds produced by the lexer.
 +/
enum TokenKind {
    Number,  /// Integer literal.
    Plus,    /// Addition operator.
    Minus,   /// Subtraction / unary negation.
    Star,    /// Multiplication operator.
    Slash,   /// Division operator.
    LParen,  /// Opening parenthesis.
    RParen,  /// Closing parenthesis.
    Eof,     /// End of input.
}

/++
 + A single lexer token with its kind and source text.
 +/
struct Token {
    TokenKind kind;  /// What kind of token this is.
    string    text;  /// Raw text from the source.
}

/++
 + Tokenise a source string into a flat array of tokens.
 +
 + Params:
 +   src = Source expression string.
 + Returns:
 +   Array of tokens; always ends with a `TokenKind.Eof` sentinel.
 + Throws:
 +   Exception on unrecognised characters.
 +/
Token[] tokenise(string src);

/++
 + Parse and evaluate a simple arithmetic expression.
 +
 + Supports `+`, `-`, `*`, `/`, unary negation, and parentheses.
 + Integer division truncates toward zero.
 +
 + Params:
 +   expr = Null-terminated expression string.
 + Returns:
 +   Computed integer result.
 + Throws:
 +   Exception on syntax errors or division by zero.
 +/
long evaluate(string expr);
