use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Expr, LitBool, Type, parse_macro_input};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn derive(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let mut data = None;
    let mut draw = None;
    let mut reads_pointer = false;
    for attr in &input.attrs {
        if !attr.path().is_ident("control_painter") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("data") {
                data = Some(meta.value()?.parse::<Type>()?);
                return Ok(());
            }
            if meta.path.is_ident("draw") {
                draw = Some(meta.value()?.parse::<Expr>()?);
                return Ok(());
            }
            if meta.path.is_ident("reads_pointer") {
                reads_pointer = meta.value()?.parse::<LitBool>()?.value;
                return Ok(());
            }
            Err(meta.error("unsupported control_painter option"))
        })?;
    }
    let data =
        data.ok_or_else(|| syn::Error::new_spanned(input, "missing control painter data type"))?;
    let draw =
        draw.ok_or_else(|| syn::Error::new_spanned(input, "missing control painter draw call"))?;
    let pointer = reads_pointer.then(|| {
        quote!(
            const READS_POINTER: bool = true;
        )
    });
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics crate::atoms::painter::ControlPainter for #name #type_generics #where_clause {
            type Data = #data;
            #pointer

            fn draw(
                &self,
                list: &mut crate::draw::DrawListBuilder,
                text: &mut crate::shaping::TextContext,
                data: &Self::Data,
                bounds: crate::draw::Rect,
                state: crate::atoms::button::VisualState,
            ) {
                #draw;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use syn::parse_quote;

    use super::derive;

    #[kithara::test(native, flash(false))]
    fn emits_data_draw_and_pointer_policy() {
        let input = parse_quote! {
            #[control_painter(
                data = crate::atoms::painter::Labelled,
                draw = self.paint(list, text, &data.label, data.active, bounds),
                reads_pointer = true
            )]
            struct Chip;
        };
        let output = derive(&input).expect("valid derive").to_string();
        assert!(output.contains("type Data"));
        assert!(output.contains("self . paint"));
        assert!(output.contains("READS_POINTER"));
    }

    #[kithara::test(native, flash(false))]
    fn requires_data_and_draw() {
        let input = parse_quote!(
            struct Chip;
        );
        assert!(derive(&input).is_err());
    }
}
