macro_rules! config_derives {
    () => {
        /// Implements `Default` by calling the type's existing no-input builder.
        #[cfg(feature = "built-default")]
        #[proc_macro_derive(BuiltDefault)]
        pub fn built_default(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
            config::built::expand(input)
        }

        /// Generates the document patch and merge operation for a configuration struct.
        #[cfg(feature = "patch")]
        #[proc_macro_derive(Patch, attributes(patch, config))]
        pub fn patch(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
            config::expand(input)
        }
    };
}

pub(crate) use config_derives;
