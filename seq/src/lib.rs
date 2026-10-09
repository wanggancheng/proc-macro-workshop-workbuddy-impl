use proc_macro::TokenStream;
use proc_macro2::{Delimiter, Group, Ident, Literal, TokenStream as TokenStream2, TokenTree};
use syn::{braced, parse::Parse, parse::ParseStream, parse_macro_input, LitInt, Result, Token};

#[proc_macro]
pub fn seq(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as SeqInput);

    expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct SeqInput {
    var: Ident,
    start: LitInt,
    end: LitInt,
    inclusive: bool,
    body: TokenStream2,
}

impl Parse for SeqInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let var: Ident = input.parse()?;
        let _: Token![in] = input.parse()?;
        let start: LitInt = input.parse()?;
        let inclusive = if input.peek(Token![..=]) {
            let _: Token![..=] = input.parse()?;
            true
        } else {
            let _: Token![..] = input.parse()?;
            false
        };
        let end: LitInt = input.parse()?;

        let content;
        braced!(content in input);
        let body: TokenStream2 = content.parse()?;

        Ok(SeqInput {
            var,
            start,
            end,
            inclusive,
            body,
        })
    }
}

fn expand(input: SeqInput) -> Result<TokenStream2> {
    let start = input.start.base10_parse::<u64>()?;
    let end = input.end.base10_parse::<u64>()?;
    let var = input.var;

    let last = if input.inclusive {
        Some(end)
    } else {
        end.checked_sub(1)
    };

    let indices: Vec<u64> = match last {
        Some(last) if start <= last => (start..=last).collect(),
        _ => Vec::new(),
    };

    // If the caller did not mark any `#(...)*` section anywhere in the body,
    // the entire body is stamped out once per iteration. Otherwise the body is
    // emitted once and only the marked sections repeat.
    if !contains_repeat(&input.body) {
        let mut out = TokenStream2::new();
        for &n in &indices {
            out.extend(substitute(input.body.clone(), &var, n));
        }
        return Ok(out);
    }

    Ok(expand_once(&input.body, &var, &indices))
}

/// Emit a token stream exactly once, expanding any `#(...)*` sections found
/// within it (at any nesting depth) across all loop iterations.
fn expand_once(tokens: &TokenStream2, var: &Ident, indices: &[u64]) -> TokenStream2 {
    let items: Vec<TokenTree> = tokens.clone().into_iter().collect();
    let mut out = TokenStream2::new();

    let mut i = 0;
    while i < items.len() {
        // `#` `( ... )` `*`
        if let TokenTree::Punct(punct) = &items[i] {
            if punct.as_char() == '#' {
                if let (Some(TokenTree::Group(group)), Some(TokenTree::Punct(star))) =
                    (items.get(i + 1), items.get(i + 2))
                {
                    if group.delimiter() == Delimiter::Parenthesis && star.as_char() == '*' {
                        for &n in indices {
                            out.extend(substitute(group.stream(), var, n));
                        }
                        i += 3;
                        continue;
                    }
                }
            }
        }

        match &items[i] {
            TokenTree::Group(group) if contains_repeat(&group.stream()) => {
                let inner = expand_once(&group.stream(), var, indices);
                let mut new_group = Group::new(group.delimiter(), inner);
                new_group.set_span(group.span());
                out.extend([TokenTree::Group(new_group)]);
            }
            other => out.extend([other.clone()]),
        }
        i += 1;
    }

    out
}

/// Whether a token stream contains a `#(...)*` section at any nesting depth.
fn contains_repeat(tokens: &TokenStream2) -> bool {
    let items: Vec<TokenTree> = tokens.clone().into_iter().collect();
    let mut i = 0;
    while i < items.len() {
        if let TokenTree::Punct(punct) = &items[i] {
            if punct.as_char() == '#' {
                if let (Some(TokenTree::Group(group)), Some(TokenTree::Punct(star))) =
                    (items.get(i + 1), items.get(i + 2))
                {
                    if group.delimiter() == Delimiter::Parenthesis && star.as_char() == '*' {
                        return true;
                    }
                }
            }
        }
        if let TokenTree::Group(group) = &items[i] {
            if contains_repeat(&group.stream()) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Replace the loop variable in a chunk of tokens with the given index,
/// including pasting `prefix~N` into a single identifier. The pasted
/// identifier carries the span of the prefix so that compiler errors point at
/// the place the caller wrote it.
fn substitute(tokens: TokenStream2, var: &Ident, n: u64) -> TokenStream2 {
    let items: Vec<TokenTree> = tokens.into_iter().collect();
    let mut out = TokenStream2::new();

    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            TokenTree::Ident(ident) => {
                if let (Some(TokenTree::Punct(tilde)), Some(TokenTree::Ident(loop_var))) =
                    (items.get(i + 1), items.get(i + 2))
                {
                    if tilde.as_char() == '~' && loop_var == var {
                        let name = format!("{}{}", ident, n);
                        out.extend([TokenTree::Ident(Ident::new(&name, ident.span()))]);
                        i += 3;
                        continue;
                    }
                }
                if ident == var {
                    out.extend([TokenTree::Literal(Literal::u64_unsuffixed(n))]);
                    i += 1;
                    continue;
                }
                out.extend([items[i].clone()]);
            }
            TokenTree::Group(group) => {
                let inner = substitute(group.stream(), var, n);
                let mut new_group = Group::new(group.delimiter(), inner);
                new_group.set_span(group.span());
                out.extend([TokenTree::Group(new_group)]);
            }
            other => out.extend([other.clone()]),
        }
        i += 1;
    }

    out
}
