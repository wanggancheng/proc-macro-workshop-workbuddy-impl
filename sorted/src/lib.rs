use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, ToTokens};
use syn::{
    parse_macro_input, visit_mut::VisitMut, Arm, ExprMatch, Item, ItemEnum, ItemFn, Pat, Path,
};

#[proc_macro_attribute]
pub fn sorted(args: TokenStream, input: TokenStream) -> TokenStream {
    let _ = args;

    let tokens: TokenStream2 = input.clone().into();

    let item: Item = match syn::parse2(tokens.clone()) {
        Ok(item) => item,
        Err(error) => return error.to_compile_error().into(),
    };

    let result = match &item {
        Item::Enum(item_enum) => check_enum(item_enum),
        _ => Err(not_supported()),
    };

    match result {
        Ok(()) => input,
        Err(error) => {
            // When the caller handed us an enum, keep the original item in the
            // output alongside the error so that the rest of the crate keeps
            // compiling and does not suddenly grow unused/dead-code warnings.
            if matches!(item, Item::Enum(_)) {
                let mut tokens = tokens;
                tokens.extend(error.to_compile_error());
                tokens.into()
            } else {
                error.to_compile_error().into()
            }
        }
    }
}

#[proc_macro_attribute]
pub fn check(args: TokenStream, input: TokenStream) -> TokenStream {
    let _ = args;

    let mut item = parse_macro_input!(input as ItemFn);

    let mut checker = Checker { error: None };
    checker.visit_item_fn_mut(&mut item);

    let mut tokens = item.to_token_stream();
    if let Some(error) = checker.error {
        tokens.extend(error.to_compile_error());
    }
    tokens.into()
}

fn not_supported() -> syn::Error {
    syn::Error::new(Span::call_site(), "expected enum or match expression")
}

fn check_enum(item: &ItemEnum) -> syn::Result<()> {
    let entries = item
        .variants
        .iter()
        .map(|variant| Entry::Name {
            name: variant.ident.to_string(),
            path: Path::from(variant.ident.clone()),
        })
        .collect::<Vec<_>>();
    check_order(&entries)
}

/// One thing that must appear in ascending position order.
enum Entry {
    /// A path-like pattern such as an enum variant or `Error::Io`.
    Name { name: String, path: Path },
    /// A `_` wildcard pattern, which must come last.
    Wildcard,
}

struct Checker {
    error: Option<syn::Error>,
}

impl VisitMut for Checker {
    fn visit_expr_match_mut(&mut self, node: &mut ExprMatch) {
        let has_sorted = node.attrs.iter().any(|attr| attr.path().is_ident("sorted"));

        if has_sorted {
            // Strip the inert attribute so that the stable compiler does not
            // reject an attribute written on an expression.
            node.attrs.retain(|attr| !attr.path().is_ident("sorted"));

            if self.error.is_none() {
                if let Err(error) = check_arms(&node.arms) {
                    self.error = Some(error);
                }
            }
        }

        syn::visit_mut::visit_expr_match_mut(self, node);
    }
}

fn check_arms(arms: &[Arm]) -> syn::Result<()> {
    let mut entries = Vec::new();
    for arm in arms {
        entries.push(classify(&arm.pat)?);
    }
    check_order(&entries)
}

/// Ensure that every entry is greater than all the entries before it, and that
/// a wildcard appears last.
fn check_order(entries: &[Entry]) -> syn::Result<()> {
    for (i, entry) in entries.iter().enumerate() {
        let (name, path) = match entry {
            Entry::Name { name, path } => (name, path),
            Entry::Wildcard => continue,
        };

        for earlier in &entries[..i] {
            match earlier {
                Entry::Name {
                    name: earlier_name,
                    path: earlier_path,
                } => {
                    if name < earlier_name {
                        return Err(syn::Error::new_spanned(
                            path,
                            format!(
                                "{} should sort before {}",
                                display_path(path),
                                display_path(earlier_path),
                            ),
                        ));
                    }
                }
                Entry::Wildcard => {
                    return Err(syn::Error::new_spanned(
                        path,
                        format!(
                            "{} should sort before the wildcard pattern",
                            display_path(path),
                        ),
                    ));
                }
            }
        }
    }

    Ok(())
}

fn classify(pat: &Pat) -> syn::Result<Entry> {
    match pat {
        Pat::Path(pat) => Ok(to_entry(pat.path.clone())),
        Pat::TupleStruct(pat) => Ok(to_entry(pat.path.clone())),
        Pat::Struct(pat) => Ok(to_entry(pat.path.clone())),
        Pat::Ident(pat) => Ok(to_entry(Path::from(pat.ident.clone()))),
        Pat::Wild(_) => Ok(Entry::Wildcard),
        other => Err(syn::Error::new_spanned(other, "unsupported by #[sorted]")),
    }
}

fn to_entry(path: Path) -> Entry {
    let name = path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_default();
    Entry::Name { name, path }
}

/// Rendering of a path without the spaces that `TokenStream::to_string` adds.
fn display_path(path: &Path) -> String {
    quote!(#path).to_string().replace(' ', "")
}
