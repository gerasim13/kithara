use syn::{
    Ident, LitStr, Token,
    parse::{Parse, ParseStream},
};

use crate::test::case::Case;

/// Case names for one asset function, rejecting the shapes an asset cannot use.
///
/// An unnamed case is numbered by position, so inserting a case ahead of it
/// would silently rename the accessor and re-address the stored bytes. An asset
/// case therefore has to be named.
pub(crate) fn case_names(fn_name: &Ident, cases: &[Case]) -> syn::Result<Vec<String>> {
    if cases.is_empty() {
        return Err(syn::Error::new(
            fn_name.span(),
            "an asset needs at least one `#[case::name(...)]`",
        ));
    }
    let mut names = Vec::with_capacity(cases.len());
    for case in cases {
        let name = case
            .name
            .as_ref()
            .ok_or_else(|| {
                syn::Error::new(
                    fn_name.span(),
                    "an asset case must be named: write `#[case::some_name(...)]`",
                )
            })?
            .to_string();
        if names.contains(&name) {
            return Err(syn::Error::new(
                fn_name.span(),
                format!("duplicate asset case `{name}`"),
            ));
        }
        names.push(name);
    }
    Ok(names)
}

/// Parsed `#[kithara::asset(ext = "…", content_type = "…")]`.
pub(crate) struct AssetArgs {
    pub(crate) content_type: LitStr,
    /// Bake the asset into the binary instead of reading it from the store.
    pub(crate) embed: bool,
    pub(crate) ext: LitStr,
}

impl Parse for AssetArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut content_type: Option<LitStr> = None;
        let mut embed = false;
        let mut ext: Option<LitStr> = None;

        while !input.is_empty() {
            let key = input.parse::<Ident>()?;
            if key == "embed" {
                if embed {
                    return Err(syn::Error::new(key.span(), "duplicate key `embed`"));
                }
                embed = true;
                if !input.is_empty() {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }
            input.parse::<Token![=]>()?;
            let value = input.parse::<LitStr>()?;
            let slot = match key.to_string().as_str() {
                "content_type" => &mut content_type,
                "ext" => &mut ext,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown asset key `{other}`; expected `ext`, `content_type`, \
                             or `embed`"
                        ),
                    ));
                }
            };
            if slot.is_some() {
                return Err(syn::Error::new(
                    key.span(),
                    format!("duplicate key `{key}`"),
                ));
            }
            *slot = Some(value);
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        let ext = ext.ok_or_else(|| input.error("asset needs `ext = \"…\"`"))?;
        let content_type =
            content_type.ok_or_else(|| input.error("asset needs `content_type = \"…\"`"))?;

        let ext_value = ext.value();
        if ext_value.is_empty() || ext_value.contains(['/', '\\', '.']) {
            return Err(syn::Error::new(
                ext.span(),
                "asset `ext` must be a bare file extension such as \"wav\"",
            ));
        }

        Ok(Self {
            content_type,
            embed,
            ext,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AssetArgs, case_names};
    use crate::test::case::Case;

    #[test]
    fn both_keys_are_parsed() {
        let args = syn::parse_str::<AssetArgs>(r#"ext = "wav", content_type = "audio/wav""#)
            .expect("valid attribute");
        assert_eq!(args.ext.value(), "wav");
        assert_eq!(args.content_type.value(), "audio/wav");
    }

    #[test]
    fn key_order_does_not_matter() {
        let args = syn::parse_str::<AssetArgs>(r#"content_type = "audio/mpeg", ext = "mp3""#)
            .expect("valid attribute");
        assert_eq!(args.ext.value(), "mp3");
        assert_eq!(args.content_type.value(), "audio/mpeg");
    }

    #[test]
    fn a_missing_key_is_rejected() {
        for input in [r#"ext = "wav""#, r#"content_type = "audio/wav""#, ""] {
            assert!(
                syn::parse_str::<AssetArgs>(input).is_err(),
                "accepted {input}",
            );
        }
    }

    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(
            syn::parse_str::<AssetArgs>(r#"ext = "wav", content_type = "audio/wav", cache = true"#)
                .is_err(),
        );
    }

    #[test]
    fn a_duplicate_key_is_rejected() {
        assert!(
            syn::parse_str::<AssetArgs>(r#"ext = "wav", ext = "mp3", content_type = "audio/wav""#)
                .is_err(),
        );
    }

    #[test]
    fn embed_is_off_by_default_and_opt_in() {
        let plain = syn::parse_str::<AssetArgs>(r#"ext = "wav", content_type = "audio/wav""#)
            .expect("valid attribute");
        assert!(!plain.embed);

        let embedded =
            syn::parse_str::<AssetArgs>(r#"ext = "wav", content_type = "audio/wav", embed"#)
                .expect("valid attribute");
        assert!(embedded.embed);
    }

    #[test]
    fn embed_is_rejected_twice() {
        assert!(
            syn::parse_str::<AssetArgs>(r#"ext = "wav", content_type = "audio/wav", embed, embed"#)
                .is_err(),
        );
    }

    #[test]
    fn an_extension_with_a_path_separator_is_rejected() {
        for ext in ["../escape", "sub/dir", ""] {
            let input = format!(r#"ext = "{ext}", content_type = "audio/wav""#);
            assert!(
                syn::parse_str::<AssetArgs>(&input).is_err(),
                "accepted {ext}"
            );
        }
    }

    fn ident(name: &str) -> syn::Ident {
        syn::parse_str::<syn::Ident>(name).expect("valid identifier")
    }

    fn named_case(name: &str) -> Case {
        Case {
            name: Some(ident(name)),
            values: vec![],
        }
    }

    #[test]
    fn case_names_keep_declaration_order() {
        let names = case_names(
            &ident("sine_wav"),
            &[named_case("a440"), named_case("a880")],
        )
        .expect("valid cases");
        assert_eq!(names, ["a440", "a880"]);
    }

    #[test]
    fn an_unnamed_case_is_rejected() {
        let unnamed = Case {
            name: None,
            values: vec![],
        };
        assert!(case_names(&ident("sine_wav"), &[unnamed]).is_err());
    }

    #[test]
    fn a_repeated_case_name_is_rejected() {
        assert!(
            case_names(
                &ident("sine_wav"),
                &[named_case("a440"), named_case("a440")]
            )
            .is_err(),
        );
    }

    #[test]
    fn an_empty_case_matrix_is_rejected() {
        assert!(case_names(&ident("sine_wav"), &[]).is_err());
    }
}
