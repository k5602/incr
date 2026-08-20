use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, PatType, ReturnType, Type, TypeReference, spanned::Spanned};

pub fn expand_query(item_fn: ItemFn) -> syn::Result<TokenStream> {
    if item_fn.sig.asyncness.is_some() {
        return Err(syn::Error::new(
            item_fn.sig.fn_token.span(),
            "#[query] does not support async functions in incr v0.1",
        ));
    }

    if matches!(item_fn.sig.safety, syn::Safety::Unsafe(_)) {
        return Err(syn::Error::new(
            item_fn.sig.fn_token.span(),
            "#[query] functions cannot be unsafe",
        ));
    }

    if item_fn.sig.inputs.is_empty() {
        return Err(syn::Error::new(
            item_fn.sig.paren_token.span.join(),
            "#[query] function must have a database parameter (e.g. `db: &MyDb`)",
        ));
    }

    let fn_name = &item_fn.sig.ident;
    let fn_vis = &item_fn.vis;
    let fn_attrs = &item_fn.attrs;
    let user_block = &item_fn.block;
    let return_ty = match &item_fn.sig.output {
        ReturnType::Default => quote! { () },
        ReturnType::Type(_, ty) => quote! { #ty },
    };

    let first_arg = item_fn.sig.inputs.first().unwrap();
    let (db_pat, db_ty) = match first_arg {
        FnArg::Receiver(rec) => {
            if rec.mutability.is_some() {
                return Err(syn::Error::new(
                    rec.span(),
                    "#[query] receiver must be immutable `&self`",
                ));
            }
            return Err(syn::Error::new(
                rec.span(),
                "#[query] on methods inside impl blocks is not yet supported; declare as `fn name(db: &MyDb, ...)`",
            ));
        }
        FnArg::Typed(PatType { pat, ty, .. }) => {
            if let Type::Reference(TypeReference {
                mutability, elem, ..
            }) = &**ty
            {
                if mutability.is_some() {
                    return Err(syn::Error::new(
                        ty.span(),
                        "#[query] database parameter must be an immutable reference `&Db`",
                    ));
                }
                (pat.clone(), elem.clone())
            } else {
                return Err(syn::Error::new(
                    ty.span(),
                    "#[query] database parameter must be a reference (e.g. `db: &MyDb`)",
                ));
            }
        }
    };

    let mut query_params = Vec::new();
    let mut query_pats = Vec::new();
    let mut query_tys = Vec::new();

    for arg in item_fn.sig.inputs.iter().skip(1) {
        match arg {
            FnArg::Receiver(rec) => {
                return Err(syn::Error::new(rec.span(), "unexpected receiver argument"));
            }
            FnArg::Typed(PatType { pat, ty, .. }) => {
                query_params.push(arg);
                query_pats.push((**pat).clone());
                query_tys.push((**ty).clone());
            }
        }
    }

    let (key_ty, key_expr, unpack_stmt) = match query_pats.len() {
        0 => (quote! { () }, quote! { () }, quote! {}),
        1 => {
            let p = &query_pats[0];
            let t = &query_tys[0];
            (
                quote! { #t },
                quote! { #p.clone() },
                quote! { let #p = __key.clone(); },
            )
        }
        _ => {
            let pats = &query_pats;
            let tys = &query_tys;
            (
                quote! { (#(#tys),*) },
                quote! { (#(#pats.clone()),*) },
                quote! { let (#(#pats),*) = __key.clone(); },
            )
        }
    };

    let marker_ident = format_ident!("__IncrQueryMarker_{}", fn_name);
    let compute_fn_ident = format_ident!("__incr_compute_{}", fn_name);
    let fn_name_str = fn_name.to_string();

    let generics = &item_fn.sig.generics;
    let all_inputs = &item_fn.sig.inputs;

    let expanded = quote! {
        #[allow(non_camel_case_types)]
        #[doc(hidden)]
        pub struct #marker_ident;

        #[doc(hidden)]
        fn #compute_fn_ident #generics(#db_pat: &#db_ty, __key: &#key_ty) -> #return_ty {
            #unpack_stmt
            #user_block
        }

        #(#fn_attrs)*
        #fn_vis fn #fn_name #generics(#all_inputs) -> #return_ty {
            let __key = #key_expr;
            let __query_id = ::incr::QueryId::of::<#marker_ident, #key_ty>(
                &__key,
                #fn_name_str,
            );
            ::incr::runtime::execute_query(
                #db_pat,
                __query_id,
                |__db| #compute_fn_ident(__db, &__key),
            )
        }

        impl #db_ty {
            #(#fn_attrs)*
            #fn_vis fn #fn_name(&self, #(#query_params),*) -> #return_ty {
                #fn_name(self, #(#query_pats),*)
            }
        }
    };

    Ok(expanded)
}
