use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::{quote, ToTokens};
use std::collections::BTreeSet;
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, parse_quote, Data, DeriveInput, Fields, Ident, Lit, LitStr, Meta, Type,
    WherePredicate,
};

#[proc_macro_derive(CustomDebug, attributes(debug))]
pub fn derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct FieldInfo<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    format: Option<LitStr>,
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "derive(CustomDebug) only supports structs with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "derive(CustomDebug) only supports structs with named fields",
            ))
        }
    };

    let type_params: Vec<Ident> = input
        .generics
        .type_params()
        .map(|param| param.ident.clone())
        .collect();

    // Collect per-field info, including the custom format string if any.
    let mut infos = Vec::new();
    for field in fields {
        let field_ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "expected named field"))?;
        let mut format = None;
        for attr in &field.attrs {
            if attr.path().is_ident("debug") {
                format = Some(parse_format(&attr.meta)?);
            }
        }
        infos.push(FieldInfo {
            ident: field_ident,
            ty: &field.ty,
            format,
        });
    }

    // Trait bounds: either taken verbatim from `#[debug(bound = "...")]`, or
    // inferred from how each type parameter is used in the field types.
    let explicit_bounds = parse_bounds_attr(input)?;

    let mut generics = input.generics.clone();
    if let Some(predicates) = explicit_bounds {
        if !predicates.is_empty() {
            let where_clause = generics.make_where_clause();
            for predicate in predicates {
                where_clause.predicates.push(predicate);
            }
        }
    } else {
        let inferred = infer_bounds(&infos, &type_params);
        if !inferred.is_empty() {
            let where_clause = generics.make_where_clause();
            for predicate in inferred {
                where_clause.predicates.push(predicate);
            }
        }
    }

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let body = infos.iter().map(|info| {
        let name = info.ident;
        let name_str = name.to_string();
        match &info.format {
            Some(format) => quote! {
                .field(#name_str, &::std::format_args!(#format, &self.#name))
            },
            None => quote! {
                .field(#name_str, &self.#name)
            },
        }
    });

    let expanded = quote! {
        impl #impl_generics ::std::fmt::Debug for #ident #ty_generics #where_clause {
            fn fmt(
                &self,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                f.debug_struct(stringify!(#ident))
                    #(#body)*
                    .finish()
            }
        }
    };

    Ok(expanded)
}

/// Parse `#[debug = "..."]` on a field into the format string.
fn parse_format(meta: &Meta) -> syn::Result<LitStr> {
    if let Meta::NameValue(name_value) = meta {
        if let syn::Expr::Lit(expr_lit) = &name_value.value {
            if let Lit::Str(lit) = &expr_lit.lit {
                return Ok(lit.clone());
            }
        }
    }
    Err(syn::Error::new_spanned(
        meta,
        "expected `#[debug = \"...\"]`",
    ))
}

/// Parse a struct-level `#[debug(bound = "...")]` attribute, if present.
fn parse_bounds_attr(input: &DeriveInput) -> syn::Result<Option<Vec<WherePredicate>>> {
    let mut result = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("debug") {
            continue;
        }
        let mut collected = Vec::new();
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("bound") {
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                let predicates = lit.parse_with(
                    Punctuated::<WherePredicate, syn::Token![,]>::parse_terminated,
                )?;
                collected.extend(predicates);
                Ok(())
            } else {
                Err(meta.error("expected `debug(bound = \"...\")`"))
            }
        })?;
        result = Some(collected);
    }
    Ok(result)
}

/// Infer `T: Debug` style predicates from the way each type parameter is
/// mentioned in the field types.
fn infer_bounds(infos: &[FieldInfo], params: &[Ident]) -> Vec<WherePredicate> {
    // For each type parameter, the set of associated type paths used and
    // whether the parameter itself is mentioned directly.
    let mut direct: BTreeSet<String> = BTreeSet::new();
    let mut associated: BTreeSet<String> = BTreeSet::new();

    for info in infos {
        scan_usage(info.ty, params, &mut direct, &mut associated);
    }

    let mut predicates = Vec::new();
    for param in params {
        let key = param.to_string();
        if direct.contains(&key) {
            predicates.push(parse_quote!(#param: ::std::fmt::Debug));
        } else {
            for path in &associated {
                if let Some(rest) = path.strip_prefix(&format!("{}::", key)) {
                    if rest.is_empty() {
                        continue;
                    }
                    let assoc: syn::Path = match syn::parse_str(path) {
                        Ok(path) => path,
                        Err(_) => continue,
                    };
                    predicates.push(parse_quote!(#assoc: ::std::fmt::Debug));
                }
            }
        }
    }
    predicates
}

/// Walk a type's tokens recording how each type parameter is used.
///
/// * A parameter mentioned as a bare `T` is recorded in `direct`.
/// * A parameter mentioned as `T::Assoc` is recorded in `associated` as
///   `T::Assoc`.
/// * A parameter mentioned only inside `PhantomData<...>` is ignored, because
///   `PhantomData<T>` implements `Debug` for every `T`.
fn scan_usage(
    ty: &Type,
    params: &[Ident],
    direct: &mut BTreeSet<String>,
    associated: &mut BTreeSet<String>,
) {
    let mut tokens = Vec::new();
    flatten(ty.to_token_stream(), &mut tokens);

    // For each currently open `<`, whether it belongs to a `PhantomData<...>`.
    let mut phantom_stack: Vec<bool> = Vec::new();
    let mut phantom_pending = false;

    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            TokenTree::Ident(ident) => {
                let inside_phantom = phantom_stack.iter().any(|&inside| inside);
                if !inside_phantom && params.iter().any(|param| param == ident) {
                    let is_path_sep = matches!(
                        (tokens.get(i + 1), tokens.get(i + 2)),
                        (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
                            if a.as_char() == ':' && b.as_char() == ':'
                    );
                    if is_path_sep {
                        if let Some(TokenTree::Ident(assoc)) = tokens.get(i + 3) {
                            associated.insert(format!("{}::{}", ident, assoc));
                            i += 4;
                            phantom_pending = false;
                            continue;
                        }
                    }
                    direct.insert(ident.to_string());
                }
                phantom_pending = ident == "PhantomData";
            }
            TokenTree::Punct(punct) if punct.as_char() == '<' => {
                phantom_stack.push(phantom_pending);
                phantom_pending = false;
            }
            TokenTree::Punct(punct) if punct.as_char() == '>' => {
                phantom_stack.pop();
                phantom_pending = false;
            }
            _ => phantom_pending = false,
        }
        i += 1;
    }
}

/// Flatten a token stream, recursing into delimited groups.
fn flatten(tokens: TokenStream2, out: &mut Vec<TokenTree>) {
    for token in tokens {
        match token {
            TokenTree::Group(group) => flatten(group.stream(), out),
            other => out.push(other),
        }
    }
}
