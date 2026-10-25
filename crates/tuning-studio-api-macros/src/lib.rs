//! `#[derive(Watchable)]` for tuning-studio-api.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Ident, Index, parse_macro_input};

/// Implement `Watchable` for a struct that is not `Copy`. It generates a `Copy`
/// struct named after it with `Snapshot` appended, holding each field's own
/// snapshot under the same name. `#[watch(skip)]` leaves a field out, such as a
/// driver handle with no value to show. A `Copy` type is already `Watchable`,
/// so deriving it there conflicts.
#[proc_macro_derive(Watchable, attributes(watch))]
pub fn derive_watchable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "`Watchable` cannot be derived for a generic type yet; implement it by hand",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`Watchable` can be derived only for structs; implement it by hand",
        ));
    };
    let name = &input.ident;
    let vis = &input.vis;
    let snapshot = format_ident!("{name}Snapshot");
    let api = quote!(::tuning_studio_api);
    let doc = format!("What the host reads of [`{name}`], from its `Watchable` derive");

    let mut kept = Vec::new();
    for (i, field) in data.fields.iter().enumerate() {
        if !skipped(field)? {
            kept.push((i, field));
        }
    }
    let (fields, values) = match &data.fields {
        Fields::Named(_) => {
            let defs = kept.iter().map(|(_, f)| {
                let (vis, ident, ty) = (&f.vis, &f.ident, &f.ty);
                quote!(#vis #ident: <#ty as #api::Watchable>::Snapshot)
            });
            let values = kept.iter().map(|(_, f)| {
                let ident = &f.ident;
                quote!(#ident: #api::Watchable::snapshot(&self.#ident))
            });
            (quote!({ #(#defs,)* }), quote!({ #(#values,)* }))
        }
        Fields::Unnamed(_) => {
            let defs = kept.iter().map(|(_, f)| {
                let (vis, ty) = (&f.vis, &f.ty);
                quote!(#vis <#ty as #api::Watchable>::Snapshot)
            });
            let values = kept.iter().map(|(i, _)| {
                let index = Index::from(*i);
                quote!(#api::Watchable::snapshot(&self.#index))
            });
            (quote!(( #(#defs,)* );), quote!(( #(#values,)* )))
        }
        Fields::Unit => (quote!(;), quote!()),
    };
    // A tuple struct's snapshot is built with a call, a named one with a literal.
    let construct = match &data.fields {
        Fields::Unit => quote!(#snapshot),
        _ => quote!(#snapshot #values),
    };
    let unused = Ident::new("dead_code", Span::call_site());
    Ok(quote! {
        #[doc = #doc]
        #[derive(Clone, Copy)]
        #[allow(#unused)]
        #vis struct #snapshot #fields

        impl #api::Watchable for #name {
            type Snapshot = #snapshot;
            fn snapshot(&self) -> #snapshot {
                #construct
            }
        }
    })
}

fn skipped(field: &syn::Field) -> syn::Result<bool> {
    let mut skip = false;
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("watch")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                skip = true;
                Ok(())
            } else {
                Err(meta.error("expected `skip`"))
            }
        })?;
    }
    Ok(skip)
}
