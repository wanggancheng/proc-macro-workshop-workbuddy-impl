use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, spanned::Spanned, Data, DeriveInput, Fields, GenericArgument, Meta,
    PathArguments, Type,
};

#[proc_macro_derive(Builder, attributes(builder))]
pub fn derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// What kind of builder setter(s) a field gets.
enum Kind {
    /// `field: T` -- required, set via `fn field(&mut self, value: T)`.
    Plain,
    /// `field: Option<T>` -- optional, set via `fn field(&mut self, value: T)`.
    Optional(Type),
    /// `#[builder(each = "item")] field: Vec<T>` -- set via
    /// `fn item(&mut self, value: T)`, which appends to the vector.
    Repeated { each: syn::Ident, elem: Type },
}

struct FieldInfo {
    ident: syn::Ident,
    ty: Type,
    kind: Kind,
    /// Whether the field's type is literally written `Option<...>`.
    optional_storage: bool,
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let builder_ident = format_ident!("{}Builder", ident);

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "derive(Builder) only supports structs with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "derive(Builder) only supports structs with named fields",
            ))
        }
    };

    let mut infos = Vec::new();

    for field in fields {
        let field_ident = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new_spanned(field, "expected named field"))?;
        let ty = field.ty.clone();

        // Look for `#[builder(each = "...")]`.
        let mut each = None;
        for attr in &field.attrs {
            if attr.path().is_ident("builder") {
                if let Some(name) = parse_each(&attr.meta)? {
                    each = Some(name);
                }
            }
        }

        let (kind, optional_storage) = if let Some(inner) = option_inner(&ty) {
            (Kind::Optional(inner.clone()), true)
        } else if let Some(each) = each {
            let elem = vec_elem(&ty).ok_or_else(|| {
                syn::Error::new_spanned(
                    &ty,
                    "field with #[builder(each = \"...\")] must have type Vec<...>",
                )
            })?;
            (
                Kind::Repeated {
                    each,
                    elem: elem.clone(),
                },
                false,
            )
        } else {
            (Kind::Plain, false)
        };

        infos.push(FieldInfo {
            ident: field_ident,
            ty,
            kind,
            optional_storage,
        });
    }

    // --- builder struct declaration -----------------------------------------
    let decls = infos.iter().map(|info| {
        let name = &info.ident;
        let ty = &info.ty;
        if info.optional_storage {
            quote! { #name: #ty }
        } else {
            quote! { #name: ::std::option::Option<#ty> }
        }
    });

    // --- builder() constructor ----------------------------------------------
    let inits = infos.iter().map(|info| {
        let name = &info.ident;
        quote! { #name: ::std::option::Option::None }
    });

    // --- setter methods ------------------------------------------------------
    let mut setters = TokenStream2::new();
    for info in &infos {
        let name = &info.ident;
        match &info.kind {
            Kind::Plain => {
                let ty = &info.ty;
                setters.extend(quote! {
                    pub fn #name(&mut self, value: #ty) -> &mut Self {
                        self.#name = ::std::option::Option::Some(value);
                        self
                    }
                });
            }
            Kind::Optional(inner) => {
                setters.extend(quote! {
                    pub fn #name(&mut self, value: #inner) -> &mut Self {
                        self.#name = ::std::option::Option::Some(value);
                        self
                    }
                });
            }
            Kind::Repeated { each, elem } => {
                // The one-at-a-time setter.
                setters.extend(quote! {
                    pub fn #each(&mut self, value: #elem) -> &mut Self {
                        self.#name
                            .get_or_insert_with(::std::vec::Vec::new)
                            .push(value);
                        self
                    }
                });
                // The all-at-once setter, unless the name would collide.
                if each != name {
                    let ty = &info.ty;
                    setters.extend(quote! {
                        pub fn #name(&mut self, value: #ty) -> &mut Self {
                            self.#name = ::std::option::Option::Some(value);
                            self
                        }
                    });
                }
            }
        }
    }

    // --- build() -------------------------------------------------------------
    let build_fields = infos.iter().map(|info| {
        let name = &info.ident;
        match &info.kind {
            Kind::Optional(_) => quote! { #name: self.#name.clone() },
            Kind::Repeated { .. } => {
                quote! { #name: self.#name.clone().unwrap_or_default() }
            }
            Kind::Plain => quote! {
                #name: self.#name.clone().ok_or(concat!(
                    "field `", stringify!(#name), "` is not set",
                ))?
            },
        }
    });

    let expanded = quote! {
        pub struct #builder_ident {
            #(#decls,)*
        }

        impl #ident {
            pub fn builder() -> #builder_ident {
                #builder_ident {
                    #(#inits,)*
                }
            }
        }

        impl #builder_ident {
            #setters

            pub fn build(
                &mut self,
            ) -> ::std::result::Result<#ident, ::std::boxed::Box<dyn ::std::error::Error>> {
                ::std::result::Result::Ok(#ident {
                    #(#build_fields,)*
                })
            }
        }
    };

    Ok(expanded)
}

/// Parse the contents of a `#[builder(...)]` attribute, expecting exactly
/// `each = "ident"`.
fn parse_each(meta: &Meta) -> syn::Result<Option<syn::Ident>> {
    let err = || syn::Error::new_spanned(meta, "expected `builder(each = \"...\")`");

    let tokens = match meta {
        Meta::List(list) => list.tokens.clone(),
        _ => return Err(err()),
    };

    let mut iter = tokens.into_iter();
    let first = iter.next();
    let second = iter.next();
    let third = iter.next();
    let fourth = iter.next();

    if let (
        Some(TokenTree::Ident(ident)),
        Some(TokenTree::Punct(punct)),
        Some(TokenTree::Literal(literal)),
        None,
    ) = (first, second, third, fourth)
    {
        if ident == "each" && punct.as_char() == '=' {
            if let syn::Lit::Str(lit) = syn::Lit::new(literal) {
                if let Ok(name) = lit.parse::<syn::Ident>() {
                    return Ok(Some(name));
                }
            }
        }
    }

    Err(err())
}

/// If `ty` is written `Option<T>`, return the `T`.
fn option_inner(ty: &Type) -> Option<&Type> {
    let path = match ty {
        Type::Path(type_path) if type_path.qself.is_none() => &type_path.path,
        _ => return None,
    };
    let segment = path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    single_type_arg(&segment.arguments)
}

/// If `ty` is written `Vec<T>`, return the `T`.
fn vec_elem(ty: &Type) -> Option<&Type> {
    let path = match ty {
        Type::Path(type_path) if type_path.qself.is_none() => &type_path.path,
        _ => return None,
    };
    let segment = path.segments.last()?;
    if segment.ident != "Vec" {
        return None;
    }
    single_type_arg(&segment.arguments)
}

fn single_type_arg(arguments: &PathArguments) -> Option<&Type> {
    let args = match arguments {
        PathArguments::AngleBracketed(args) => &args.args,
        _ => return None,
    };
    if args.len() != 1 {
        return None;
    }
    match args.first()? {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    }
}

// `Spanned` is only used for span-accurate errors in this file.
#[allow(dead_code)]
fn _assert_spanned<T: Spanned>(_: &T) {}
