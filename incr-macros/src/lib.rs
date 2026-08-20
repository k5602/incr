use proc_macro::TokenStream;
use syn::{DeriveInput, ItemFn, parse_macro_input};

mod db;
mod query;

#[proc_macro_derive(Db, attributes(input))]
pub fn derive_db(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    db::expand_derive_db(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_attribute]
pub fn query(args: TokenStream, input: TokenStream) -> TokenStream {
    let _ = args;
    let item_fn = parse_macro_input!(input as ItemFn);
    query::expand_query(item_fn)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
