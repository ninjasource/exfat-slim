// vendored from bisync_macros by jm4ier
// MIT or Apache-2.0, redistributed under Apache-2.0

// all these macros do is remove async and await tokens from the token stream
// to allow for dual sync and async support without duplication in the codebase.
// this works for simplistic async use cases

use proc_macro::{Group, TokenStream};

#[proc_macro_attribute]
pub fn internal_noop(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn internal_delete(_attr: TokenStream, _item: TokenStream) -> TokenStream {
    TokenStream::new()
}

#[proc_macro_attribute]
pub fn internal_strip_async(attr: TokenStream, item: TokenStream) -> TokenStream {
    use proc_macro::TokenTree;

    let mut new = Vec::new();
    let tokens: Vec<TokenTree> = item.into_iter().collect();
    let mut i = 0;

    while i < tokens.len() {
        match &tokens[i] {
            TokenTree::Group(group) => {
                let new_inner = internal_strip_async(attr.clone(), group.stream());
                let mut new_group = Group::new(group.delimiter(), new_inner);
                new_group.set_span(group.span());
                new.push(TokenTree::Group(new_group));
            }
            TokenTree::Ident(ident) => {
                if ident.to_string() == "async" {
                    if i + 1 < tokens.len() && tokens[i + 1].to_string() == "move" {
                        i += 1;
                    }
                } else {
                    new.push(tokens[i].clone());
                }
            }
            TokenTree::Punct(punct) => {
                if punct.as_char() == '.'
                    && i + 1 < tokens.len()
                    && tokens[i + 1].to_string() == "await"
                {
                    i += 2;
                    continue;
                } else {
                    new.push(tokens[i].clone());
                }
            }
            TokenTree::Literal(..) => new.push(tokens[i].clone()),
        }
        i += 1;
    }

    new.into_iter().collect()
}
