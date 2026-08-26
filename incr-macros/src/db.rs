use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Fields, GenericArgument, PathArguments, Type, TypePath, spanned::Spanned,
};

enum InputKind {
    Table {
        key_ty: Box<Type>,
        val_ty: Box<Type>,
    },
    Scalar {
        val_ty: Box<Type>,
    },
}

fn extract_input_kind(ty: &Type) -> InputKind {
    if let Type::Path(TypePath { path, .. }) = ty
        && let Some(last_seg) = path.segments.last()
        && let PathArguments::AngleBracketed(args) = &last_seg.arguments
    {
        let types: Vec<&Type> = args
            .args
            .iter()
            .filter_map(|arg| match arg {
                GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .collect();

        if types.len() == 2 {
            return InputKind::Table {
                key_ty: Box::new((*types[0]).clone()),
                val_ty: Box::new((*types[1]).clone()),
            };
        } else if types.len() == 1 {
            return InputKind::Scalar {
                val_ty: Box::new((*types[0]).clone()),
            };
        }
    }

    InputKind::Scalar {
        val_ty: Box::new(ty.clone()),
    }
}

pub fn expand_derive_db(input: DeriveInput) -> syn::Result<TokenStream> {
    let struct_ident = &input.ident;

    let Data::Struct(data_struct) = &input.data else {
        return Err(syn::Error::new_spanned(
            struct_ident,
            "#[derive(Db)] is only supported on structs with named fields",
        ));
    };

    let Fields::Named(fields_named) = &data_struct.fields else {
        return Err(syn::Error::new_spanned(
            struct_ident,
            "#[derive(Db)] is only supported on structs with named fields",
        ));
    };

    let memo_field = fields_named
        .named
        .iter()
        .find(|f| f.ident.as_ref().map(|id| id == "memo").unwrap_or(false));

    let Some(memo_field) = memo_field else {
        return Err(syn::Error::new_spanned(
            struct_ident,
            "struct deriving Db must contain a field named `memo` (of type incr::MemoTable)",
        ));
    };

    let memo_ident = memo_field.ident.as_ref().unwrap();

    let mut marker_structs = Vec::new();
    let mut generated_methods = Vec::new();

    for field in &fields_named.named {
        let has_input_attr = field.attrs.iter().any(|attr| attr.path().is_ident("input"));

        if !has_input_attr {
            continue;
        }

        let field_ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new(field.span(), "input field must have a name"))?;

        let setter_ident = format_ident!("set_{}", field_ident);
        let marker_ident = format_ident!("__IncrInputMarker_{}_{}", struct_ident, field_ident);
        let field_name_str = field_ident.to_string();

        marker_structs.push(quote! {
            #[allow(non_camel_case_types)]
            #[doc(hidden)]
            pub struct #marker_ident;
        });

        match extract_input_kind(&field.ty) {
            InputKind::Table { key_ty, val_ty } => {
                generated_methods.push(quote! {
                    pub fn #field_ident(&self, key: #key_ty) -> #val_ty {
                        let input_id = ::incr::InputId::of::<#marker_ident, #key_ty>(
                            &key,
                            #field_name_str,
                        );
                        ::incr::runtime::record_input_dep(input_id);
                        self.#field_ident.get(&key).cloned().expect("input key not found")
                    }

                    pub fn #setter_ident(&mut self, key: #key_ty, value: #val_ty) {
                        let epoch = self.#memo_ident.bump_epoch();
                        let input_id = ::incr::InputId::of::<#marker_ident, #key_ty>(
                            &key,
                            #field_name_str,
                        );
                        let lookup = key.clone();
                        self.#field_ident.set(key, value, epoch);
                        // set() applies Eq cutoff; the stored epoch is the
                        // effective changed_at either way.
                        if let Some(at) = self.#field_ident.changed_at(&lookup) {
                            self.#memo_ident.note_input(input_id, at);
                        }
                    }
                });
            }
            InputKind::Scalar { val_ty } => {
                generated_methods.push(quote! {
                    pub fn #field_ident(&self) -> #val_ty {
                        let input_id = ::incr::InputId::of::<#marker_ident, ()>(
                            &(),
                            #field_name_str,
                        );
                        ::incr::runtime::record_input_dep(input_id);
                        self.#field_ident.get().cloned().expect("input not set")
                    }

                    pub fn #setter_ident(&mut self, value: #val_ty) {
                        let epoch = self.#memo_ident.bump_epoch();
                        let input_id = ::incr::InputId::of::<#marker_ident, ()>(
                            &(),
                            #field_name_str,
                        );
                        self.#field_ident.set(value, epoch);
                        if let Some(at) = self.#field_ident.changed_at() {
                            self.#memo_ident.note_input(input_id, at);
                        }
                    }
                });
            }
        }
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        #(#marker_structs)*

        impl #impl_generics #struct_ident #ty_generics #where_clause {
            #(#generated_methods)*
        }

        impl #impl_generics ::incr::Database for #struct_ident #ty_generics #where_clause {
            fn memo_table(&self) -> &::incr::MemoTable {
                &self.#memo_ident
            }

            fn memo_table_mut(&mut self) -> &mut ::incr::MemoTable {
                &mut self.#memo_ident
            }
        }
    };

    Ok(expanded)
}
