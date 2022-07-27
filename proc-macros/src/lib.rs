use proc_macro::{self, TokenStream};
use quote::{quote, ToTokens};
use syn::{
    Expr,
    parse_macro_input, parse_quote, Block,
    ItemFn, ReturnType,
    Type,
    PathArguments,
    GenericArgument,
    token::Comma,
    punctuated::Punctuated,
    Ident,
    PatIdent,
};
use proc_macro2::Span;

fn extract_type_from_result(ty: &Type) -> Type {
    match ty {
        Type::Path(typepath) if typepath.qself.is_none() => {
            // Get the first segment of the path (there is only one, in fact: "Option"):
            let type_params = typepath.path.segments.first().unwrap().arguments.clone();
            // It should have only on angle-bracketed param ("<String>"):
            let generic_arg = match type_params {
                PathArguments::AngleBracketed(params) => params.args.first().unwrap().clone(),
                _ => panic!("TODO: error handling"),
            };
            // This argument must be a type:
            match generic_arg {
                GenericArgument::Type(ty) => ty.clone(),
                _ => panic!("TODO: error handling"),
            }
        }
        _ => panic!("TODO: error handling"),
    }
}

fn transform_params(params: Punctuated<syn::FnArg, syn::token::Comma>) ->
    Punctuated<syn::FnArg, syn::token::Comma>
{
    let mut unnamed = 0;
    params.into_iter().map(|param|{
        match param {
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
            },
            x => x,
        }
    }).collect()
}

fn transform_params_to_call(params: Punctuated<syn::FnArg, syn::token::Comma>) -> Expr {
    // 1. Filter the params, so that only typed arguments remain
    // 2. Extract the ident (in case the pattern type is ident)
    let mut unnamed = 0;
    let idents = params.iter().filter_map(|param|{
        if let syn::FnArg::Typed(pat_type) = param {
            if let syn::Pat::Ident(pat_ident) = *pat_type.pat.clone() {
                return Some(pat_ident.ident)
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
    let ItemFn {
        sig,
        ..
    } = parse_macro_input!(item as ItemFn);
    let ident = sig.ident;
    quote! {
        #ident
    }.into()
}

#[proc_macro_attribute]
pub fn json_result(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let ItemFn {
        attrs,
        vis,
        mut sig,
        block,
    } = parse_macro_input!(item as ItemFn);

    let inner = match sig.output.clone() {
        ReturnType::Type(_, ty) => extract_type_from_result(&ty),
       _ => panic!("type is not a Json"),
    };
    let args = sig.inputs.clone();
    let call_args = transform_params_to_call(sig.inputs.clone());
    sig.inputs = transform_params(sig.inputs.clone());

    let block: Block = parse_quote! {
        {
            async fn inner(#args) -> #inner {
                #block
            }
            Json(inner #call_args .await)
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
