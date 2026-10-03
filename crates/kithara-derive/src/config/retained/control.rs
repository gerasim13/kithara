use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Attribute, DeriveInput, Ident};

use super::{
    field::{LiveField, Member},
    implementation::docs,
    live::{cfgs, change_name},
};
use crate::config::field::Live;

/// `<Name>Control`: on any owner that configures the struct, a getter of each
/// field with an accessor, a `Nested` getter of each nested live field and a
/// setter of each live value field.
pub(super) fn control(item: &DeriveInput, members: &[Member<'_>]) -> TokenStream {
    let name = &item.ident;
    let visibility = &item.vis;
    let change = change_name(item);
    let control = format_ident!("{}Control", name, span = Span::call_site());
    let configure = quote!(::kithara_config::Configure<#change>);
    let methods = members.iter().map(|member| {
        let field = member.name;
        let ty = member.ty;
        let surface = docs(member.attributes);
        let cfgs: Vec<&Attribute> = cfgs(member.attributes).collect();
        let mut getter = field.clone();
        getter.set_span(Span::call_site());
        let get = if member.nested && member.live.is_some() {
            Some(quote! {
                #(#cfgs)*
                #(#surface)*
                fn #getter(&self) -> ::kithara_config::Nested<&Self, fn(&#name) -> #ty> {
                    ::kithara_config::__private::nested(self, |config: &#name| config.#field)
                }
            })
        } else {
            member.accessor.as_ref().map(|_| {
                quote! {
                    #(#cfgs)*
                    #(#surface)*
                    fn #getter(&self) -> #ty {
                        <Self as #configure>::settings(self).#field
                    }
                }
            })
        };
        let set = member.live.as_ref().filter(|_| !member.nested).map(|live| {
            let setter = format_ident!("set_{}", field, span = Span::call_site());
            let variant = &live.variant;
            let doc = format!(" Hands the owner a change of `{field}` for the nearest moment.");
            quote! {
                #(#cfgs)*
                #[doc = #doc]
                ///
                /// # Errors
                ///
                /// Returns the owner's refusal.
                fn #setter(
                    &self,
                    value: #ty,
                ) -> ::core::result::Result<<Self as #configure>::Output, <Self as #configure>::Error> {
                    <Self as #configure>::configure(
                        self,
                        #change::#variant(value),
                        ::core::default::Default::default(),
                    )
                }
            }
        });
        quote! { #get #set }
    });
    let subject = format!(" Getters and setters of [`{name}`] on any owner that configures it.");
    quote! {
        #[doc = #subject]
        #visibility trait #control: ::kithara_config::Configure<#change, Config = #name> {
            #(#methods)*
        }

        #[automatically_derived]
        impl<__KitharaConfigOwner> #control for __KitharaConfigOwner
        where
            __KitharaConfigOwner: ::kithara_config::Configure<#change, Config = #name> + ?Sized,
        {
        }
    }
}

/// `<Name>Exec`: how an owner executes a change, each `live(owner)` field
/// through its own method and every other live field through `exec_live`.
pub(super) fn exec(item: &DeriveInput, fields: &[(&Member<'_>, &LiveField)]) -> TokenStream {
    let name = &item.ident;
    let visibility = &item.vis;
    let change = change_name(item);
    let exec = format_ident!("{}Exec", name, span = Span::call_site());
    let cx = Ident::new("__KitharaCx", Span::call_site());
    let mut methods: Vec<TokenStream> = Vec::new();
    let mut arms: Vec<TokenStream> = Vec::new();
    let mut shared = false;
    for (member, live) in fields {
        let variant = &live.variant;
        let field = member.name;
        let ty = member.ty;
        let cfgs: Vec<&Attribute> = cfgs(member.attributes).collect();
        if matches!(live.mode, Live::Owner) {
            let method = format_ident!("exec_{}", field, span = Span::call_site());
            let doc = format!(" Executes a change of `{field}` at `at`.");
            methods.push(quote! {
                #(#cfgs)*
                #[doc = #doc]
                fn #method(&mut self, value: #ty, at: Self::At, cx: &mut #cx) -> Self::Output;
            });
            arms.push(quote! {
                #(#cfgs)*
                #change::#variant(value) => self.#method(value, at, cx)
            });
        } else {
            shared = true;
            arms.push(quote! {
                #(#cfgs)*
                #change::#variant(_) => self.exec_live(change, at, cx)
            });
        }
    }
    let exec_live = shared.then(|| {
        quote! {
            /// Executes a change of a live field that is not `live(owner)`.
            fn exec_live(&mut self, change: #change, at: Self::At, cx: &mut #cx) -> Self::Output;
        }
    });
    let subject = format!(" How an owner executes each change of [`{name}`].");
    quote! {
        #[doc = #subject]
        #visibility trait #exec<#cx: ?Sized> {
            /// When a change executes.
            type At;
            /// What executing a change yields.
            type Output;

            /// Executes one change through the method of its field.
            fn exec(&mut self, change: #change, at: Self::At, cx: &mut #cx) -> Self::Output {
                match change {
                    #(#arms,)*
                }
            }

            #(#methods)*

            #exec_live
        }
    }
}
