//! Guest SDK for Aurora app-plugin hooks (docs/09 §8, §12).
//!
//! Implement [`Hooks`] for a type (override only the hooks declared in
//! `permissions.hooks`; the rest return [`HookError::Unsupported`]) and export
//! it with [`export_hooks!`]. Build with `--target wasm32-wasip2` and
//! `crate-type = ["cdylib"]`; the output is already a component.
//!
//! Ergonomics note: a declarative `export_hooks!` macro is used instead of a
//! `#[aurora_plugin::hooks]` proc-macro attribute (documented deviation, 09 §12).
//!
//! ```ignore
//! use aurora_plugin_sdk::*;
//! struct P;
//! impl Hooks for P {
//!     fn token_mint(i: MintInput) -> Result<MintOutput, HookError> { /* ... */ }
//! }
//! export_hooks!(P);
//! ```
#![allow(clippy::all)]

#[allow(missing_docs, clippy::all, unused_unsafe)]
pub mod bindings {
    wit_bindgen::generate!({
        world: "plugin",
        path: "../aurora-hook-wit/wit",
        pub_export_macro: true,
        export_macro_name: "__export_plugin",
        default_bindings_module: "aurora_plugin_sdk::bindings",
    });
}

pub use bindings::aurora::plugin::types::{
    AuthorizeInput, AuthorizeOutput, Connected, ContinueWith, ExchangeInput, ExchangeOutput,
    ExchangeSource, FetchError, HookError, HttpRequest, HttpResponse, Kv, LogLevel, MintInput,
    MintOutput, SummarizeInput, Summary,
};

pub mod testing;

/// Hook implementations. Every method defaults to `unsupported`.
pub trait Hooks {
    fn connect_authorize(_input: AuthorizeInput) -> Result<AuthorizeOutput, HookError> {
        Err(HookError::Unsupported)
    }
    fn connect_exchange(_input: ExchangeInput) -> Result<ExchangeOutput, HookError> {
        Err(HookError::Unsupported)
    }
    fn token_mint(_input: MintInput) -> Result<MintOutput, HookError> {
        Err(HookError::Unsupported)
    }
    fn approval_summarize(_input: SummarizeInput) -> Result<Summary, HookError> {
        Err(HookError::Unsupported)
    }
}

/// Export a [`Hooks`] implementation as the component's `aurora:plugin/hooks`.
#[macro_export]
macro_rules! export_hooks {
    ($ty:ty) => {
        const _: () = {
            struct __AuroraShim;
            impl $crate::bindings::exports::aurora::plugin::hooks::Guest for __AuroraShim {
                fn connect_authorize(
                    i: $crate::AuthorizeInput,
                ) -> ::core::result::Result<$crate::AuthorizeOutput, $crate::HookError> {
                    <$ty as $crate::Hooks>::connect_authorize(i)
                }
                fn connect_exchange(
                    i: $crate::ExchangeInput,
                ) -> ::core::result::Result<$crate::ExchangeOutput, $crate::HookError> {
                    <$ty as $crate::Hooks>::connect_exchange(i)
                }
                fn token_mint(
                    i: $crate::MintInput,
                ) -> ::core::result::Result<$crate::MintOutput, $crate::HookError> {
                    <$ty as $crate::Hooks>::token_mint(i)
                }
                fn approval_summarize(
                    i: $crate::SummarizeInput,
                ) -> ::core::result::Result<$crate::Summary, $crate::HookError> {
                    <$ty as $crate::Hooks>::approval_summarize(i)
                }
            }
    $crate::bindings::__export_plugin!(__AuroraShim with_types_in $crate::bindings);
        };
    };
}

/// Perform an HTTP request through the host (egress is policed host-side).
pub fn fetch(req: &HttpRequest) -> Result<HttpResponse, FetchError> {
    bindings::aurora::plugin::host::fetch(req)
}

/// Log a message through the host.
pub fn log(level: LogLevel, message: &str) {
    bindings::aurora::plugin::host::log(level, message)
}

/// Random bytes from the host.
pub fn random_bytes(len: u32) -> Vec<u8> {
    bindings::aurora::plugin::host::random_bytes(len)
}

/// Host clock, unix seconds.
pub fn now_unix_seconds() -> u64 {
    bindings::aurora::plugin::host::now_unix_seconds()
}

/// Build a `Kv` list from pairs.
pub fn kv<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Kv> {
    pairs
        .into_iter()
        .map(|(k, v)| Kv {
            key: k.into(),
            value: v.into(),
        })
        .collect()
}

/// Look up a value in a `Kv` list.
pub fn kv_get<'a>(list: &'a [Kv], key: &str) -> Option<&'a str> {
    list.iter().find(|e| e.key == key).map(|e| e.value.as_str())
}
