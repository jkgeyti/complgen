use std::rc::Rc;

use nom::{
    branch::alt,
    bytes::complete::{is_not, tag, take_while1, escaped},
    character::{complete::{char, multispace0, multispace1, one_of}, is_alphanumeric},
    multi::many0,
    IResult, combinator::fail, error::context,
};

use complgen::{Error, Result};
use ustr::{Ustr, ustr, UstrMap};

// Can't use an arena here until proptest supports non-owned types: https://github.com/proptest-rs/proptest/issues/9
#[derive(Clone, PartialEq)]
pub enum Expr {
    Literal(Ustr), // e.g. an option: "--help", or a command: "build"
    Variable(Ustr), // e.g. <FILE>, <PATH>, <DIR>, etc.
    Sequence(Vec<Rc<Expr>>),
    Alternative(Vec<Rc<Expr>>),
    Optional(Rc<Expr>),
    Many1(Rc<Expr>),
}

impl std::fmt::Debug for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Literal(arg0) => f.write_fmt(format_args!(r#"Rc::new(Literal(ustr("{}")))"#, arg0)),
            Self::Variable(arg0) => f.write_fmt(format_args!(r#"Rc::new(Variable(ustr("{}")))"#, arg0)),
            Self::Sequence(arg0) => f.write_fmt(format_args!(r#"Rc::new(Sequence(vec!{:?}))"#, arg0)),
            Self::Alternative(arg0) => f.write_fmt(format_args!(r#"Rc::new(Alternative(vec!{:?}))"#, arg0)),
            Self::Optional(arg0) => f.write_fmt(format_args!(r#"Rc::new(Optional({:?}))"#, arg0)),
            Self::Many1(arg0) => f.write_fmt(format_args!(r#"Rc::new(Many1({:?}))"#, arg0)),
        }
    }
}

fn terminal(input: &str) -> IResult<&str, &str> {
    fn is_terminal_char(c: char) -> bool {
        c.is_ascii() && (is_alphanumeric(c as u8) || c == '-' || c == '+' || c == '_')
    }
    let (input, term) = escaped(take_while1(is_terminal_char), '\\', one_of(r#"()[]<>.|;"#))(input)?;
    if term.len() == 0 {
        return fail(input);
    }
    Ok((input, term))
}

fn terminal_expr(input: &str) -> IResult<&str, Expr> {
    let (input, literal) = context("terminal", terminal)(input)?;
    Ok((input, Expr::Literal(ustr(literal))))
}

fn symbol(input: &str) -> IResult<&str, &str> {
    let (input, _) = char('<')(input)?;
    let (input, name) = is_not(">")(input)?;
    let (input, _) = char('>')(input)?;
    Ok((input, name))
}

fn symbol_expr(input: &str) -> IResult<&str, Expr> {
    let (input, nonterm) = context("symbol", symbol)(input)?;
    Ok((input, Expr::Variable(ustr(nonterm))))
}

fn optional_expr(input: &str) -> IResult<&str, Expr> {
    let (input, _) = char('[')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, expr) = expr(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(']')(input)?;
    Ok((input, Expr::Optional(Rc::new(expr))))
}

fn parenthesized_expr(input: &str) -> IResult<&str, Expr> {
    let (input, _) = char('(')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, e) = expr(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(')')(input)?;
    Ok((input, e))
}

fn one_or_more_tag(input: &str) -> IResult<&str, ()> {
    let (input, _) = multispace0(input)?;
    let (input, _) = tag("...")(input)?;
    Ok((input, ()))
}

fn expr_no_alternative_no_sequence(input: &str) -> IResult<&str, Expr> {
    let (input, e) = alt((
        symbol_expr,
        optional_expr,
        parenthesized_expr,
        terminal_expr,
    ))(input)?;

    if let Ok((input, ())) = one_or_more_tag(input) {
        return Ok((input, Expr::Many1(Rc::new(e))));
    }

    Ok((input, e))
}

fn sequence_expr(input: &str) -> IResult<&str, Expr> {
    fn do_sequence_expr(input: &str) -> IResult<&str, Expr> {
        let (input, _) = multispace0(input)?;
        let (input, right) = sequence_expr(input)?;
        Ok((input, right))
    }

    let (mut input, left) = expr_no_alternative_no_sequence(input)?;
    let mut factors: Vec<Expr> = vec![left];
    loop {
        let Ok((pos, right)) = do_sequence_expr(input) else { break };
        factors.push(right);
        input = pos;
    }
    let result = if factors.len() == 1 {
        factors.drain(..).next().unwrap()
    } else {
        Expr::Sequence(factors.into_iter().map(Rc::new).collect())
    };
    Ok((input, result))
}

fn alternative_expr(input: &str) -> IResult<&str, Expr> {
    fn do_alternative_expr(input: &str) -> IResult<&str, Expr> {
        let (input, _) = multispace0(input)?;
        let (input, _) = char('|')(input)?;
        let (input, _) = multispace0(input)?;
        let (input, right) = sequence_expr(input)?;
        Ok((input, right))
    }

    let (mut input, left) = sequence_expr(input)?;
    let mut elems: Vec<Expr> = vec![left];
    loop {
        let Ok((pos, right)) = do_alternative_expr(input) else { break };
        elems.push(right);
        input = pos;
    }
    let result = if elems.len() == 1 {
        elems.drain(..).next().unwrap()
    } else {
        Expr::Alternative(elems.into_iter().map(Rc::new).collect())
    };
    Ok((input, result))
}

fn expr(input: &str) -> IResult<&str, Expr> {
    alternative_expr(input)
}


#[derive(Debug, Clone, PartialEq)]
enum Statement {
    CallVariant {
        lhs: Ustr,
        rhs: Rc<Expr>,
    },
    VariableDefinition {
        symbol: Ustr,
        rhs: Rc<Expr>,
    },
}


fn call_variant(input: &str) -> IResult<&str, Statement> {
    let (input, name) = terminal(input)?;
    let (input, _) = multispace1(input)?;
    let (input, expr) = expr(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(';')(input)?;

    let production = Statement::CallVariant {
        lhs: ustr(name),
        rhs: Rc::new(expr),
    };

    Ok((input, production))
}

fn variable_definition(input: &str) -> IResult<&str, Statement> {
    let (input, symbol) = symbol(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = tag("::=")(input)?;
    let (input, _) = multispace0(input)?;
    let (input, e) = expr(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(';')(input)?;

    let stmt = Statement::VariableDefinition {
        symbol: ustr(symbol),
        rhs: Rc::new(e),
    };

    Ok((input, stmt))
}

fn statement(input: &str) -> IResult<&str, Statement> {
    let (input, stmt) = alt((call_variant, variable_definition))(input)?;
    let (input, _) = multispace0(input)?;
    Ok((input, stmt))
}

fn grammar(input: &str) -> IResult<&str, Vec<Statement>> {
    let (input, _) = multispace0(input)?;
    let (input, statements) = many0(statement)(input)?;
    let (input, _) = multispace0(input)?;
    Ok((input, statements))
}


#[derive(Debug, PartialEq, Clone)]
pub struct Grammar {
    statements: Vec<Statement>,
}


#[derive(Debug, PartialEq, Clone)]
pub struct Validated {
    pub command: Ustr,
    pub expr: Rc<Expr>,
}


fn resolve_variables(expr: Rc<Expr>, vars: &UstrMap<Rc<Expr>>) -> Rc<Expr> {
    match expr.as_ref() {
        Expr::Literal(_) => Rc::clone(&expr),
        Expr::Variable(name) => {
            match vars.get(&name) {
                Some(replacement) => {
                    Rc::clone(&replacement)
                },
                None => {
                    Rc::clone(&expr)
                },
            }
        },
        Expr::Sequence(children) => {
            let mut new_children: Vec<Rc<Expr>> = Default::default();
            let mut any_child_replaced = false;
            for child in children {
                let new_child = resolve_variables(Rc::clone(child), vars);
                if !Rc::ptr_eq(&child, &new_child) {
                    any_child_replaced = true;
                }
                new_children.push(new_child);
            }
            if any_child_replaced {
                Rc::new(Expr::Sequence(new_children))
            } else {
                Rc::clone(&expr)
            }
        },
        Expr::Alternative(children) => {
            let mut new_children: Vec<Rc<Expr>> = Default::default();
            let mut any_child_replaced = false;
            for child in children {
                let new_child = resolve_variables(Rc::clone(child), vars);
                if !Rc::ptr_eq(&child, &new_child) {
                    any_child_replaced = true;
                }
                new_children.push(new_child);
            }
            if any_child_replaced {
                Rc::new(Expr::Alternative(new_children))
            } else {
                Rc::clone(&expr)
            }
        },
        Expr::Optional(child) => {
            let new_child = resolve_variables(Rc::clone(child), vars);
            if Rc::ptr_eq(&child, &new_child) {
                Rc::clone(&expr)
            }
            else {
                Rc::new(Expr::Optional(new_child))
            }
        },
        Expr::Many1(child) => {
            let new_child = resolve_variables(Rc::clone(child), vars);
            if Rc::ptr_eq(&child, &new_child) {
                Rc::clone(&expr)
            }
            else {
                Rc::new(Expr::Many1(new_child))
            }
        },
    }
}


fn get_topologically_ordered_variables(variable_definitions: &UstrMap<Rc<Expr>>) -> Result<Vec<Ustr>> {
    todo!();
}


impl Grammar {
    pub fn validate(&self) -> Result<Validated> {
        let command = {
            let mut commands: Vec<Ustr> = self.statements.iter().filter_map(|v|
                match v {
                    Statement::CallVariant { lhs, .. } => Some(*lhs),
                    Statement::VariableDefinition { .. } => None,
                }
            ).collect();

            if commands.is_empty() {
                return Err(Error::EmptyGrammar);
            }

            commands.sort_unstable();
            commands.dedup();

            if commands.len() > 1 {
                return Err(Error::VaryingCommandNames(
                    commands.into_iter().collect(),
                ));
            }
            commands[0]
        };

        let expr = {
            let call_variants: Vec<Rc<Expr>> = self.statements.iter().filter_map(|v|
                match v {
                    Statement::CallVariant { rhs, .. } => Some(rhs.clone()),
                    Statement::VariableDefinition { .. } => None,
                }
            ).collect();

            if call_variants.len() == 1 {
                Rc::clone(&call_variants[0])
            }
            else {
                Rc::new(Expr::Alternative(call_variants))
            }
        };

        let mut variable_definitions: UstrMap<Rc<Expr>> = self.statements.iter().filter_map(|v|
            match v {
                Statement::CallVariant { .. } => None,
                Statement::VariableDefinition { symbol, rhs } => Some((*symbol, Rc::clone(&rhs))),
            }
        ).collect();

        for variable in get_topologically_ordered_variables(&variable_definitions)? {
            let e = Rc::clone(variable_definitions.get(&variable).unwrap());
            *variable_definitions.get_mut(&variable).unwrap() = resolve_variables(e, &variable_definitions);
        }
        let expr = resolve_variables(expr, &variable_definitions);

        let g = Validated {
            command,
            expr,
        };
        Ok(g)

    }
}


pub fn parse(input: &str) -> Result<Grammar> {
    let (input, statements) = match grammar(input) {
        Ok((input, statements)) => (input, statements),
        Err(e) => return Err(Error::ParsingError(e.to_string())),
    };

    if !input.is_empty() {
        return Err(Error::TrailingInput(input.to_owned()));
    }

    let g = Grammar {
        statements,
    };

    Ok(g)
}


#[cfg(test)]
pub mod tests {
    use std::{rc::Rc, ops::Rem};
    use proptest::{strategy::BoxedStrategy, test_runner::TestRng};
    use proptest::prelude::*;
    use ustr::ustr as u;
    use Expr::*;

    use super::*;

    fn arb_literal(inputs: Rc<Vec<Ustr>>) -> BoxedStrategy<Rc<Expr>> {
        (0..inputs.len()).prop_map(move |index| Rc::new(Literal(ustr(&inputs[index])))).boxed()
    }

    fn arb_variable(variables: Rc<Vec<Ustr>>) -> BoxedStrategy<Rc<Expr>> {
        (0..variables.len()).prop_map(move |index| Rc::new(Variable(ustr(&variables[index])))).boxed()
    }

    fn arb_optional(inputs: Rc<Vec<Ustr>>, variables: Rc<Vec<Ustr>>, remaining_depth: usize, max_width: usize) -> BoxedStrategy<Rc<Expr>> {
        arb_expr(inputs, variables, remaining_depth-1, max_width).prop_map(|e| Rc::new(Optional(e))).boxed()
    }

    fn arb_many1(inputs: Rc<Vec<Ustr>>, variables: Rc<Vec<Ustr>>, remaining_depth: usize, max_width: usize) -> BoxedStrategy<Rc<Expr>> {
        arb_expr(inputs, variables, remaining_depth-1, max_width).prop_map(|e| Rc::new(Many1(e))).boxed()
    }

    fn arb_sequence(inputs: Rc<Vec<Ustr>>, variables: Rc<Vec<Ustr>>, remaining_depth: usize, max_width: usize) -> BoxedStrategy<Rc<Expr>> {
        (2..max_width).prop_flat_map(move |width| {
            let e = arb_expr(inputs.clone(), variables.clone(), remaining_depth-1, max_width);
            prop::collection::vec(e, width).prop_map(|v| Rc::new(Sequence(v)))
        }).boxed()
    }

    fn arb_alternative(inputs: Rc<Vec<Ustr>>, variables: Rc<Vec<Ustr>>, remaining_depth: usize, max_width: usize) -> BoxedStrategy<Rc<Expr>> {
        (2..max_width).prop_flat_map(move |width| {
            let e = arb_expr(inputs.clone(), variables.clone(), remaining_depth-1, max_width);
            prop::collection::vec(e, width).prop_map(|v| Rc::new(Alternative(v)))
        }).boxed()
    }

    pub fn arb_expr(inputs: Rc<Vec<Ustr>>, variables: Rc<Vec<Ustr>>, remaining_depth: usize, max_width: usize) -> BoxedStrategy<Rc<Expr>> {
        if remaining_depth <= 1 {
            prop_oneof![
                arb_literal(Rc::clone(&inputs)),
                arb_variable(variables),
            ].boxed()
        }
        else {
            prop_oneof![
                arb_literal(inputs.clone()),
                arb_variable(variables.clone()),
                arb_optional(inputs.clone(), variables.clone(), remaining_depth, max_width),
                arb_many1(inputs.clone(), variables.clone(), remaining_depth, max_width),
                arb_sequence(inputs.clone(), variables.clone(), remaining_depth, max_width),
                arb_alternative(inputs, variables, remaining_depth, max_width),
            ].boxed()
        }
    }

    pub fn do_arb_match(e: Rc<Expr>, rng: &mut TestRng, max_width: usize, output: &mut Vec<Ustr>) {
        match e.as_ref() {
            Literal(s) => output.push(*s),
            Variable(_) => output.push(ustr("anything")),
            Sequence(v) => {
                for subexpr in v {
                    do_arb_match(Rc::clone(&subexpr), rng, max_width, output);
                }
            },
            Alternative(v) => {
                let chosen_alternative = usize::try_from(rng.next_u64().rem(u64::try_from(v.len()).unwrap())).unwrap();
                do_arb_match(Rc::clone(&v[chosen_alternative]), rng, max_width, output);
            },
            Optional(subexpr) => {
                if rng.next_u64() % 2 == 0 {
                    do_arb_match(Rc::clone(&subexpr), rng, max_width, output);
                }
            },
            Many1(subexpr) => {
                let n = rng.next_u64();
                let chosen_len = n % u64::try_from(max_width).unwrap() + 1;
                for _ in 0..chosen_len {
                    do_arb_match(Rc::clone(&subexpr), rng, max_width, output);
                }
            },
        }
    }

    pub fn arb_match(e: Rc<Expr>, mut rng: TestRng, max_width: usize) -> (Rc<Expr>, Vec<Ustr>) {
        let mut output: Vec<Ustr> = Default::default();
        do_arb_match(Rc::clone(&e), &mut rng, max_width, &mut output);
        (e, output)
    }

    // Produce an arbitrary sequence matching `e`.
    pub fn arb_expr_match(inputs: Rc<Vec<Ustr>>, variables: Rc<Vec<Ustr>>, remaining_depth: usize, max_width: usize) -> BoxedStrategy<(Rc<Expr>, Vec<Ustr>)> {
        arb_expr(inputs, variables, remaining_depth, max_width).prop_perturb(move |e, rng| arb_match(e, rng, max_width)).boxed()
    }


    #[test]
    fn parses_word_terminal() {
        const INPUT: &str = r#"foo"#;
        let ("", e) = terminal_expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(e, Literal(u("foo")));
    }

    #[test]
    fn parses_short_option_terminal() {
        const INPUT: &str = r#"-f"#;
        let ("", e) = terminal_expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(e, Literal(u("-f")));
    }

    #[test]
    fn parses_long_option_terminal() {
        const INPUT: &str = r#"--foo"#;
        let ("", e) = terminal_expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(e, Literal(u("--foo")));
    }

    #[test]
    fn parses_symbol() {
        const INPUT: &str = "<FILE>";
        let ("", e) = symbol_expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(e, Variable(u("FILE")));
    }

    #[test]
    fn parses_optional_expr() {
        const INPUT: &str = "[<foo>]";
        let ("", e) = expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(e, Optional(Rc::new(Variable(u("foo")))));
    }

    #[test]
    fn parses_one_or_more_expr() {
        const INPUT: &str = "<foo>...";
        let ("", e) = expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(e, Many1(Rc::new(Variable(u("foo")))));
    }

    #[test]
    fn parses_sequence_expr() {
        const INPUT: &str = "<first-symbol> <second symbol>";
        let ("", e) = expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(
            e,
            Sequence(vec![
                Rc::new(Variable(u("first-symbol"))),
                Rc::new(Variable(u("second symbol"))),
            ])
        );
    }

    #[test]
    fn parses_alternative_expr() {
        const INPUT: &str = "a b | c";
        let ("", e) = expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(
            e,
            Alternative(vec![
                Rc::new(Sequence(vec![Rc::new(Literal(u("a"))), Rc::new(Literal(u("b")))])),
                Rc::new(Literal(u("c")))
            ])
        );
    }

    #[test]
    fn parses_parenthesised_expr() {
        const INPUT: &str = r#"a (b | c)"#;
        let ("", e) = expr(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(
            e,
            Sequence(vec![
                Rc::new(Literal(u("a"))),
                Rc::new(Alternative(vec![Rc::new(Literal(u("b"))), Rc::new(Literal(u("c")))])),
            ])
        );
    }

    #[test]
    fn parses_variant() {
        const INPUT: &str = r#"foo bar;"#;
        let ("", v) = call_variant(INPUT).unwrap() else { panic!("parsing error"); };
        assert_eq!(
            v,
            Statement::CallVariant {
                lhs: u("foo"),
                rhs: Rc::new(Literal(u("bar")))
            }
        );
    }

    #[test]
    fn parses_grammar() {
        const INPUT: &str = r#"
foo bar;
foo baz;
"#;
        let g = parse(INPUT).unwrap();
        assert_eq!(
            g,
            Grammar {
                statements: vec![
                    Statement::CallVariant { lhs: u("foo"), rhs: Rc::new(Literal(u("bar"))) },
                    Statement::CallVariant { lhs: u("foo"), rhs: Rc::new(Literal(u("baz"))) }
                ],
            }
        );
    }

    #[test]
    fn bug1() {
        // Did not consider whitespace before ...
        const INPUT: &str = "darcs help ( ( -v | --verbose ) | ( -q | --quiet ) ) ... [<DARCS_COMMAND> [DARCS_SUBCOMMAND]]  ;";
        let g = parse(INPUT).unwrap();
        assert_eq!(
            g,
            Grammar {
                statements: vec![
                    Statement::CallVariant { lhs: u("darcs"), rhs: Rc::new(Sequence(vec![
                    Rc::new(Literal(u("help"))),
                    Rc::new(Sequence(vec![
                        Rc::new(Many1(Rc::new(Alternative(vec![
                            Rc::new(Alternative(vec![Rc::new(Literal(u("-v"))), Rc::new(Literal(u("--verbose")))])),
                            Rc::new(Alternative(vec![Rc::new(Literal(u("-q"))), Rc::new(Literal(u("--quiet")))])),
                        ],)),)),
                        Rc::new(Optional(Rc::new(Sequence(vec![
                            Rc::new(Variable(u("DARCS_COMMAND"))),
                            Rc::new(Optional(Rc::new(Literal(u("DARCS_SUBCOMMAND"))))),
                        ])))),
                    ])),
                ])) },
                ],
            }
        );
    }

    #[test]
    fn parses_darcs_grammar() {
        // Source: https://github.com/mbrubeck/compleat/blob/56dd9761cdbb07de674947b129192cd8043cda8a/examples/darcs.usage
        const INPUT: &str = r#"
darcs help ( ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [<DARCS_COMMAND> [DARCS_SUBCOMMAND]]  ;
darcs add ( --boring | ( --case-ok | --reserved-ok ) | ( ( -r | --recursive ) | --not-recursive ) | ( --date-trick | --no-date-trick ) | --repodir <DIRECTORY> | --dry-run | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ( <FILE> | <DIRECTORY> )...;
darcs remove ( --repodir <DIRECTORY> | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ( <FILE> | <DIRECTORY> )...;
darcs move ( ( --case-ok | --reserved-ok ) | --repodir <DIRECTORY> | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... <SOURCE> ... <DESTINATION>;
darcs replace ( --token-chars <"[CHARS]"> | ( ( -f | --force ) | --no-force ) | --repodir <DIRECTORY> | --ignore-times | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... <OLD> <NEW> <FILE> ...;
darcs revert ( ( ( -a | --all ) | ( -i | --interactive ) ) | --repodir <DIRECTORY> | --ignore-times | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [ ( <FILE> | <DIRECTORY> ) ]...;
darcs unrevert ( --ignore-times | ( ( -a | --all ) | ( -i | --interactive ) ) | --repodir <DIRECTORY> | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs whatsnew ( ( ( -s | --summary ) | --no-summary ) | ( -u | --unified ) | ( ( -l | --look-for-adds ) | --dont-look-for-adds ) | --repodir <DIRECTORY> | --ignore-times | --boring | --no-cache | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [ ( <FILE> | <DIRECTORY> ) ]...;
darcs record ( ( -m <PATCHNAME> | --patch-name <PATCHNAME> ) | ( -A <EMAIL> | --author <EMAIL> ) | ( --no-test | --test ) | ( --leave-test-directory | --remove-test-directory ) | ( ( -a | --all ) | --pipe | ( -i | --interactive ) ) | ( --ask-deps | --no-ask-deps ) | ( --edit-long-comment | --skip-long-comment | --prompt-long-comment ) | ( ( -l | --look-for-adds ) | --dont-look-for-adds ) | --repodir <DIRECTORY> | --logfile <FILE> | --delete-logfile | ( --compress | --dont-compress ) | --ignore-times | --umask <UMASK> | ( --set-scripts-executable | --dont-set-scripts-executable ) | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [ ( <FILE> | <DIRECTORY> ) ]...;
darcs unrecord ( ( --from-match <PATTERN> | --from-patch <REGEXP> | --from-tag <REGEXP> | --last <NUMBER> | --matches <PATTERN> | ( -p <REGEXP> | --patches <REGEXP> ) | ( -t <REGEXP> | --tags <REGEXP> ) ) | ( --no-deps | --dont-prompt-for-dependencies | --prompt-for-dependencies ) | ( ( -a | --all ) | ( -i | --interactive ) ) | --repodir <DIRECTORY> | ( --compress | --dont-compress ) | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs amend-record ( ( --match <PATTERN> | ( -p <REGEXP> | --patch <REGEXP> ) | ( -n <N> | --index <N> ) ) | ( --no-test | --test ) | ( --leave-test-directory | --remove-test-directory ) | ( ( -a | --all ) | ( -i | --interactive ) ) | ( -A <EMAIL> | --author <EMAIL> ) | ( -m <PATCHNAME> | --patch-name <PATCHNAME> ) | ( --edit-long-comment | --skip-long-comment | --prompt-long-comment ) | ( ( -l | --look-for-adds ) | --dont-look-for-adds ) | --repodir <DIRECTORY> | ( --compress | --dont-compress ) | --ignore-times | --umask <UMASK> | ( --set-scripts-executable | --dont-set-scripts-executable ) | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [ ( <FILE> | <DIRECTORY> ) ]...;
darcs mark-conflicts ( --ignore-times | --repodir <DIRECTORY> | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs tag ( ( -m <PATCHNAME> | --patch-name <PATCHNAME> ) | ( -A <EMAIL> | --author <EMAIL> ) | ( --pipe | ( -i | --interactive ) ) | ( --edit-long-comment | --skip-long-comment | --prompt-long-comment ) | --repodir <DIRECTORY> | ( --compress | --dont-compress ) | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [TAGNAME];
darcs setpref ( --repodir <DIRECTORY> | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... <PREF> <VALUE>;
darcs diff ( ( --to-match <PATTERN> | --to-patch <REGEXP> | --to-tag <REGEXP> | --from-match <PATTERN> | --from-patch <REGEXP> | --from-tag <REGEXP> | --match <PATTERN> | ( -p <REGEXP> | --patch <REGEXP> ) | --last <NUMBER> | ( -n <N-M> | --index <N-M> ) ) | --diff-command <COMMAND> | --diff-opts <OPTIONS> | ( -u | --unified ) | --repodir <DIRECTORY> | --store-in-memory | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [ ( <FILE> | <DIRECTORY> ) ]...;
darcs changes ( ( --to-match <PATTERN> | --to-patch <REGEXP> | --to-tag <REGEXP> | --from-match <PATTERN> | --from-patch <REGEXP> | --from-tag <REGEXP> | --last <NUMBER> | ( -n <N-M> | --index <N-M> ) | --matches <PATTERN> | ( -p <REGEXP> | --patches <REGEXP> ) | ( -t <REGEXP> | --tags <REGEXP> ) ) | --max-count <NUMBER> | --only-to-files | ( --context | --xml-output | --human-readable | --number | --count ) | ( ( -s | --summary ) | --no-summary ) | --reverse | --repo <URL> | --repodir <DIRECTORY> | ( ( -a | --all ) | ( -i | --interactive ) ) | ( --ssh-cm | --no-ssh-cm ) | ( --http-pipelining | --no-http-pipelining ) | --no-cache | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [ ( <FILE> | <DIRECTORY> ) ]...;
darcs annotate ( ( ( -s | --summary ) | --no-summary ) | ( -u | --unified ) | --human-readable | --xml-output | ( --match <PATTERN> | ( -p <REGEXP> | --patch <REGEXP> ) | ( -t <REGEXP> | --tag <REGEXP> ) | ( -n <N> | --index <N> ) ) | --creator-hash <HASH> | --repodir <DIRECTORY> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [ ( <FILE> | <DIRECTORY> ) ]...;
darcs dist ( ( -d <DISTNAME> | --dist-name <DISTNAME> ) | --repodir <DIRECTORY> | ( --match <PATTERN> | ( -p <REGEXP> | --patch <REGEXP> ) | ( -t <REGEXP> | --tag <REGEXP> ) | ( -n <N> | --index <N> ) ) | --store-in-memory | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs trackdown ( --repodir <DIRECTORY> | ( --set-scripts-executable | --dont-set-scripts-executable ) | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [[INITIALIZATION] COMMAND];
darcs show ( contents ( ( --match <PATTERN> | ( -p <REGEXP> | --patch <REGEXP> ) | ( -t <REGEXP> | --tag <REGEXP> ) | ( -n <N> | --index <N> ) ) | --repodir <DIRECTORY> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [FILE]... | files ( ( --files | --no-files ) | ( --directories | --no-directories ) | ( --pending | --no-pending ) | ( -0 | --null ) | --repodir <DIRECTORY> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ...  | index ( ( --files | --no-files ) | ( --directories | --no-directories ) | ( -0 | --null ) | --repodir <DIRECTORY> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ...  | pristine ( ( --files | --no-files ) | ( --directories | --no-directories ) | ( -0 | --null ) | --repodir <DIRECTORY> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ...  | repo ( --repodir <DIRECTORY> | ( --files | --no-files ) | --xml-output | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ...  | authors ( --repodir <DIRECTORY> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ...  | tags ( --repodir <DIRECTORY> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ...  );
darcs pull ( ( --matches <PATTERN> | ( -p <REGEXP> | --patches <REGEXP> ) | ( -t <REGEXP> | --tags <REGEXP> ) ) | ( ( -a | --all ) | ( -i | --interactive ) ) | ( --mark-conflicts | --allow-conflicts | --dont-allow-conflicts | --skip-conflicts ) | --external-merge <COMMAND> | ( --test | --no-test ) | --dry-run | --xml-output | ( ( -s | --summary ) | --no-summary ) | ( --no-deps | --dont-prompt-for-dependencies | --prompt-for-dependencies ) | ( --set-default | --no-set-default ) | --repodir <DIRECTORY> | --ignore-unrelated-repos | ( --intersection | --union | --complement ) | ( --compress | --dont-compress ) | --nolinks | --ignore-times | --remote-repo <URL> | ( --set-scripts-executable | --dont-set-scripts-executable ) | --umask <UMASK> | ( --restrict-paths | --dont-restrict-paths ) | ( --ssh-cm | --no-ssh-cm ) | ( --http-pipelining | --no-http-pipelining ) | --no-cache | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [REPOSITORY]...;
darcs obliterate ( ( --from-match <PATTERN> | --from-patch <REGEXP> | --from-tag <REGEXP> | --last <NUMBER> | --matches <PATTERN> | ( -p <REGEXP> | --patches <REGEXP> ) | ( -t <REGEXP> | --tags <REGEXP> ) ) | ( --no-deps | --dont-prompt-for-dependencies | --prompt-for-dependencies ) | ( ( -a | --all ) | ( -i | --interactive ) ) | --repodir <DIRECTORY> | ( ( -s | --summary ) | --no-summary ) | ( --compress | --dont-compress ) | --ignore-times | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs rollback ( ( --from-match <PATTERN> | --from-patch <REGEXP> | --from-tag <REGEXP> | --last <NUMBER> | --matches <PATTERN> | ( -p <REGEXP> | --patches <REGEXP> ) | ( -t <REGEXP> | --tags <REGEXP> ) ) | ( ( -a | --all ) | ( -i | --interactive ) ) | ( -A <EMAIL> | --author <EMAIL> ) | ( -m <PATCHNAME> | --patch-name <PATCHNAME> ) | ( --edit-long-comment | --skip-long-comment | --prompt-long-comment ) | ( --no-test | --test ) | ( --leave-test-directory | --remove-test-directory ) | --repodir <DIRECTORY> | ( --compress | --dont-compress ) | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [ ( <FILE> | <DIRECTORY> ) ]...;
darcs push ( ( --matches <PATTERN> | ( -p <REGEXP> | --patches <REGEXP> ) | ( -t <REGEXP> | --tags <REGEXP> ) ) | ( --no-deps | --dont-prompt-for-dependencies | --prompt-for-dependencies ) | ( ( -a | --all ) | ( -i | --interactive ) ) | ( --sign | --sign-as <KEYID> | --sign-ssl <IDFILE> | --dont-sign ) | --dry-run | --xml-output | ( ( -s | --summary ) | --no-summary ) | --repodir <DIRECTORY> | ( --set-default | --no-set-default ) | --ignore-unrelated-repos | ( --apply-as <USERNAME> | --apply-as-myself ) | --nolinks | --remote-repo <URL> | ( --ssh-cm | --no-ssh-cm ) | ( --http-pipelining | --no-http-pipelining ) | --no-cache | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [REPOSITORY];
darcs send ( ( --matches <PATTERN> | ( -p <REGEXP> | --patches <REGEXP> ) | ( -t <REGEXP> | --tags <REGEXP> ) ) | ( --no-deps | --dont-prompt-for-dependencies | --prompt-for-dependencies ) | ( ( -a | --all ) | ( -i | --interactive ) ) | --from <EMAIL> | ( -A <EMAIL> | --author <EMAIL> ) | --to <EMAIL> | --cc <EMAIL> | --subject <SUBJECT> | --in-reply-to <EMAIL> | ( -o <FILE> | --output <FILE> ) | ( -O [<DIRECTORY>] | --output-auto-name [<DIRECTORY>] ) | ( --sign | --sign-as <KEYID> | --sign-ssl <IDFILE> | --dont-sign ) | --dry-run | --xml-output | ( ( -s | --summary ) | --no-summary ) | ( --edit-description | --dont-edit-description ) | ( --set-default | --no-set-default ) | --repodir <DIRECTORY> | --sendmail-command <COMMAND> | --ignore-unrelated-repos | --logfile <FILE> | --delete-logfile | --remote-repo <URL> | --context <FILENAME> | ( --ssh-cm | --no-ssh-cm ) | ( --http-pipelining | --no-http-pipelining ) | --no-cache | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... [REPOSITORY];
darcs apply ( ( --verify <PUBRING> | --verify-ssl <KEYS> | --no-verify ) | ( ( -a | --all ) | ( -i | --interactive ) ) | --dry-run | --xml-output | ( --mark-conflicts | --allow-conflicts | --no-resolve-conflicts | --dont-allow-conflicts | --skip-conflicts ) | --external-merge <COMMAND> | ( --no-test | --test ) | ( --leave-test-directory | --remove-test-directory ) | --repodir <DIRECTORY> | --reply <FROM> | --cc <EMAIL> | --happy-forwarding | --sendmail-command <COMMAND> | --ignore-times | ( --compress | --dont-compress ) | ( --set-scripts-executable | --dont-set-scripts-executable ) | --umask <UMASK> | ( --restrict-paths | --dont-restrict-paths ) | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... <PATCHFILE>;
darcs get ( ( --repo-name <DIRECTORY> | --repodir <DIRECTORY> ) | ( --partial | --lazy | --ephemeral | --complete ) | ( --to-match <PATTERN> | --to-patch <REGEXP> | ( -t <REGEXP> | --tag <REGEXP> ) | --context <FILENAME> ) | ( --set-default | --no-set-default ) | ( --set-scripts-executable | --dont-set-scripts-executable ) | --nolinks | ( --hashed | --old-fashioned-inventory ) | ( --ssh-cm | --no-ssh-cm ) | ( --http-pipelining | --no-http-pipelining ) | --no-cache | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... <REPOSITORY> [<DIRECTORY>];
darcs put ( ( --to-match <PATTERN> | --to-patch <REGEXP> | ( -t <REGEXP> | --tag <REGEXP> ) | --context <FILENAME> ) | ( --set-scripts-executable | --dont-set-scripts-executable ) | ( --hashed | --old-fashioned-inventory ) | ( --set-default | --no-set-default ) | --repodir <DIRECTORY> | ( --apply-as <USERNAME> | --apply-as-myself ) | ( --ssh-cm | --no-ssh-cm ) | ( --http-pipelining | --no-http-pipelining ) | --no-cache | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... <NEW_REPOSITORY>;
darcs initialize ( ( --hashed | --darcs-2 | --old-fashioned-inventory ) | --repodir <DIRECTORY> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs optimize ( --repodir <DIRECTORY> | --reorder-patches | --sibling <URL> | --relink | --relink-pristine | --upgrade | --pristine | ( --compress | --dont-compress | --uncompress ) | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs check ( ( --complete | --partial ) | ( --no-test | --test ) | ( --leave-test-directory | --remove-test-directory ) | --repodir <DIRECTORY> | --ignore-times | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs repair ( --repodir <DIRECTORY> | --umask <UMASK> | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... ;
darcs convert ( ( --repo-name <DIRECTORY> | --repodir <DIRECTORY> ) | ( --set-scripts-executable | --dont-set-scripts-executable ) | ( --ssh-cm | --no-ssh-cm ) | ( --http-pipelining | --no-http-pipelining ) | --no-cache | ( --debug | --debug-verbose | --debug-http | ( -v | --verbose ) | ( -q | --quiet ) | --standard-verbosity ) | --timings | ( --posthook <COMMAND> | --no-posthook ) | ( --prompt-posthook | --run-posthook ) | ( --prehook <COMMAND> | --no-prehook ) | ( --prompt-prehook | --run-prehook ) ) ... <SOURCE> [<DESTINATION>];
"#;
        let _ = parse(INPUT).unwrap();
    }

    #[test]
    fn parses_variable_definition() {
        const INPUT: &str = r#"
grep [<OPTION>]... <PATTERNS> [<FILE>]...;
<OPTION> ::= --color <WHEN>;
<WHEN> ::= always | never | auto;
"#;
        let g = parse(INPUT).unwrap();
        assert_eq!(
            g,
            Grammar {
                statements: vec![
                    Statement::CallVariant { lhs: u("grep"), rhs: Rc::new(Sequence(vec![Rc::new(Many1(Rc::new(Optional(Rc::new(Variable(ustr("OPTION"))))))), Rc::new(Sequence(vec![Rc::new(Variable(ustr("PATTERNS"))), Rc::new(Many1(Rc::new(Optional(Rc::new(Variable(ustr("FILE")))))))]))])) },
                    Statement::VariableDefinition { symbol: u("OPTION"), rhs: Rc::new(Sequence(vec![Rc::new(Literal(ustr("--color"))), Rc::new(Variable(ustr("WHEN")))])) },
                    Statement::VariableDefinition { symbol: u("WHEN"), rhs: Rc::new(Alternative(vec![Rc::new(Literal(ustr("always"))), Rc::new(Literal(ustr("never"))), Rc::new(Literal(ustr("auto")))])) },
                ],
            }
        );
    }
}
