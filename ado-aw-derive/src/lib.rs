//! Derive macros for sanitization traits.
//!
//! Provides `#[derive(SanitizeConfig)]` and `#[derive(SanitizeContent)]` for automatic
//! implementation of field-level sanitization on structs.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, GenericArgument, PathArguments, Type, parse_macro_input};

// ── Shared helpers ─────────────────────────────────────────────────────────

/// Parsed field-level attributes for a sanitization derive.
struct FieldAttrs {
    skip: bool,
    nested: bool,
    light: bool,
    sanitize_keys: bool,
}

/// Parse `#[sanitize_config(...)]` or `#[sanitize_content(...)]` attributes on a field.
fn parse_field_attrs(field: &syn::Field, attr_name: &str) -> FieldAttrs {
    let mut attrs = FieldAttrs {
        skip: false,
        nested: false,
        light: false,
        sanitize_keys: false,
    };

    for attr in &field.attrs {
        if attr.path().is_ident(attr_name) {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    attrs.skip = true;
                } else if meta.path.is_ident("nested") {
                    attrs.nested = true;
                } else if meta.path.is_ident("light") {
                    attrs.light = true;
                } else if meta.path.is_ident("sanitize_keys") {
                    attrs.sanitize_keys = true;
                }
                Ok(())
            });
        }
    }

    attrs
}

/// Check whether a `Type` is exactly `String`.
fn is_string(ty: &Type) -> bool {
    matches_path_ident(ty, "String")
}

/// Check whether a `Type` is `Option<String>`.
fn is_option_string(ty: &Type) -> bool {
    is_generic_of(ty, "Option", is_string)
}

/// Check whether a `Type` is `Option<Vec<String>>`.
fn is_option_vec_string(ty: &Type) -> bool {
    is_generic_of(ty, "Option", is_vec_string)
}

/// Check whether a `Type` is `Vec<String>`.
fn is_vec_string(ty: &Type) -> bool {
    is_generic_of(ty, "Vec", is_string)
}

/// Check whether a `Type` is `HashMap<String, String>` (std or std::collections).
fn is_hashmap_string_string(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        let seg = type_path.path.segments.last();
        if let Some(seg) = seg {
            if seg.ident == "HashMap" {
                if let PathArguments::AngleBracketed(args) = &seg.arguments {
                    let type_args: Vec<_> = args
                        .args
                        .iter()
                        .filter_map(|a| {
                            if let GenericArgument::Type(t) = a {
                                Some(t)
                            } else {
                                None
                            }
                        })
                        .collect();
                    return type_args.len() == 2
                        && is_string(type_args[0])
                        && is_string(type_args[1]);
                }
            }
        }
    }
    false
}

/// Check whether a `Type` is a simple path matching `ident` (e.g., `String`).
fn matches_path_ident(ty: &Type, ident: &str) -> bool {
    if let Type::Path(type_path) = ty {
        type_path.path.is_ident(ident)
    } else {
        false
    }
}

/// Check whether a `Type` is `Wrapper<Inner>` where `Inner` satisfies `pred`.
fn is_generic_of(ty: &Type, wrapper: &str, pred: fn(&Type) -> bool) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            if seg.ident == wrapper {
                if let PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(GenericArgument::Type(inner)) = args.args.first() {
                        return pred(inner);
                    }
                }
            }
        }
    }
    false
}

// ── #[derive(SanitizeConfig)] ──────────────────────────────────────────────

/// Derive macro for `SanitizeConfig`.
///
/// Automatically implements `sanitize_config_fields(&mut self)` by calling
/// `crate::sanitize::sanitize_config()` on all recognised string-typed fields.
///
/// # Field attributes
///
/// - `#[sanitize_config(skip)]` — do not sanitize this field.
/// - `#[sanitize_config(nested)]` — call `sanitize_config_fields()` on this field
///   (it must implement `SanitizeConfig`).
/// - `#[sanitize_config(sanitize_keys)]` — for `HashMap<String, String>`, also
///   sanitize the keys (default: values only).
#[proc_macro_derive(SanitizeConfig, attributes(sanitize_config))]
pub fn derive_sanitize_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "SanitizeConfig can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input.ident,
                "SanitizeConfig can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let mut stmts = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let attrs = parse_field_attrs(field, "sanitize_config");

        if attrs.skip {
            continue;
        }

        if attrs.nested {
            stmts.push(quote! {
                self.#field_name.sanitize_config_fields();
            });
            continue;
        }

        let ty = &field.ty;

        if is_string(ty) {
            stmts.push(quote! {
                self.#field_name = crate::sanitize::sanitize_config(&self.#field_name);
            });
        } else if is_option_string(ty) {
            stmts.push(quote! {
                self.#field_name = self.#field_name.as_deref().map(crate::sanitize::sanitize_config);
            });
        } else if is_option_vec_string(ty) {
            stmts.push(quote! {
                self.#field_name = self.#field_name.as_ref().map(|v| {
                    v.iter().map(|s| crate::sanitize::sanitize_config(s)).collect()
                });
            });
        } else if is_vec_string(ty) {
            stmts.push(quote! {
                self.#field_name = self.#field_name.iter().map(|s| crate::sanitize::sanitize_config(s)).collect();
            });
        } else if is_hashmap_string_string(ty) {
            if attrs.sanitize_keys {
                stmts.push(quote! {
                    self.#field_name = self.#field_name.iter().map(|(k, v)| {
                        (crate::sanitize::sanitize_config(k), crate::sanitize::sanitize_config(v))
                    }).collect();
                });
            } else {
                stmts.push(quote! {
                    for v in self.#field_name.values_mut() {
                        *v = crate::sanitize::sanitize_config(v);
                    }
                });
            }
        }
        // else: skip (numeric, boolean, enum, complex types)
    }

    let expanded = quote! {
        impl #impl_generics crate::sanitize::SanitizeConfig for #name #ty_generics #where_clause {
            fn sanitize_config_fields(&mut self) {
                #(#stmts)*
            }
        }
    };

    expanded.into()
}

// ── #[derive(SanitizeContent)] ─────────────────────────────────────────────

/// Derive macro for `SanitizeContent`.
///
/// Automatically implements `sanitize_content_fields(&mut self)` by calling
/// `crate::sanitize::sanitize()` on all recognised string-typed fields.
///
/// # Field attributes
///
/// - `#[sanitize_content(skip)]` — do not sanitize this field.
/// - `#[sanitize_content(nested)]` — call `sanitize_content_fields()` on this field
///   (it must implement `SanitizeContent`).
/// - `#[sanitize_content(light)]` — apply only control character removal (for
///   structural identifiers like wiki page paths that shouldn't be HTML-escaped).
#[proc_macro_derive(SanitizeContent, attributes(sanitize_content))]
pub fn derive_sanitize_content(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "SanitizeContent can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input.ident,
                "SanitizeContent can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let mut stmts = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();

        // The `name` field on tool result structs is a tool identifier, not content.
        if field_name == "name" {
            continue;
        }

        let attrs = parse_field_attrs(field, "sanitize_content");

        if attrs.skip {
            continue;
        }

        if attrs.nested {
            stmts.push(quote! {
                self.#field_name.sanitize_content_fields();
            });
            continue;
        }

        let ty = &field.ty;

        // Choose the sanitization function based on the `light` attribute.
        let sanitize_fn = if attrs.light {
            quote! { crate::sanitize::sanitize_light }
        } else {
            quote! { crate::sanitize::sanitize }
        };

        if is_string(ty) {
            stmts.push(quote! {
                self.#field_name = #sanitize_fn(&self.#field_name);
            });
        } else if is_option_string(ty) {
            stmts.push(quote! {
                self.#field_name = self.#field_name.as_deref().map(#sanitize_fn);
            });
        } else if is_option_vec_string(ty) {
            stmts.push(quote! {
                self.#field_name = self.#field_name.as_ref().map(|v| {
                    v.iter().map(|s| #sanitize_fn(s)).collect()
                });
            });
        } else if is_vec_string(ty) {
            stmts.push(quote! {
                self.#field_name = self.#field_name.iter().map(|s| #sanitize_fn(s)).collect();
            });
        }
        // else: skip
    }

    let expanded = quote! {
        impl #impl_generics crate::sanitize::SanitizeContent for #name #ty_generics #where_clause {
            fn sanitize_content_fields(&mut self) {
                #(#stmts)*
            }
        }
    };

    expanded.into()
}
