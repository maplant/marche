use proc_macro::{self, TokenStream};
use proc_macro2::Span;
use quote::{quote, ToTokens};
use syn::{
    parse_macro_input, parse_quote, punctuated::Punctuated, token::Comma, Block, Expr, Fields,
    Ident, ItemEnum, ItemFn, PatIdent,
};

fn transform_params(
    params: Punctuated<syn::FnArg, syn::token::Comma>,
) -> Punctuated<syn::FnArg, syn::token::Comma> {
    let mut unnamed = 0;
    params
        .into_iter()
        .map(|param| match param {
            syn::FnArg::Typed(mut ty) => {
                let pat = if let syn::Pat::Ident(pat_ident) = *ty.pat.clone() {
                    syn::Pat::Ident(pat_ident)
                } else {
                    unnamed += 1;
                    syn::Pat::Ident(PatIdent {
                        attrs: vec![],
                        by_ref: None,
                        mutability: None,
                        ident: Ident::new(&format!("t{unnamed}"), Span::call_site()),
                        subpat: None,
                    })
                };
                ty.pat = Box::new(pat);
                syn::FnArg::Typed(ty)
            }
            x => x,
        })
        .collect()
}

fn transform_params_to_call(params: Punctuated<syn::FnArg, syn::token::Comma>) -> Expr {
    // 1. Filter the params, so that only typed arguments remain
    // 2. Extract the ident (in case the pattern type is ident)
    let mut unnamed = 0;
    let idents = params.iter().filter_map(|param| {
        if let syn::FnArg::Typed(pat_type) = param {
            if let syn::Pat::Ident(pat_ident) = *pat_type.pat.clone() {
                return Some(pat_ident.ident);
            }
        }
        unnamed += 1;
        Some(Ident::new(&format!("t{unnamed}"), Span::call_site()))
    });

    // Add all idents to a Punctuated => param1, param2, ...
    let mut punctuated: Punctuated<syn::Ident, Comma> = Punctuated::new();
    idents.for_each(|ident| punctuated.push(ident));

    // Generate expression from Punctuated (and wrap with parentheses)
    let transformed_params = parse_quote!((#punctuated));
    transformed_params
}

#[proc_macro]
pub fn get_fn_name(item: TokenStream) -> TokenStream {
    let ItemFn { sig, .. } = parse_macro_input!(item as ItemFn);
    let ident = sig.ident;
    quote! {
        #ident
    }
    .into()
}

#[proc_macro_attribute]
pub fn json(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let ItemFn {
        attrs,
        vis,
        mut sig,
        block,
    } = parse_macro_input!(item as ItemFn);

    let inner = sig.output.clone();
    let args = sig.inputs.clone();
    let call_args = transform_params_to_call(sig.inputs.clone());
    sig.inputs = transform_params(sig.inputs.clone());
    sig.output = parse_quote!(-> (http::StatusCode, axum::Json<serde_json::Value>));

    let block: Block = parse_quote! {
        {
            async fn inner(#args) #inner {
                #block
            }
            match inner #call_args .await {
                Err(err) => {
                    use crate::ErrorCode;

                    tracing::error!("{}", err);
                    (
                        err.error_code(),
                        axum::Json(serde_json::json!({
                            "error": format!("{}", err),
                            "error_type": err,
                        }))
                    )
                },
                Ok(ok) => {
                    (
                        http::StatusCode::OK,
                        axum::Json(serde_json::json!({
                            "ok": ok,
                        }))
                    )
                }
            }
        }
    };

    ItemFn {
        attrs,
        vis,
        sig,
        block: Box::new(block),
    }
    .into_token_stream()
    .into()
}

#[proc_macro_derive(ErrorCode)]
pub fn error_code(input: TokenStream) -> TokenStream {
    let ItemEnum {
        ident, variants, ..
    } = parse_macro_input!(input as ItemEnum);

    let mut matches = Vec::new();
    for variant in variants {
        let status_code = if variant.ident == "Unauthorized" {
            quote! { http::StatusCode::UNAUTHORIZED }
        } else if variant.ident == "UnknownError"
            || variant.ident.to_string().starts_with("Internal")
        {
            quote! { http::StatusCode::INTERNAL_SERVER_ERROR }
        } else {
            continue;
        };
        let guard = match variant.fields {
            Fields::Named(..) => quote! { { .. } },
            Fields::Unnamed(..) => quote! { ( .. ) },
            Fields::Unit => quote! {},
        };
        let ident = variant.ident;
        matches.push(quote! {
            Self::#ident #guard => #status_code,
        });
    }

    quote! {
        impl crate::ErrorCode for #ident {
            fn error_code(&self) -> http::StatusCode {
                match self {
                    #( #matches )*
                    _ => http::StatusCode::BAD_REQUEST,
                }
            }
        }
    }
    .into()
}
