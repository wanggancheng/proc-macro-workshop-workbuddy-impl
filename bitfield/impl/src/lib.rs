use proc_macro::TokenStream;
use proc_macro2::{Group, Span, TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, Ident, Lit, LitInt, Meta, Type};

/// Re-spans every token (recursively) in `tokens` to `span`. Used to make a
/// generated assertion report its error at the exact source location that the
/// user should look at.
fn respan(tokens: TokenStream2, span: Span) -> TokenStream2 {
    tokens
        .into_iter()
        .map(|tt| match tt {
            TokenTree::Group(group) => {
                let mut group = Group::new(group.delimiter(), respan(group.stream(), span));
                group.set_span(span);
                TokenTree::Group(group)
            }
            mut other => {
                other.set_span(span);
                other
            }
        })
        .collect()
}

/// Defines the marker types `B1` ..= `B64` together with their `Specifier`
/// implementations. Invoked once from inside the `bitfield` crate.
#[proc_macro]
pub fn define_specifiers(_input: TokenStream) -> TokenStream {
    let mut tokens = TokenStream2::new();

    for bits in 1..=64usize {
        let name = format_ident!("B{}", bits);
        let accessor = accessor_type(bits);

        tokens.extend(quote! {
            #[allow(non_camel_case_types)]
            pub enum #name {}

            impl crate::Specifier for #name {
                const BITS: usize = #bits;

                type Accessor = #accessor;

                fn into_bits(value: #accessor) -> u64 {
                    value as u64
                }

                fn from_bits(bits: u64) -> #accessor {
                    bits as #accessor
                }
            }
        });
    }

    tokens.into()
}

fn accessor_type(bits: usize) -> TokenStream2 {
    if bits <= 8 {
        quote!(u8)
    } else if bits <= 16 {
        quote!(u16)
    } else if bits <= 32 {
        quote!(u32)
    } else {
        quote!(u64)
    }
}

/// `#[bitfield]` -- rewrite a struct of bit field specifiers into a byte array
/// with generated accessors.
#[proc_macro_attribute]
pub fn bitfield(args: TokenStream, input: TokenStream) -> TokenStream {
    let _ = args;
    let input = parse_macro_input!(input as DeriveInput);

    expand_bitfield(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_bitfield(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let vis = &input.vis;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "#[bitfield] requires a struct with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "#[bitfield] requires a struct with named fields",
            ))
        }
    };

    let mut names = Vec::new();
    let mut types: Vec<&Type> = Vec::new();
    for field in fields {
        let name = field.ident.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(field, "#[bitfield] requires named fields")
        })?;
        names.push(name.clone());
        types.push(&field.ty);
    }

    // Total bit count and byte count, both as const expressions so that they
    // work for type aliases and enums whose width is only known to the trait
    // system.
    let total_bits = quote! { (0usize #( + <#types as ::bitfield::Specifier>::BITS )*) };
    let bytes = quote! { (#total_bits / 8) };
    let mod8 = quote! { (#total_bits % 8) };

    // `#[bits = N]` assertions on individual fields. Everything is spanned to
    // the literal the user wrote so that a wrong value reports on that literal
    // rather than on the macro invocation.
    let mut bits_checks = TokenStream2::new();
    for field in fields {
        for attr in &field.attrs {
            if !attr.path().is_ident("bits") {
                continue;
            }
            let lit = bits_attribute(attr)?;
            let ty = &field.ty;
            bits_checks.extend(respan(
                quote! {
                    const _: [(); #lit] = [(); <#ty as ::bitfield::Specifier>::BITS];
                },
                lit.span(),
            ));
        }
    }

    // Accessors.
    let mut accessors = TokenStream2::new();
    for (index, (name, ty)) in names.iter().zip(&types).enumerate() {
        let previous: Vec<&Type> = types[..index].to_vec();
        let offset = quote! { (0usize #( + <#previous as ::bitfield::Specifier>::BITS )*) };

        let getter = format_ident!("get_{}", name);
        let setter = format_ident!("set_{}", name);

        accessors.extend(quote! {
            pub fn #getter(&self) -> <#ty as ::bitfield::Specifier>::Accessor {
                let offset: usize = #offset;
                let bits: usize = <#ty as ::bitfield::Specifier>::BITS;
                let mut value: u64 = 0;
                let mut i: usize = 0;
                while i < bits {
                    let bit = offset + i;
                    value |= (((self.data[bit / 8] >> (bit % 8)) & 1) as u64) << i;
                    i += 1;
                }
                <#ty as ::bitfield::Specifier>::from_bits(value)
            }

            pub fn #setter(&mut self, val: <#ty as ::bitfield::Specifier>::Accessor) {
                let offset: usize = #offset;
                let bits: usize = <#ty as ::bitfield::Specifier>::BITS;
                let value: u64 = <#ty as ::bitfield::Specifier>::into_bits(val);
                let mut i: usize = 0;
                while i < bits {
                    let bit = offset + i;
                    let mask: u8 = 1 << (bit % 8);
                    if (value >> i) & 1 == 1 {
                        self.data[bit / 8] |= mask;
                    } else {
                        self.data[bit / 8] &= !mask;
                    }
                    i += 1;
                }
            }
        });
    }

    let expanded = quote! {
        #[repr(C)]
        #vis struct #ident {
            data: [u8; #bytes],
        }

        impl #ident {
            pub fn new() -> Self {
                Self { data: [0; #bytes] }
            }

            #accessors
        }

        #bits_checks

        // The total size of the bitfield must be a multiple of 8 bits. A
        // `where` clause on an item is the least noisy way to demand a trait
        // bound: unlike a call to a generic helper function it produces no
        // "required by a bound in ..." note.
        #[allow(dead_code)]
        struct _TotalSizeIsMultipleOfEightBits
        where
            <[(); #mod8] as ::bitfield::checks::TypeArray>::Type:
                ::bitfield::checks::TotalSizeIsMultipleOfEightBits;
    };

    Ok(expanded)
}

fn bits_attribute(attr: &syn::Attribute) -> syn::Result<LitInt> {
    if let Meta::NameValue(name_value) = &attr.meta {
        if let syn::Expr::Lit(expr_lit) = &name_value.value {
            if let Lit::Int(lit) = &expr_lit.lit {
                return Ok(lit.clone());
            }
        }
    }
    Err(syn::Error::new_spanned(attr, "expected `#[bits = N]`"))
}

/// `#[derive(BitfieldSpecifier)]` for enums used as bit field specifiers.
#[proc_macro_derive(BitfieldSpecifier, attributes(bits))]
pub fn bitfield_specifier(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand_specifier(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_specifier(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;

    let variants = match &input.data {
        Data::Enum(data) => &data.variants,
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "BitfieldSpecifier expected an enum",
            ))
        }
    };

    let bits = enum_bits(input, variants.len())?;

    let variant_idents: Vec<&Ident> = variants.iter().map(|variant| &variant.ident).collect();

    // Check that every discriminant fits in `bits` bits. Each check is written
    // as a `where` clause on an otherwise unused type so that the trait bound
    // is what fails, and every token is spanned to the variant itself so that
    // the error points at the variant that is out of range.
    let mut range_checks = TokenStream2::new();
    for variant in variants {
        let variant_ident = &variant.ident;
        let span = variant_ident.span();
        let bound = (1isize << bits) as isize;

        range_checks.extend(respan(
            quote! {
                const _: () = {
                    #[allow(dead_code)]
                    struct _DiscriminantInRange
                    where
                        <[(); (
                            (#ident::#variant_ident as isize) >= 0
                                && (#ident::#variant_ident as isize) < #bound
                        ) as usize] as ::bitfield::checks::InRangeArray>::Type:
                            ::bitfield::checks::DiscriminantInRange;
                };
            },
            span,
        ));
    }

    let expanded = quote! {
        impl ::bitfield::Specifier for #ident {
            const BITS: usize = #bits;

            type Accessor = Self;

            fn into_bits(value: Self) -> u64 {
                value as u64
            }

            fn from_bits(bits: u64) -> Self {
                #(
                    if bits == (Self::#variant_idents as u64) {
                        return Self::#variant_idents;
                    }
                )*
                unreachable!("discriminant is out of range")
            }
        }

        #range_checks
    };

    Ok(expanded)
}

fn enum_bits(input: &DeriveInput, variant_count: usize) -> syn::Result<usize> {
    for attr in &input.attrs {
        if attr.path().is_ident("bits") {
            return Ok(bits_attribute(attr)?.base10_parse::<usize>()?);
        }
    }

    if variant_count == 0 || (variant_count & (variant_count - 1)) != 0 {
        return Err(syn::Error::new(
            Span::call_site(),
            "BitfieldSpecifier expected a number of variants which is a power of 2",
        ));
    }

    let mut bits = 0usize;
    while (1usize << bits) < variant_count {
        bits += 1;
    }
    Ok(bits)
}
