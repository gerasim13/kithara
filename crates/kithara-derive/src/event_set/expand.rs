use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Error, Fields, Ident, Index, Result, Type};

struct Member {
    variant: Ident,
    index: Index,
    event: Type,
}

fn members(input: &DeriveInput) -> Result<Vec<Member>> {
    let Data::Enum(data) = &input.data else {
        return Err(Error::new_spanned(input, "`EventSet` needs an enum"));
    };
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "`EventSet` needs a concrete type",
        ));
    }
    if data.variants.is_empty() {
        return Err(Error::new_spanned(
            input,
            "`EventSet` needs at least one variant",
        ));
    }

    data.variants
        .iter()
        .enumerate()
        .map(|(index, variant)| {
            let Fields::Unnamed(fields) = &variant.fields else {
                return Err(Error::new_spanned(
                    variant,
                    "each `EventSet` variant wraps exactly one event type",
                ));
            };
            if fields.unnamed.len() != 1 {
                return Err(Error::new_spanned(
                    variant,
                    "each `EventSet` variant wraps exactly one event type",
                ));
            }
            let field = &fields.unnamed[0];
            Ok(Member {
                variant: variant.ident.clone(),
                event: field.ty.clone(),
                index: Index::from(index),
            })
        })
        .collect()
}

pub(crate) fn derive(input: &DeriveInput) -> Result<TokenStream> {
    let members = members(input)?;
    let set = &input.ident;

    let conversions = members.iter().map(|member| {
        let Member { variant, event, .. } = member;
        quote! {
            impl ::core::convert::From<#event> for #set {
                fn from(event: #event) -> Self {
                    Self::#variant(event)
                }
            }
        }
    });

    let receivers = members.iter().map(|Member { event, .. }| {
        quote! { ::kithara_events::TopicReceiver<#event>, }
    });
    let subscribers = members.iter().map(|Member { event, .. }| {
        quote! { ::kithara_events::TopicReceiver::<#event>::new(bus), }
    });

    let branches = members.iter().map(|Member { variant, index, .. }| {
        quote! {
            received = rx.#index.recv(), if !rx.#index.is_closed() => match received {
                ::core::result::Result::Err(::kithara_events::RecvError::Closed) => continue,
                other => return other.map(|envelope| envelope.map(Self::#variant)),
            },
        }
    });

    let attempts = members.iter().map(|Member { variant, index, .. }| {
        quote! {
            match rx.#index.try_recv() {
                ::core::result::Result::Ok(envelope) => {
                    return ::core::result::Result::Ok(envelope.map(Self::#variant));
                }
                ::core::result::Result::Err(::kithara_events::TryRecvError::Lagged(count)) => {
                    return ::core::result::Result::Err(
                        ::kithara_events::TryRecvError::Lagged(count),
                    );
                }
                ::core::result::Result::Err(::kithara_events::TryRecvError::Empty) => open = true,
                ::core::result::Result::Err(::kithara_events::TryRecvError::Closed) => {}
            }
        }
    });

    let publishes = members.iter().map(|Member { variant, .. }| {
        quote! { Self::#variant(event) => bus.publish_stamped(meta, event), }
    });

    Ok(quote! {
        #(#conversions)*

        impl ::kithara_events::EventSet for #set {
            type Receivers = (#(#receivers)*);

            fn subscribe(bus: &::kithara_events::EventBus) -> Self::Receivers {
                (#(#subscribers)*)
            }

            fn recv(
                rx: &mut Self::Receivers,
            ) -> impl ::core::future::Future<
                Output = ::core::result::Result<
                    ::kithara_events::Envelope<Self>,
                    ::kithara_events::RecvError,
                >,
            > + ::core::marker::Send {
                async move {
                    loop {
                        ::kithara_events::select! {
                            biased;
                            #(#branches)*
                            else => return ::core::result::Result::Err(
                                ::kithara_events::RecvError::Closed,
                            ),
                        }
                    }
                }
            }

            fn try_recv(
                rx: &mut Self::Receivers,
            ) -> ::core::result::Result<
                ::kithara_events::Envelope<Self>,
                ::kithara_events::TryRecvError,
            > {
                let mut open = false;
                #(#attempts)*
                if open {
                    ::core::result::Result::Err(::kithara_events::TryRecvError::Empty)
                } else {
                    ::core::result::Result::Err(::kithara_events::TryRecvError::Closed)
                }
            }

            fn publish(
                bus: &::kithara_events::EventBus,
                meta: ::kithara_events::EventMeta,
                event: Self,
            ) {
                match event {
                    #(#publishes)*
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use syn::{DeriveInput, parse_quote};

    use super::derive;

    fn expansion(input: &DeriveInput) -> String {
        derive(input).expect("the derive expands").to_string()
    }

    fn two_member_set() -> DeriveInput {
        parse_quote! {
            enum AudioLaneEvent {
                Audio(AudioEvent),
                Decoder(DecoderEvent),
            }
        }
    }

    #[test]
    fn each_variant_gets_a_from_impl() {
        let expanded = expansion(&two_member_set());

        assert!(
            expanded.contains("impl :: core :: convert :: From < AudioEvent > for AudioLaneEvent")
        );
        assert!(
            expanded
                .contains("impl :: core :: convert :: From < DecoderEvent > for AudioLaneEvent")
        );
    }

    #[test]
    fn the_receiver_tuple_holds_one_receiver_per_variant() {
        let expanded = expansion(&two_member_set());

        assert!(expanded.contains(
            "type Receivers = (:: kithara_events :: TopicReceiver < AudioEvent > , \
             :: kithara_events :: TopicReceiver < DecoderEvent > ,)"
        ));
    }

    #[test]
    fn recv_selects_over_every_open_receiver() {
        let expanded = expansion(&two_member_set());

        assert!(expanded.contains("biased ;"));
        assert!(expanded.contains("rx . 0 . recv () , if ! rx . 0 . is_closed ()"));
        assert!(expanded.contains("rx . 1 . recv () , if ! rx . 1 . is_closed ()"));
        assert!(expanded.contains(
            "else => return :: core :: result :: Result :: Err \
             (:: kithara_events :: RecvError :: Closed ,)"
        ));
    }

    #[test]
    fn publish_dispatches_to_the_wrapped_event() {
        let expanded = expansion(&two_member_set());

        assert!(expanded.contains("Self :: Audio (event) => bus . publish_stamped (meta , event)"));
        assert!(
            expanded.contains("Self :: Decoder (event) => bus . publish_stamped (meta , event)")
        );
    }

    #[test]
    fn a_struct_is_refused() {
        let input: DeriveInput = parse_quote! {
            struct NotASet {
                inner: u8,
            }
        };

        let error = derive(&input).expect_err("the derive refuses a struct");

        assert!(error.to_string().contains("needs an enum"));
    }

    #[test]
    fn a_generic_enum_is_refused() {
        let input: DeriveInput = parse_quote! {
            enum Generic<T> {
                Only(T),
            }
        };

        let error = derive(&input).expect_err("the derive refuses a type parameter");

        assert!(error.to_string().contains("needs a concrete type"));
    }

    #[test]
    fn an_empty_enum_is_refused() {
        let input: DeriveInput = parse_quote! {
            enum Empty {}
        };

        let error = derive(&input).expect_err("the derive refuses an empty enum");

        assert!(error.to_string().contains("at least one variant"));
    }

    #[test]
    fn a_unit_variant_is_refused() {
        let input: DeriveInput = parse_quote! {
            enum Mixed {
                Audio(AudioEvent),
                Silence,
            }
        };

        let error = derive(&input).expect_err("the derive refuses a unit variant");

        assert!(error.to_string().contains("wraps exactly one event type"));
    }

    #[test]
    fn a_named_field_variant_is_refused() {
        let input: DeriveInput = parse_quote! {
            enum Named {
                Audio { event: AudioEvent },
            }
        };

        let error = derive(&input).expect_err("the derive refuses a named field");

        assert!(error.to_string().contains("wraps exactly one event type"));
    }

    #[test]
    fn a_two_field_variant_is_refused() {
        let input: DeriveInput = parse_quote! {
            enum Pair {
                Both(AudioEvent, DecoderEvent),
            }
        };

        let error = derive(&input).expect_err("the derive refuses a two-field variant");

        assert!(error.to_string().contains("wraps exactly one event type"));
    }
}
