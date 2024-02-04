use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{punctuated::Punctuated, Data, DeriveInput, Fields, LitStr, Token};

use crate::helpers::{
    non_enum_error, non_single_field_variant_error, HasStrumVariantProperties, HasTypeProperties,
};

pub fn display_inner(ast: &DeriveInput) -> syn::Result<TokenStream> {
    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let variants = match &ast.data {
        Data::Enum(v) => &v.variants,
        _ => return Err(non_enum_error()),
    };

    let type_properties = ast.get_type_properties()?;

    let mut arms = Vec::new();
    for variant in variants {
        let ident = &variant.ident;
        let variant_properties = variant.get_variant_properties()?;

        if variant_properties.disabled.is_some() {
            continue;
        }

        if variant_properties.transparent.is_some() {
            let arm_end = match &variant.fields {
                Fields::Unnamed(f) if f.unnamed.len() == 1 => {
                    quote! { (ref v) => ::core::fmt::Display::fmt(v, f)}
                }
                Fields::Named(f) if f.named.len() == 1 => {
                    let ident = f.named.last().unwrap().ident.as_ref().unwrap();
                    quote! { {ref #ident} => ::core::fmt::Display::fmt(#ident, f) }
                }
                _ => return Err(non_single_field_variant_error()),
            };

            arms.push(quote! { #name::#ident #arm_end });
            continue;
        }

        // Look at all the serialize attributes.
        let mut output = variant_properties.get_preferred_name(type_properties.case_style);
        if let Some(prefix) = &type_properties.prefix {
            output = LitStr::new(&(prefix.value() + &output.value()), output.span());
        }

        let params = match variant.fields {
            Fields::Unit => quote! {},
            Fields::Unnamed(..) => quote! { (..) },
            Fields::Named(ref field_names) => {
                // Transform named params '{ name: String, age: u8 }' to '{ ref name, ref age }'
                let names: Punctuated<TokenStream, Token!(,)> = field_names
                    .named
                    .iter()
                    .map(|field| {
                        let ident = field.ident.as_ref().unwrap();
                        quote! { ref #ident }
                    })
                    .collect();

                quote! { {#names} }
            }
        };

        if variant_properties.to_string.is_none() && variant_properties.default.is_some() {
            match &variant.fields {
                Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                    arms.push(quote! { #name::#ident(ref s) => ::core::fmt::Display::fmt(s, f) });
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        variant,
                        "Default only works on newtype structs with a single String field",
                    ))
                }
            }
        } else {
            let arm = if let Fields::Named(ref field_names) = variant.fields {
                let used_vars = capture_format_string_idents(&output)?;
                if used_vars.is_empty() {
                    quote! { #name::#ident #params => ::core::fmt::Display::fmt(#output, f) }
                } else {
                    // Create args like 'name = name, age = age' for format macro
                    let args: Punctuated<_, Token!(,)> = field_names
                        .named
                        .iter()
                        .filter_map(|field| {
                            let ident = field.ident.as_ref().unwrap();
                            // Only contain variables that are used in format string
                            if !used_vars.contains(ident) {
                                None
                            } else {
                                Some(quote! { #ident = #ident })
                            }
                        })
                        .collect();

                    quote! {
                        #[allow(unused_variables)]
                        #name::#ident #params => ::core::fmt::Display::fmt(&format!(#output, #args), f)
                    }
                }
            } else {
                quote! { #name::#ident #params => ::core::fmt::Display::fmt(#output, f) }
            };

            arms.push(arm);
        }
    }

    if arms.len() < variants.len() {
        arms.push(quote! { _ => panic!("fmt() called on disabled variant.") });
    }

    Ok(quote! {
        impl #impl_generics ::core::fmt::Display for #name #ty_generics #where_clause {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::result::Result<(), ::core::fmt::Error> {
                match *self {
                    #(#arms),*
                }
            }
        }
    })
}

fn capture_format_string_idents(string_literal: &LitStr) -> syn::Result<Vec<Ident>> {
    // Remove escaped brackets
    let format_str = string_literal.value().replace("{{", "").replace("}}", "");

    let mut new_var_start_index: Option<usize> = None;
    let mut var_used: Vec<Ident> = Vec::new();

    for (i, chr) in format_str.bytes().enumerate() {
        if chr == b'{' {
            if new_var_start_index.is_some() {
                return Err(syn::Error::new_spanned(
                    string_literal,
                    "Bracket opened without closing previous bracket",
                ));
            }
            new_var_start_index = Some(i);
            continue;
        }

        if chr == b'}' {
            let start_index = new_var_start_index.take().ok_or(syn::Error::new_spanned(
                string_literal,
                "Bracket closed without previous opened bracket",
            ))?;

            let inside_brackets = &format_str[start_index + 1..i];
            let ident_str = inside_brackets.split(":").next().unwrap();
            let ident = syn::parse_str::<Ident>(ident_str).map_err(|_| {
                syn::Error::new_spanned(
                    string_literal,
                    "Invalid identifier inside format string bracket",
                )
            })?;
            var_used.push(ident);
        }
    }

    Ok(var_used)
}
